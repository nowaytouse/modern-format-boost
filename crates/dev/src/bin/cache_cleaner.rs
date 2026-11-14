//! Modern Format Boost - Cache Cleaner in Rust.
//! Clears conversion/analysis caches scoped by target or full purges.

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use dev::infra::sqlite_backup;
use dev::infra::ui_tokens::pick_symbol;
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

// ANSI Colors
const RED: &str = "\x1b[0;31m";
const GREEN: &str = "\x1b[0;32m";
const YELLOW: &str = "\x1b[1;33m";
const BLUE: &str = "\x1b[0;34m";
const CYAN: &str = "\x1b[0;36m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

const PG_DEFAULT_CONNSTR: &str = "host=localhost dbname=modern_format_boost";

const PG_ANALYSIS_CACHE_TABLES: &[&str] = &[
    "analysis_records",
    "quality_records",
    "video_records",
    "path_index",
    "path_tree_snapshots",
    "cache_metadata",
];

const PG_INFERENCE_LOG_TABLES: &[&str] = &[
    "loop_intent_inference_log",
    "image_quality_inference_log",
    "animated_image_quality_inference_log",
    "video_quality_inference_log",
];

const PG_TRAINING_PROTECTED_TABLES: &[&str] = &[
    "loop_samples",
    "image_quality_samples",
    "animated_image_quality_samples",
    "video_quality_samples",
    "multi_scenario_metadata",
];

const ANIMATION_CACHE_EXTENSIONS: &[&str] =
    &["gif", "webp", "png", "apng", "avif", "heic", "heif", "jxl"];

#[derive(Parser, Debug)]
#[command(
    name = "cache_cleaner",
    version,
    about = "Modern Format Boost Cache Cleaner",
    group(clap::ArgGroup::new("inspection").args(["stats", "history_stats"])),
    group(clap::ArgGroup::new("history_operation").args(["history_stats", "history_backup", "history_cleanup_preview", "history_cleanup", "history_prepare_readonly"])),
    group(clap::ArgGroup::new("core_recovery").args(["preview_core_recovery", "export_core_recovery"])
        .conflicts_with_all(["migrate_core_state", "local_backup", "history_operation", "stats", "pruning_action", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "config", "no_config"])),
    group(clap::ArgGroup::new("pruning_action").args(["prune", "preview_prune"]))
)]
struct Args {
    #[arg(long, requires_all = ["history_directory", "json"], conflicts_with_all = ["local_backup", "stats", "prune", "preview_prune", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "yes", "config", "no_config", "cache_max_bytes", "cache_ttl_seconds"], help = "Prepare existing History WAL access files for read-only readers without creating a database, migrating its schema or changing history rows")]
    history_prepare_readonly: bool,

    #[arg(long, requires_all = ["history_directory", "json"], conflicts_with_all = ["local_backup", "stats", "prune", "preview_prune", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "yes", "config", "no_config"], help = "Preview exact selected History runs and proven log artifacts using a versioned JSON request; no history rows are changed")]
    history_cleanup_preview: Option<String>,

    #[arg(long, requires_all = ["history_directory", "json", "yes"], conflicts_with_all = ["local_backup", "stats", "prune", "preview_prune", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "config", "no_config"], help = "Clean only the previewed History run identities and snapshot token; media, recovery state and unproven external logs are retained")]
    history_cleanup: Option<String>,
    #[arg(long, requires = "history_directory", conflicts_with_all = ["local_backup", "stats", "history_stats", "prune", "preview_prune", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "yes", "config", "no_config", "cache_max_bytes", "cache_ttl_seconds"], help = "Create a verified snapshot of the explicitly selected History SQLite database at a new local destination; no source creation or migration")]
    history_backup: Option<PathBuf>,

    #[arg(
        long,
        requires = "history_operation",
        help = "Existing absolute history log directory to inspect or back up; no implicit source or fallback"
    )]
    history_directory: Option<PathBuf>,

    #[arg(long, requires = "yes", conflicts_with_all = ["local_backup", "history_operation", "stats", "prune", "preview_prune", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "config", "no_config"], help = "Move the existing Core database to the state directory with a verified rollback snapshot; requires --yes")]
    migrate_core_state: bool,

    #[arg(
        long,
        conflicts_with = "yes",
        help = "Inspect the verified pre-migration Core snapshot when the active database is missing"
    )]
    preview_core_recovery: bool,

    #[arg(long, value_name = "NEW_DIRECTORY", requires_all = ["expected_snapshot_blake3", "yes"], help = "Export the verified historical Core snapshot to a new inspection directory; requires explicit digest and --yes")]
    export_core_recovery: Option<PathBuf>,

    #[arg(
        long,
        requires = "export_core_recovery",
        value_name = "DIGEST",
        help = "Expected BLAKE3 from the Core recovery preview"
    )]
    expected_snapshot_blake3: Option<String>,

    #[arg(long, conflicts_with_all = ["stats", "history_stats", "prune", "preview_prune", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "yes", "config", "no_config"], help = "Create a verified snapshot of Core SQLite at a new local destination; dedicated cache, History and PostgreSQL are excluded")]
    local_backup: Option<PathBuf>,

    #[arg(long, value_name = "SNAPSHOT", requires = "expected_backup_blake3",
        conflicts_with_all = ["local_backup", "core_recovery", "migrate_core_state",
            "history_operation", "history_directory", "stats", "pruning_action", "check_integrity",
            "path", "postgres", "purge_animation_cache", "purge_session_state", "yes"],
        help = "Verify the exact bytes and Core contents of a standalone backup wherever it was kept")]
    check_core_backup: Option<PathBuf>,

    #[arg(
        long,
        value_name = "DIGEST",
        requires = "check_core_backup",
        help = "Expected BLAKE3 from the original Core backup receipt"
    )]
    expected_backup_blake3: Option<String>,

    #[arg(long, conflicts_with_all = ["stats", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state"], help = "Remove only expired or over-budget directory snapshots, retaining useful cache and all formal state")]
    prune: bool,

    #[arg(long, conflicts_with_all = ["stats", "history_stats", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "yes"], help = "Preview exact directory-cache expiry and eviction at one read-only snapshot; no rows or timestamps are changed")]
    preview_prune: bool,

    #[arg(long, requires = "history_directory", conflicts_with_all = ["prune", "preview_prune", "path", "postgres", "purge_animation_cache", "purge_session_state", "yes", "config", "no_config", "cache_max_bytes", "cache_ttl_seconds"], help = "Inspect History schema, storage row counts and logical UTF-8 bytes without loading logs, migrating or creating a database; add --check-integrity for structural checks only")]
    history_stats: bool,

    #[arg(
        long,
        requires = "pruning_action",
        conflicts_with = "no_config",
        help = "Overlay an explicit JSON configuration for cache pruning"
    )]
    config: Option<PathBuf>,

    #[arg(
        long,
        requires = "pruning_action",
        help = "Ignore configuration files when pruning"
    )]
    no_config: bool,

    #[command(flatten)]
    cache: foundation::runtime_config::cache_args::CacheArgs,

    #[arg(long, conflicts_with_all = ["path", "postgres", "purge_animation_cache", "purge_session_state"], help = "Inspect local cache without deleting or rebuilding anything")]
    stats: bool,

    #[arg(long, requires = "inspection", conflicts_with_all = ["yes", "path", "postgres", "purge_animation_cache", "purge_session_state"], help = "Check Core SQLite and payload hashes, or explicitly selected History SQLite structure/relationships/types; never repair or delete data")]
    check_integrity: bool,

    #[arg(long, conflicts_with_all = ["postgres", "purge_animation_cache", "purge_session_state"], help = "Return versioned local inspection results or a backup receipt as JSON")]
    json: bool,

    #[arg(long, conflicts_with_all = ["purge_animation_cache", "purge_session_state"], help = "Also purge advanced PostgreSQL analysis and inference caches (requires PostgreSQL)")]
    postgres: bool,

    #[arg(
        long = "purge-animation-cache",
        conflicts_with = "path",
        help = "Remove cache rows for animation-capable image formats"
    )]
    purge_animation_cache: bool,

    #[arg(
        long = "purge-session-state", conflicts_with_all = ["path", "purge_animation_cache"],
        help = "Legacy bulk cleanup flag; use Processing History for exact session cleanup"
    )]
    purge_session_state: bool,

    #[arg(help = "Target file or directory for cache-only cleanup; resume records are retained")]
    path: Option<String>,

    #[arg(long = "yes", short = 'y', help = "Skip interactive confirmation")]
    yes: bool,
}

fn get_mfb_state_root() -> Result<PathBuf> {
    foundation::process_lock::get_mfb_root().map_err(|e| anyhow!("Failed to resolve MFB root: {e}"))
}

fn backup_cache_directory() -> Result<PathBuf> {
    Ok(foundation::process_lock::inspect_mfb_root_path()?.join("cache"))
}

fn pg_connstr() -> String {
    match std::env::var("MFB_PG_CONNSTR") {
        Ok(val) => {
            if val.trim().is_empty() {
                PG_DEFAULT_CONNSTR.to_string()
            } else {
                val
            }
        }
        Err(_err) => PG_DEFAULT_CONNSTR.to_string(),
    }
}

fn check_postgres_reachable() -> Result<()> {
    let conn_str = pg_connstr();
    let status = Command::new("psql")
        .arg("-X")
        .arg(&conn_str)
        .arg("-c")
        .arg("SELECT 1;")
        .output();

    match status {
        Ok(output) => {
            if !output.status.success() {
                let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
                return Err(anyhow!(
                    "PostgreSQL unreachable via psql: {}",
                    err_msg.trim()
                ));
            }
            Ok(())
        }
        Err(e) => Err(anyhow!("psql CLI not found or failed to start: {e}")),
    }
}

fn run_pg_query(query: &str) -> Result<String> {
    let conn_str = pg_connstr();
    let output = Command::new("psql")
        .arg("-X")
        .arg(&conn_str)
        .arg("-c")
        .arg(query)
        .output()
        .with_context(|| "Failed to run PostgreSQL query via psql".to_string())?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(anyhow!("PostgreSQL query failed: {}", err_msg.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn purge_postgres_full() -> Result<()> {
    let mut tables = Vec::new();
    tables.extend_from_slice(PG_ANALYSIS_CACHE_TABLES);
    tables.extend_from_slice(PG_INFERENCE_LOG_TABLES);
    ensure_no_training_tables(&tables)?;

    let query = format!("TRUNCATE TABLE {} RESTART IDENTITY;", tables.join(", "));
    run_pg_query(&query)?;

    println!(
        "   {GREEN} PostgreSQL: analysis + inference-log caches truncated (training tables \
         untouched)"
    );
    Ok(())
}

fn ensure_no_training_tables(tables: &[&str]) -> Result<()> {
    for table in tables {
        if PG_TRAINING_PROTECTED_TABLES.contains(table) {
            return Err(anyhow!(
                "refusing to truncate protected training table: {table}"
            ));
        }
    }
    Ok(())
}

fn postgres_path_literal(path: &str) -> String {
    // E literals preserve filesystem backslashes regardless of server string settings.
    format!("E'{}'", path.replace('\\', "\\\\").replace('\'', "''"))
}

fn purge_postgres_for_path(target_path: &Path) -> Result<u64> {
    let absolute = target_path.canonicalize()?;
    let target_abs = absolute
        .to_str()
        .context("PostgreSQL cache target is not valid UTF-8")?;
    let absolute_literal = postgres_path_literal(target_abs);
    let mut total = 0u64;

    if target_path.is_dir() {
        let prefix = postgres_path_literal(&format!("{}/", target_abs.trim_end_matches('/')));

        let q1 = format!(
            "DELETE FROM path_index WHERE file_path = {absolute_literal} OR \
             left(file_path, length({prefix})) = {prefix};"
        );
        let out1 = run_pg_query(&q1)?;
        total = total
            .checked_add(parse_row_count(&out1)?)
            .context("PostgreSQL purge count overflow")?;

        for table in &["analysis_records", "quality_records", "video_records"] {
            let q = format!(
                "DELETE FROM {table} WHERE content_hash NOT IN (SELECT content_hash FROM \
                 path_index);"
            );
            let out = run_pg_query(&q)?;
            total = total
                .checked_add(parse_row_count(&out)?)
                .context("PostgreSQL purge count overflow")?;
        }
    } else {
        let q1 = format!("DELETE FROM path_index WHERE file_path = {absolute_literal};");
        let out1 = run_pg_query(&q1)?;
        total = total
            .checked_add(parse_row_count(&out1)?)
            .context("PostgreSQL purge count overflow")?;

        for table in &["analysis_records", "quality_records", "video_records"] {
            let q = format!(
                "DELETE FROM {table} WHERE content_hash NOT IN (SELECT content_hash FROM \
                 path_index);"
            );
            let out = run_pg_query(&q)?;
            total = total
                .checked_add(parse_row_count(&out)?)
                .context("PostgreSQL purge count overflow")?;
        }
    }

    let prefix = postgres_path_literal(&format!("{}/", target_abs.trim_end_matches('/')));
    total = total.checked_add(parse_row_count(&run_pg_query(&format!(
        "DELETE FROM path_tree_snapshots WHERE root_path = {absolute_literal} OR left(root_path, length({prefix})) = {prefix};"
    ))?)?).context("PostgreSQL purge count overflow")?;

    if total > 0 {
        println!(
            "   {} PostgreSQL: removed {} rows for {}",
            GREEN,
            total,
            target_path
                .file_name()
                .unwrap_or(std::ffi::OsStr::new(""))
                .to_string_lossy()
        );
    }
    Ok(total)
}

fn purge_postgres_inference_logs_for_path(target_path: &Path) -> Result<u64> {
    let absolute = target_path.canonicalize()?;
    let target_abs = absolute
        .to_str()
        .context("PostgreSQL inference target is not valid UTF-8")?;
    let absolute_literal = postgres_path_literal(target_abs);
    let mut total = 0u64;

    for table in PG_INFERENCE_LOG_TABLES {
        let q = if target_path.is_dir() {
            let prefix = postgres_path_literal(&format!("{}/", target_abs.trim_end_matches('/')));
            format!(
                "DELETE FROM {table} WHERE source_path = {absolute_literal} OR \
                 left(source_path, length({prefix})) = {prefix};"
            )
        } else {
            format!("DELETE FROM {table} WHERE source_path = {absolute_literal};")
        };
        let out = run_pg_query(&q)?;
        total = total
            .checked_add(parse_row_count(&out)?)
            .context("PostgreSQL inference purge count overflow")?;
    }

    if total > 0 {
        println!(
            "   {} PostgreSQL inference-log: removed {} row(s) for {}",
            GREEN,
            total,
            target_path
                .file_name()
                .unwrap_or(std::ffi::OsStr::new(""))
                .to_string_lossy()
        );
    }
    Ok(total)
}

fn purge_postgres_animation_cache() -> Result<u64> {
    let mut array_elems = Vec::new();
    for ext in ANIMATION_CACHE_EXTENSIONS {
        array_elems.push(format!("'%.{ext}'"));
    }
    let array_str = format!("ARRAY[{}]", array_elems.join(", "));

    // We can run these deletes in sequence
    let q_temp = format!(
        "CREATE TEMP TABLE mfb_animation_cache_purge AS SELECT DISTINCT content_hash FROM \
         path_index WHERE lower(file_path) LIKE ANY({array_str});"
    );

    let mut full_query = q_temp;
    full_query.push_str(
        "DELETE FROM analysis_records WHERE content_hash IN (SELECT content_hash FROM \
         mfb_animation_cache_purge);",
    );
    full_query.push_str(
        "DELETE FROM quality_records WHERE content_hash IN (SELECT content_hash FROM \
         mfb_animation_cache_purge);",
    );
    full_query.push_str(
        "DELETE FROM video_records WHERE content_hash IN (SELECT content_hash FROM \
         mfb_animation_cache_purge);",
    );
    full_query.push_str(
        "DELETE FROM path_index WHERE content_hash IN (SELECT content_hash FROM \
         mfb_animation_cache_purge);",
    );

    for table in PG_INFERENCE_LOG_TABLES {
        full_query.push_str(&format!(
            "DELETE FROM {table} WHERE lower(source_path) LIKE ANY({array_str});"
        ));
    }

    let out = run_pg_query(&full_query)?;
    let total = parse_row_count(&out)?;

    println!("   {GREEN} PostgreSQL: purged animation-capable caches (records & inference logs)");
    Ok(total)
}

fn parse_row_count(stdout: &str) -> Result<u64> {
    let mut count = 0u64;
    let mut received = false;
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let value = match parts.first().copied() {
            Some("DELETE") => Some(parts.get(1).context("missing DELETE count")?),
            Some("INSERT") => Some(parts.get(2).context("missing INSERT count")?),
            Some("TRUNCATE") => {
                received = true;
                None
            }
            _ => None,
        };
        if let Some(value) = value {
            received = true;
            count = count
                .checked_add(
                    value
                        .parse::<u64>()
                        .context("invalid PostgreSQL row count")?,
                )
                .context("PostgreSQL row count overflow")?;
        }
    }
    anyhow::ensure!(
        received,
        "PostgreSQL did not return a mutation-count receipt"
    );
    Ok(count)
}

#[derive(Debug, Serialize)]
struct CacheNamespace {
    name: String,
    rows: u64,
    /// Explicit evidence payload BLOB bytes, excluding keys, relational fields,
    /// indexes, free pages, and other physical SQLite storage overhead.
    payload_bytes: u64,
    rebuildable: bool,
}

#[derive(Debug, Serialize)]
struct CacheStatus {
    schema_version: u8,
    cache_directory: String,
    core_store_path: Option<String>,
    store_bytes: u64,
    namespaces: Vec<CacheNamespace>,
    legacy_analysis_bytes: u64,
    legacy_analysis_files: u64,
    removed_rows: u64,
    removed_files: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    integrity: Option<CacheIntegrity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pruning: Option<CachePruning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<DatabaseOverview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rebuildable_database: Option<DatabaseOverview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inactive_legacy_cache: Option<LegacyCacheCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pruning_preview: Option<foundation::mfb_sqlite_store::CachePrunePreview>,
}

#[derive(Debug, Default, Serialize)]
struct LegacyCacheCounts {
    rows: u64,
    payload_bytes: u64,
}

#[derive(Debug, Serialize)]
struct DatabaseOverview {
    store_schema_version: i32,
    sqlite_version: String,
    journal_mode: String,
    page_size: u64,
    page_count: u64,
    free_pages: u64,
}

#[derive(Debug, Serialize)]
struct BackupReceipt {
    schema_version: u8,
    source: String,
    destination: String,
    store_schema_version: i32,
    bytes: u64,
    snapshot_blake3: String,
}

#[derive(Debug, Serialize)]
struct CoreBackupCheck {
    schema_version: u8,
    snapshot: PathBuf,
    store_schema_version: i32,
    bytes: u64,
    snapshot_blake3: String,
}

#[derive(Debug, Serialize)]
struct CachePruning {
    policy: foundation::runtime_config::CachePolicy,
    #[serde(flatten)]
    removed: foundation::mfb_sqlite_store::CachePruneResult,
}

#[derive(Debug, Serialize)]
struct IntegrityIssue {
    code: &'static str,
    message: &'static str,
}

#[derive(Debug, Serialize)]
struct CacheIntegrity {
    schema_version: u8,
    status: &'static str,
    sqlite_integrity: &'static str,
    foreign_keys: &'static str,
    blob_scan_complete: bool,
    fast_img_scan_complete: bool,
    checked_cache_rows: u64,
    checked_protected_rows: u64,
    checked_fast_img_rows: u64,
    corrupt_cache_rows: u64,
    corrupt_protected_rows: u64,
    issue_count: u64,
    issues: Vec<IntegrityIssue>,
}

impl CacheIntegrity {
    fn new() -> Self {
        Self {
            schema_version: 1,
            status: "absent",
            sqlite_integrity: "not_checked",
            foreign_keys: "not_checked",
            blob_scan_complete: false,
            fast_img_scan_complete: false,
            checked_cache_rows: 0,
            checked_protected_rows: 0,
            checked_fast_img_rows: 0,
            corrupt_cache_rows: 0,
            corrupt_protected_rows: 0,
            issue_count: 0,
            issues: Vec::new(),
        }
    }

    fn issue(&mut self, code: &'static str, message: &'static str) {
        self.issue_count += 1;
        if self.issues.len() < 20 {
            self.issues.push(IntegrityIssue { code, message });
        }
    }
}

fn ensure_cache_directory(cache_dir: &Path) -> Result<()> {
    match fs::symlink_metadata(cache_dir) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(anyhow!(
            "cache directory is not a regular directory: {}",
            cache_dir.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", cache_dir.display())),
    }
}

fn regular_file_size(path: &Path) -> Result<Option<u64>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                anyhow::ensure!(
                    metadata.nlink() == 1,
                    "managed cache file has multiple hard links; file retained: {}",
                    path.display()
                );
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(anyhow!(
            "refusing non-regular cache file: {}",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn open_cache_store(store: &Path, read_only: bool) -> Result<Connection> {
    let conn = open_cache_connection(store, read_only)?;
    validate_cache_schema(&conn)?;
    Ok(conn)
}

fn open_cache_connection(store: &Path, read_only: bool) -> Result<Connection> {
    let directory = store
        .parent()
        .context("cache database has no parent")?
        .canonicalize()?;
    let store = directory.join(
        store
            .file_name()
            .context("cache database has no filename")?,
    );
    let mode = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let conn = Connection::open_with_flags(store, mode | OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    if read_only {
        conn.pragma_update(None, "query_only", true)?;
    }
    Ok(conn)
}

fn validate_cache_schema(conn: &Connection) -> Result<()> {
    let version = cache_store_schema_version(conn)?;
    anyhow::ensure!(
        matches!(
            version,
            2..=foundation::mfb_sqlite_store::STORE_SCHEMA_VERSION
        ),
        "unsupported cache store schema {version}; database retained"
    );
    // Reading or backing up older BLAKE3 stores preserves their actual schema
    // and evidence coverage. CRC32 v1 is outside this reader's payload format.
    conn.prepare("SELECT namespace, cache_key, schema_version, root_path, payload, payload_blake3, updated_at FROM blob_store LIMIT 0")?;
    conn.prepare("INSERT INTO blob_store(namespace,cache_key) VALUES (?1,?2) ON CONFLICT(namespace,cache_key) DO NOTHING")?;
    Ok(())
}

fn cache_store_schema_version(conn: &Connection) -> Result<i32> {
    let version: i32 = conn.query_row(
        "SELECT value FROM store_metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    Ok(version)
}

fn database_overview(conn: &Connection, store_schema_version: i32) -> Result<DatabaseOverview> {
    let overview = DatabaseOverview {
        store_schema_version,
        sqlite_version: conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?,
        journal_mode: conn.query_row("PRAGMA main.journal_mode", [], |row| row.get(0))?,
        page_size: conn.query_row("PRAGMA main.page_size", [], |row| {
            nonnegative_column(row, 0)
        })?,
        page_count: conn.query_row("PRAGMA main.page_count", [], |row| {
            nonnegative_column(row, 0)
        })?,
        free_pages: conn.query_row("PRAGMA main.freelist_count", [], |row| {
            nonnegative_column(row, 0)
        })?,
    };
    anyhow::ensure!(
        !overview.sqlite_version.is_empty()
            && matches!(
                overview.journal_mode.as_str(),
                "delete" | "truncate" | "persist" | "memory" | "wal" | "off"
            )
            && (512..=65_536).contains(&overview.page_size)
            && overview.page_size.is_power_of_two()
            && overview.page_count > 0
            && overview.free_pages <= overview.page_count,
        "invalid SQLite database overview; database retained"
    );
    Ok(overview)
}

fn inspect_store_integrity(
    conn: &Connection,
    integrity: &mut CacheIntegrity,
    phase: &mut &'static str,
    check_legacy_cache: bool,
    cache_only: bool,
) -> Result<()> {
    *phase = "sqlite_integrity";
    let mut statement = conn.prepare("PRAGMA integrity_check")?;
    let mut rows = statement.query([])?;
    if integrity.sqlite_integrity == "not_checked" {
        integrity.sqlite_integrity = "ok";
    }
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(0)? != "ok" {
            integrity.sqlite_integrity = "failed";
            integrity.issue(
                "sqlite_integrity",
                "SQLite reported a structural integrity failure.",
            );
        }
    }
    *phase = "foreign_keys";
    let mut statement = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    if integrity.foreign_keys == "not_checked" {
        integrity.foreign_keys = "ok";
    }
    while rows.next()?.is_some() {
        integrity.foreign_keys = "failed";
        integrity.issue("foreign_key", "SQLite reported a foreign key violation.");
    }
    *phase = "blob_hashes";
    let mut statement =
        conn.prepare("SELECT namespace IN (?1, ?2), payload, payload_blake3 FROM blob_store")?;
    let mut rows = statement.query([
        foundation::mfb_sqlite_store::NS_PATH_TREE,
        foundation::mfb_sqlite_store::NS_GATE1_PROOF,
    ])?;
    while let Some(row) = rows.next()? {
        let rebuildable = row.get::<_, Option<bool>>(0)?.unwrap_or(false);
        if rebuildable && !check_legacy_cache {
            continue;
        }
        if rebuildable {
            integrity.checked_cache_rows += 1;
        } else {
            integrity.checked_protected_rows += 1;
        }
        let payload = row.get_ref(1)?;
        let expected = row.get_ref(2)?;
        let valid = match (payload.as_blob(), expected.as_blob()) {
            (Ok(payload), Ok(expected)) => blake3::hash(payload).as_bytes() == expected,
            _ => false,
        };
        if !valid {
            if rebuildable {
                integrity.corrupt_cache_rows += 1;
                integrity.issue(
                    "cache_payload",
                    "A rebuildable cache payload has an invalid BLAKE3 digest or storage type.",
                );
            } else {
                integrity.corrupt_protected_rows += 1;
                integrity.issue(
                    "protected_payload",
                    "A protected state payload has an invalid BLAKE3 digest or storage type.",
                );
            }
        }
    }
    integrity.blob_scan_complete = true;
    if !cache_only {
        *phase = "fast_img_state";
        let fast_img_rows = u64::try_from(foundation::fast_img_store::inspect_integrity(conn)?)
            .context("Fast IMG protected row count exceeds u64")?;
        integrity.checked_protected_rows = integrity
            .checked_protected_rows
            .checked_add(fast_img_rows)
            .context("protected state row count overflow")?;
        integrity.checked_fast_img_rows = fast_img_rows;
        integrity.fast_img_scan_complete = true;
    }
    integrity.status = if integrity.issue_count == 0 {
        "healthy"
    } else {
        "unhealthy"
    };
    Ok(())
}

fn require_healthy_store(conn: &mut Connection, check_legacy_cache: bool) -> Result<()> {
    let snapshot = conn.transaction()?;
    validate_cache_schema(&snapshot)?;
    database_overview(&snapshot, cache_store_schema_version(&snapshot)?)?;
    let mut integrity = CacheIntegrity::new();
    inspect_store_integrity(
        &snapshot,
        &mut integrity,
        &mut "sqlite_integrity",
        check_legacy_cache,
        false,
    )?;
    anyhow::ensure!(
        integrity.status == "healthy"
            && integrity.blob_scan_complete
            && integrity.fast_img_scan_complete,
        "Core database integrity verification failed"
    );
    snapshot.commit()?;
    Ok(())
}

fn core_store_for_cache(cache_dir: &Path) -> Result<PathBuf> {
    let root = cache_dir
        .parent()
        .context("cache directory has no state root")?;
    foundation::mfb_sqlite_store::formal_store_path_at(root)
}

fn managed_database_bytes(path: &Path) -> Result<u64> {
    ["", "-wal", "-shm", "-journal"]
        .into_iter()
        .try_fold(0u64, |total, suffix| {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            total
                .checked_add(regular_file_size(Path::new(&sidecar))?.unwrap_or(0))
                .context("managed SQLite size overflow")
        })
}

fn require_outside_core_storage(cache_dir: &Path, destination: &Path) -> Result<()> {
    for managed in [cache_dir.to_path_buf(), cache_dir.with_file_name("state")] {
        if managed.try_exists()? {
            anyhow::ensure!(
                !destination.starts_with(managed.canonicalize()?),
                "SQLite backup destination must be outside managed cache and state directories"
            );
        }
    }
    Ok(())
}

fn backup_local_store(cache_dir: &Path, destination: &Path) -> Result<BackupReceipt> {
    ensure_cache_directory(cache_dir)?;
    let source = core_store_for_cache(cache_dir)?;
    managed_database_bytes(&source)?;
    anyhow::ensure!(
        regular_file_size(&source)?.is_some(),
        "Core database is absent; no backup created"
    );
    let legacy_core = source.parent() == Some(cache_dir);
    let canonical_source = source.canonicalize()?;
    let destination = sqlite_backup::resolve_destination(&canonical_source, destination)?;
    require_outside_core_storage(cache_dir, &destination)?;
    let parent = destination
        .parent()
        .context("backup destination has no parent")?;
    let check_legacy_cache = legacy_core
        && regular_file_size(&cache_dir.join(foundation::mfb_sqlite_store::CACHE_STORE_FILE_NAME))?
            .is_none();
    let mut conn = open_cache_store(&source, true)?;
    let root = cache_dir
        .parent()
        .context("cache directory has no state root")?;
    foundation::mfb_sqlite_store::verify_formal_store_connection(root, &source, &conn)?;
    require_healthy_store(&mut conn, check_legacy_cache)?;
    let store_schema_version = cache_store_schema_version(&conn)?;
    let staged = tempfile::Builder::new()
        .prefix(".mfb-core-backup-")
        .tempdir_in(parent)?;
    let backup = staged.path().join("mfb_store.sqlite");
    let mut staged_db = Connection::open(&backup)?;
    staged_db.pragma_update(None, "synchronous", "FULL")?;
    foundation::mfb_sqlite_store::copy_sqlite_snapshot(&conn, &mut staged_db)?;
    drop(staged_db);
    foundation::mfb_sqlite_store::verify_formal_store_connection(root, &source, &conn)?;
    drop(conn);
    if !check_legacy_cache {
        let staged = open_cache_store(&backup, false)?;
        staged.execute(
            "DELETE FROM blob_store WHERE namespace IN (?1, ?2)",
            [
                foundation::mfb_sqlite_store::NS_PATH_TREE,
                foundation::mfb_sqlite_store::NS_GATE1_PROOF,
            ],
        )?;
    }
    let mut verified = open_cache_store(&backup, true)?;
    require_healthy_store(&mut verified, true)?;
    anyhow::ensure!(
        cache_store_schema_version(&verified)? == store_schema_version,
        "Core database backup schema differs from its source; backup not published"
    );
    drop(verified);
    let bytes = sqlite_backup::synchronize_private_snapshot(&backup)?;
    sqlite_backup::require_snapshot_without_sidecars(&backup)?;
    let snapshot_blake3 = foundation::common_utils::calculate_blake3_hash(&backup)?;
    let receipt = BackupReceipt {
        schema_version: 1,
        source: canonical_source
            .to_str()
            .context("Core database path is not UTF-8")?
            .to_owned(),
        destination: destination
            .to_str()
            .context("backup destination path is not UTF-8")?
            .to_owned(),
        store_schema_version,
        bytes,
        snapshot_blake3,
    };
    sqlite_backup::publish_snapshot(&backup, &destination)?;
    Ok(receipt)
}

fn check_core_backup(snapshot: &Path, expected_blake3: &str) -> Result<CoreBackupCheck> {
    use std::io::Read as _;

    let expected_hash = blake3::Hash::from_hex(expected_blake3)
        .context("expected BLAKE3 must be a 64-character hexadecimal digest")?;
    let snapshot = std::path::absolute(snapshot)?;
    sqlite_backup::require_snapshot_without_sidecars(&snapshot)?;
    let bytes = regular_file_size(&snapshot)?.context("Core backup snapshot is missing")?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut source = options
        .open(&snapshot)
        .context("open Core backup snapshot")?;
    let before = source.metadata()?;
    anyhow::ensure!(
        before.is_file() && before.len() == bytes && bytes > 0,
        "Core backup snapshot changed before verification"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        anyhow::ensure!(
            before.nlink() == 1,
            "Core backup snapshot has multiple hard links"
        );
    }
    // SQLite opens only a private copy. Adjacent journals cannot affect this check.
    let mut staging = tempfile::Builder::new();
    staging.prefix("mfb-core-backup-check-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        staging.permissions(fs::Permissions::from_mode(0o700));
    }
    let directory = staging.tempdir()?;
    let private_copy = directory.path().join("snapshot.sqlite");
    let mut staged = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&private_copy)?;
    let copy_limit = bytes
        .checked_add(1)
        .context("Core backup snapshot length overflow")?;
    anyhow::ensure!(
        io::copy(&mut (&mut source).take(copy_limit), &mut staged)? == bytes,
        "Core backup snapshot length changed during verification"
    );
    drop(staged);
    let staged_hash = foundation::common_utils::calculate_blake3_hash(&private_copy)?;
    anyhow::ensure!(
        staged_hash == expected_hash.to_hex().as_str(),
        "Core backup snapshot BLAKE3 does not match the receipt"
    );

    let mut verified = open_cache_connection(&private_copy, true)?;
    verified.pragma_update(None, "trusted_schema", false)?;
    let has_core_tables: bool = verified.query_row(
        "SELECT count(*) = 2 FROM pragma_table_list
         WHERE schema = 'main' AND type = 'table'
           AND name IN ('store_metadata', 'blob_store')
           AND NOT EXISTS (SELECT 1 FROM sqlite_schema
                           WHERE name = 'cache_metadata' COLLATE NOCASE)",
        [],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        has_core_tables,
        "snapshot is not a recognized Core SQLite database"
    );
    let journal: String = verified.query_row("PRAGMA main.journal_mode", [], |row| row.get(0))?;
    anyhow::ensure!(
        journal == "delete",
        "Core backup snapshot is not in standalone SQLite journal mode"
    );
    require_healthy_store(&mut verified, true)?;
    let store_schema_version = cache_store_schema_version(&verified)?;

    sqlite_backup::require_snapshot_without_sidecars(&snapshot)?;
    let current = fs::symlink_metadata(&snapshot)?;
    anyhow::ensure!(
        current.is_file() && current.len() == bytes && current.modified()? == before.modified()?,
        "Core backup snapshot changed while verifying"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        anyhow::ensure!(
            current.dev() == before.dev()
                && current.ino() == before.ino()
                && current.nlink() == before.nlink()
                && current.ctime() == before.ctime()
                && current.ctime_nsec() == before.ctime_nsec(),
            "Core backup snapshot changed while verifying"
        );
    }
    Ok(CoreBackupCheck {
        schema_version: 1,
        snapshot,
        store_schema_version,
        bytes,
        snapshot_blake3: staged_hash,
    })
}

fn legacy_analysis_sqlite_paths(cache_dir: &Path) -> Vec<PathBuf> {
    ["image_analysis_v2.db", "image_analysis_v2_main.db"]
        .into_iter()
        .flat_map(|name| {
            ["", "-wal", "-shm", "-journal"].map(|suffix| cache_dir.join(format!("{name}{suffix}")))
        })
        .collect()
}

fn nonnegative_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn add_fast_img_state_namespace(
    conn: &Connection,
    namespaces: &mut Vec<CacheNamespace>,
    rows: u64,
) -> Result<()> {
    if rows == 0 {
        return Ok(());
    }
    // Count explicit evidence payloads. store_bytes is the physical store size.
    let version = cache_store_schema_version(conn)?;
    let payload_query = if version >= 5 {
        "SELECT COALESCE((SELECT SUM(length(header_json)) FROM fast_img_tasks),0)
              + COALESCE((SELECT SUM(length(source_json)) FROM fast_img_migrations),0)
              + COALESCE((SELECT SUM(length(directory_snapshot)) FROM fast_img_migrations),0)
              + COALESCE((SELECT SUM(length(CAST(requested_delivery AS BLOB))) FROM fast_img_retry_tasks),0)
              + COALESCE((SELECT SUM(length(CAST(source_claim AS BLOB))
                    + COALESCE(length(CAST(xmp_claim AS BLOB)),0)
                    + COALESCE(length(CAST(aae_claim AS BLOB)),0)) FROM fast_img_retry_sources),0)"
    } else if version >= 4 {
        "SELECT COALESCE((SELECT SUM(length(header_json)) FROM fast_img_tasks),0)
              + COALESCE((SELECT SUM(length(source_json)) FROM fast_img_migrations),0)
              + COALESCE((SELECT SUM(length(directory_snapshot)) FROM fast_img_migrations),0)"
    } else {
        "SELECT COALESCE((SELECT SUM(length(header_json)) FROM fast_img_tasks),0)
              + COALESCE((SELECT SUM(length(source_json)) FROM fast_img_migrations),0)"
    };
    let mut payload_bytes = conn.query_row(payload_query, [], |row| nonnegative_column(row, 0))?;
    for (table, required_since) in [
        ("fast_img_primary_cleanup", 6),
        ("fast_img_tier2_cleanup", 7),
    ] {
        if version < required_since {
            continue;
        }
        let cleanup_bytes = conn.query_row(
            &format!("SELECT COALESCE(SUM(length(CAST(entry_claim AS BLOB)) + length(evidence_blake3)),0)
                      FROM {table}"),
            [],
            |row| nonnegative_column(row, 0),
        )?;
        payload_bytes = payload_bytes
            .checked_add(cleanup_bytes)
            .context("Fast IMG namespace payload byte count overflow")?;
    }
    if version >= 8 {
        let retry_bytes = conn.query_row(
            "SELECT COALESCE((SELECT SUM(length(CAST(route AS BLOB))) FROM fast_img_retry_sources),0)
                  + COALESCE((SELECT SUM(length(CAST(source_blake3 AS BLOB)) + length(CAST(reason AS BLOB))) FROM fast_img_tier2_failures),0)",
            [],
            |row| nonnegative_column(row, 0),
        )?;
        payload_bytes = payload_bytes
            .checked_add(retry_bytes)
            .context("Fast IMG original-format retry payload byte count overflow")?;
    }
    for (table, required_since) in foundation::fast_img_store::EXECUTION_TABLES
        .into_iter()
        .chain([(
            "fast_img_primary_results",
            foundation::fast_img_store::FAST_IMG_PRIMARY_RESULT_SCHEMA_VERSION,
        )])
    {
        if version < required_since {
            continue;
        }
        let execution_bytes = conn.query_row(
            &format!(
                "SELECT COALESCE(SUM(length(payload) + length(payload_blake3)),0) FROM {table}"
            ),
            [],
            |row| nonnegative_column(row, 0),
        )?;
        payload_bytes = payload_bytes
            .checked_add(execution_bytes)
            .context("Fast IMG execution payload byte count overflow")?;
    }
    if version >= foundation::fast_img_store::FAST_IMG_ADMISSION_SCHEMA_VERSION {
        let admission_bytes = conn.query_row(
            "SELECT COALESCE(SUM(length(CAST(source_rel AS BLOB))
             + length(CAST(source_blake3 AS BLOB)) + length(CAST(route AS BLOB))),0)
             FROM fast_img_admission",
            [],
            |row| nonnegative_column(row, 0),
        )?;
        payload_bytes = payload_bytes
            .checked_add(admission_bytes)
            .context("Fast IMG admission payload byte count overflow")?;
    }
    for (extension, since) in [
        (
            "xmp",
            foundation::fast_img_store::FAST_IMG_ADMISSION_XMP_SCHEMA_VERSION,
        ),
        (
            "aae",
            foundation::fast_img_store::FAST_IMG_ADMISSION_AAE_SCHEMA_VERSION,
        ),
    ] {
        if version >= since {
            let sidecar_bytes = conn.query_row(
                &format!("SELECT COALESCE(SUM(length(CAST(source_rel AS BLOB)) + length(CAST(state AS BLOB))
                 + COALESCE(length(CAST({extension}_rel AS BLOB)),0)
                 + COALESCE(length(CAST({extension}_blake3 AS BLOB)),0)),0)
                 FROM fast_img_admission_{extension}"),
                [],
                |row| nonnegative_column(row, 0),
            )?;
            payload_bytes = payload_bytes
                .checked_add(sidecar_bytes)
                .context("Fast IMG sidecar admission payload byte count overflow")?;
        }
    }
    if let Some(existing) = namespaces
        .iter_mut()
        .find(|namespace| namespace.name == "fast_img_state")
    {
        // An older unknown protected blob namespace may already use this name;
        // include both sets of records without hiding either or duplicating the
        // namespace identifier consumed by the GUI.
        existing.rows = existing
            .rows
            .checked_add(rows)
            .context("Fast IMG namespace row count overflow")?;
        existing.payload_bytes = existing
            .payload_bytes
            .checked_add(payload_bytes)
            .context("Fast IMG namespace payload byte count overflow")?;
    } else {
        namespaces.push(CacheNamespace {
            name: "fast_img_state".into(),
            rows,
            payload_bytes,
            rebuildable: false,
        });
        namespaces.sort_by(|left, right| left.name.cmp(&right.name));
    }
    Ok(())
}

fn read_cache_namespaces(conn: &Connection) -> Result<Vec<CacheNamespace>> {
    use foundation::mfb_sqlite_store::{NS_GATE1_PROOF, NS_PATH_TREE};

    let mut statement = conn.prepare(
        "SELECT namespace, COUNT(*), SUM(length(payload)) FROM blob_store GROUP BY namespace ORDER BY namespace",
    )?;
    Ok(statement
        .query_map([], |row| {
            let name: String = row.get(0)?;
            Ok(CacheNamespace {
                rebuildable: matches!(name.as_str(), NS_PATH_TREE | NS_GATE1_PROOF),
                name,
                rows: nonnegative_column(row, 1)?,
                payload_bytes: nonnegative_column(row, 2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn cache_status(cache_dir: &Path) -> Result<CacheStatus> {
    cache_status_with_integrity(cache_dir, false)
}

fn cache_status_with_integrity(cache_dir: &Path, check_integrity: bool) -> Result<CacheStatus> {
    cache_status_with_options(cache_dir, check_integrity, None)
}

fn cache_status_with_options(
    cache_dir: &Path,
    check_integrity: bool,
    preview: Option<foundation::runtime_config::CachePolicy>,
) -> Result<CacheStatus> {
    let mut status = CacheStatus {
        schema_version: 1,
        cache_directory: cache_dir
            .to_str()
            .context("cache path is not valid UTF-8")?
            .to_owned(),
        core_store_path: None,
        store_bytes: 0,
        namespaces: Vec::new(),
        legacy_analysis_bytes: 0,
        legacy_analysis_files: 0,
        removed_rows: 0,
        removed_files: 0,
        integrity: check_integrity.then(CacheIntegrity::new),
        pruning: None,
        database: None,
        rebuildable_database: None,
        inactive_legacy_cache: None,
        pruning_preview: None,
    };
    let mut phase = "managed_paths";
    let inspection = (|| -> Result<()> {
        ensure_cache_directory(cache_dir)?;
        use foundation::mfb_sqlite_store::{CACHE_STORE_FILE_NAME, open_rebuildable_cache_at};

        let store = core_store_for_cache(cache_dir)?;
        let cache_path = cache_dir.join(CACHE_STORE_FILE_NAME);
        status.store_bytes = managed_database_bytes(&store)?
            .checked_add(managed_database_bytes(&cache_path)?)
            .context("cache store size overflow")?;
        phase = "rebuildable_cache";
        let mut cache = open_rebuildable_cache_at(&cache_path, true)?;
        let legacy_cache_is_active = cache.is_none() && store.parent() == Some(cache_dir);
        if regular_file_size(&store)?.is_some() {
            phase = "store_open";
            let mut conn = open_cache_connection(&store, true)?;
            let snapshot = conn.transaction()?;
            phase = "schema";
            let root = cache_dir
                .parent()
                .context("cache directory has no state root")?;
            foundation::mfb_sqlite_store::verify_formal_store_connection(root, &store, &snapshot)?;
            validate_cache_schema(&snapshot)?;
            phase = "database_overview";
            status.database = Some(database_overview(
                &snapshot,
                cache_store_schema_version(&snapshot)?,
            )?);
            status.core_store_path = Some(
                store
                    .to_str()
                    .context("Core database path is not UTF-8")?
                    .to_owned(),
            );
            if legacy_cache_is_active && let Some(policy) = preview {
                status.pruning_preview = Some(
                    foundation::mfb_sqlite_store::preview_path_tree_pruning(&snapshot, policy)?,
                );
            }
            let fast_img_rows = if let Some(integrity) = &mut status.integrity {
                inspect_store_integrity(
                    &snapshot,
                    integrity,
                    &mut phase,
                    legacy_cache_is_active,
                    false,
                )?;
                integrity.checked_fast_img_rows
            } else {
                phase = "fast_img_state";
                u64::try_from(foundation::fast_img_store::inspect_row_count(&snapshot)?)
                    .context("Fast IMG protected row count exceeds u64")?
            };
            phase = "namespace_statistics";
            for namespace in read_cache_namespaces(&snapshot)? {
                if !legacy_cache_is_active && namespace.rebuildable {
                    let legacy = status
                        .inactive_legacy_cache
                        .get_or_insert_with(LegacyCacheCounts::default);
                    legacy.rows = legacy
                        .rows
                        .checked_add(namespace.rows)
                        .context("legacy cache row count overflow")?;
                    legacy.payload_bytes = legacy
                        .payload_bytes
                        .checked_add(namespace.payload_bytes)
                        .context("legacy cache size overflow")?;
                } else {
                    status.namespaces.push(namespace);
                }
            }
            add_fast_img_state_namespace(&snapshot, &mut status.namespaces, fast_img_rows)?;
            snapshot.commit()?;
        }
        if let Some(cache) = &mut cache {
            phase = "rebuildable_cache";
            let snapshot = cache.transaction()?;
            foundation::mfb_sqlite_store::validate_rebuildable_cache_schema(&snapshot)?;
            status.rebuildable_database = Some(database_overview(
                &snapshot,
                foundation::mfb_sqlite_store::CACHE_STORE_SCHEMA_VERSION,
            )?);
            if let Some(policy) = preview {
                status.pruning_preview = Some(
                    foundation::mfb_sqlite_store::preview_path_tree_pruning(&snapshot, policy)?,
                );
            }
            if let Some(integrity) = &mut status.integrity {
                inspect_store_integrity(&snapshot, integrity, &mut phase, true, true)?;
            }
            status.namespaces.extend(read_cache_namespaces(&snapshot)?);
            status
                .namespaces
                .sort_unstable_by(|left, right| left.name.cmp(&right.name));
            snapshot.commit()?;
        }
        phase = "managed_paths";
        for path in legacy_analysis_sqlite_paths(cache_dir) {
            if let Some(bytes) = regular_file_size(&path)? {
                status.legacy_analysis_bytes = status
                    .legacy_analysis_bytes
                    .checked_add(bytes)
                    .context("legacy cache size overflow")?;
                status.legacy_analysis_files += 1;
            }
        }
        Ok(())
    })();
    if let Err(error) = inspection {
        if let Some(integrity) = &mut status.integrity {
            integrity.status = "error";
            integrity.issue(phase, match phase {
                "schema" => "The store schema is missing, unreadable, or unsupported; inspection could not complete.",
                "store_open" => "The store could not be opened read-only; inspection could not complete.",
                "rebuildable_cache" => "The dedicated cache could not be verified; its files were retained.",
                "managed_paths" => "A managed path is unreadable, has the wrong type, or is hard-linked. Inspection could not complete.",
                "fast_img_state" => "Protected Fast IMG state or migration evidence could not be verified; records were retained and the check is incomplete.",
                _ => "A database query failed during integrity inspection; the reported check is incomplete.",
            });
        } else {
            return Err(error);
        }
    }
    Ok(status)
}

fn remove_legacy_analysis_sqlite_files(cache_dir: &Path) -> Result<u64> {
    let mut removed = 0;
    for path in legacy_analysis_sqlite_paths(cache_dir) {
        if regular_file_size(&path)?.is_some() {
            fs::remove_file(&path)
                .with_context(|| format!("remove obsolete analysis cache {}", path.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn open_active_cache_for_maintenance(
    cache_dir: &Path,
    has_dedicated: bool,
) -> Result<Option<Connection>> {
    use foundation::mfb_sqlite_store::{CACHE_STORE_FILE_NAME, open_rebuildable_cache_at};

    if has_dedicated {
        return open_rebuildable_cache_at(&cache_dir.join(CACHE_STORE_FILE_NAME), false)?
            .map(Some)
            .context("dedicated cache disappeared before maintenance");
    }
    let store = core_store_for_cache(cache_dir)?;
    if store.parent() != Some(cache_dir) {
        return Ok(None);
    }
    anyhow::ensure!(
        regular_file_size(&cache_dir.join(CACHE_STORE_FILE_NAME))?.is_none(),
        "dedicated cache appeared after inspection; retry maintenance"
    );
    regular_file_size(&store)?
        .map(|_| open_cache_store(&store, false))
        .transpose()
}

fn purge_local_cache(cache_dir: &Path, target: Option<&Path>) -> Result<CacheStatus> {
    // Validate the entire managed scope before deleting; unknown namespaces/files are state, not cache.
    let before = cache_status(cache_dir)?;
    let mut removed_rows = 0;
    if let Some(mut conn) =
        open_active_cache_for_maintenance(cache_dir, before.rebuildable_database.is_some())?
    {
        conn.pragma_update(None, "synchronous", "FULL")?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if before.rebuildable_database.is_some() {
            foundation::mfb_sqlite_store::validate_rebuildable_cache_schema(&tx)?;
        } else {
            validate_cache_schema(&tx)?;
        }
        let rows = if let Some(target) = target {
            let absolute = target.canonicalize()?;
            let absolute = absolute
                .to_str()
                .context("target path is not valid UTF-8")?;
            let prefix = format!("{}/", absolute.trim_end_matches('/'));
            tx.execute(
                "DELETE FROM blob_store WHERE namespace IN (?1, ?2) AND (root_path = ?3 OR substr(root_path, 1, length(?4)) = ?4)",
                params![foundation::mfb_sqlite_store::NS_PATH_TREE,
                    foundation::mfb_sqlite_store::NS_GATE1_PROOF, absolute, prefix],
            )?
        } else {
            tx.execute(
                "DELETE FROM blob_store WHERE namespace IN (?1, ?2)",
                [
                    foundation::mfb_sqlite_store::NS_PATH_TREE,
                    foundation::mfb_sqlite_store::NS_GATE1_PROOF,
                ],
            )?
        };
        removed_rows = u64::try_from(rows).context("cache deletion count overflow")?;
        tx.commit()?;
    }
    let removed_files = if target.is_none() {
        remove_legacy_analysis_sqlite_files(cache_dir)?
    } else {
        0
    };
    let mut status = cache_status(cache_dir)?;
    status.removed_rows = removed_rows;
    status.removed_files = removed_files;
    Ok(status)
}

fn prune_local_cache(
    cache_dir: &Path,
    policy: foundation::runtime_config::CachePolicy,
) -> Result<CacheStatus> {
    policy.validate()?;
    let before = cache_status(cache_dir)?;
    let removed = if let Some(mut conn) =
        open_active_cache_for_maintenance(cache_dir, before.rebuildable_database.is_some())?
    {
        conn.pragma_update(None, "synchronous", "FULL")?;
        foundation::mfb_sqlite_store::prune_path_tree_cache(&mut conn, policy)?
    } else {
        foundation::mfb_sqlite_store::CachePruneResult::default()
    };
    let mut status = cache_status(cache_dir).with_context(|| format!(
        "cache pruning committed ({} expired/time-invalid, {} over-budget rows); post-prune inspection failed",
        removed.expired_rows, removed.evicted_rows
    ))?;
    status.removed_rows = removed
        .expired_rows
        .checked_add(removed.evicted_rows)
        .context("cache pruning count overflow")?;
    status.pruning = Some(CachePruning { policy, removed });
    Ok(status)
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut unit_index = 0usize;
    let mut scale = 1u64;
    while unit_index + 1 < UNITS.len() && bytes >= scale.saturating_mul(1024) {
        scale = scale.saturating_mul(1024);
        unit_index += 1;
    }
    if unit_index == 0 {
        return format!("{bytes} B");
    }
    let mut whole = bytes / scale;
    let mut tenth = ((bytes % scale).saturating_mul(10) + (scale / 2)) / scale;
    if tenth == 10 {
        whole = whole.saturating_add(1);
        tenth = 0;
    }
    format!("{whole}.{tenth} {}", UNITS[unit_index])
}

fn draw_header(targeted: bool) {
    let line = "─".repeat(60);
    println!("{BLUE}╭{line}╮{RESET}");
    let mode_text = if targeted {
        format!("{} TARGETED CACHE CLEANUP", pick_symbol("🧹", "[SWEEP]"))
    } else {
        format!("{} CACHE CLEANUP", pick_symbol("🧹", "[SWEEP]"))
    };
    println!(
        "{}  {:<62} {}",
        BLUE,
        format!("{}{}{}", BOLD, RED, mode_text),
        BLUE
    );
    println!("{BLUE}╰{line}╯{RESET}");
    if !targeted {
        println!(
            "   {GREEN} History, resume records, verification state and models are retained.{RESET}\n"
        );
    }
}

fn perform_animation_cache_cleanup(yes: bool) -> Result<()> {
    check_postgres_reachable()?;
    if !confirm_cleanup(
        yes,
        "Clear advanced animation cache? History and resume records are retained.",
    )? {
        anyhow::bail!("animation cache cleanup cancelled; no action taken");
    }
    draw_header(true);
    println!("   {BOLD}Target:{RESET} {DIM}animation-capable cache entries{RESET}");
    println!("   {YELLOW}Purging cached static/unknown verdicts and routing snapshots...{RESET}\n");

    purge_postgres_animation_cache()?;
    run_pg_query("DELETE FROM path_tree_snapshots;")?;
    purge_local_cache(&get_mfb_state_root()?.join("cache"), None)?;
    println!("\n{GREEN} Animation Cache Cleanup Complete\n");
    Ok(())
}

fn sys_stdin_stdout_isatty() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn confirm_cleanup(yes: bool, prompt: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    anyhow::ensure!(
        sys_stdin_stdout_isatty(),
        "cleanup requires --yes in non-interactive mode; use --stats to inspect only"
    );
    eprintln!("{YELLOW}{prompt}{RESET}");
    eprint!("   {CYAN}Type 'yes' to proceed: {RESET}");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "yes" | "y"))
}

fn print_cache_status(status: &CacheStatus, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(status)?);
    } else {
        println!("Cache: {}", status.cache_directory);
        if let Some(path) = &status.core_store_path {
            println!("Core database: {path}");
        }
        println!(
            "SQLite on disk (includes retained state): {}",
            format_size(status.store_bytes)
        );
        if let Some(database) = &status.database {
            println!(
                "Core SQLite: schema {}, SQLite {}, journal {}; {} pages of {} bytes ({} free)",
                database.store_schema_version,
                database.sqlite_version,
                database.journal_mode,
                database.page_count,
                database.page_size,
                database.free_pages
            );
        }
        if let Some(cache) = &status.rebuildable_database {
            println!(
                "Dedicated rebuildable cache: schema {}, SQLite {}, journal {}",
                cache.store_schema_version, cache.sqlite_version, cache.journal_mode
            );
        }
        if let Some(legacy) = &status.inactive_legacy_cache {
            println!(
                "Retained inactive cache in Core SQLite: {} rows, {} payload",
                legacy.rows,
                format_size(legacy.payload_bytes)
            );
        }
        for namespace in &status.namespaces {
            println!(
                "  {}: {} rows, {} payload [{}]",
                namespace.name,
                namespace.rows,
                format_size(namespace.payload_bytes),
                if namespace.rebuildable {
                    "rebuildable cache"
                } else {
                    "retained state"
                }
            );
        }
        println!(
            "Obsolete analysis cache: {} files, {}",
            status.legacy_analysis_files,
            format_size(status.legacy_analysis_bytes)
        );
        println!(
            "Removed: {} cache rows, {} obsolete cache files",
            status.removed_rows, status.removed_files
        );
        if let Some(pruning) = &status.pruning {
            println!(
                "Pruned: {} expired/time-invalid rows, {} over-budget rows; policy: {} payload bytes, {} idle seconds",
                pruning.removed.expired_rows,
                pruning.removed.evicted_rows,
                pruning.policy.path_tree_max_bytes,
                pruning.policy.path_tree_ttl_seconds
            );
        }
        if let Some(integrity) = &status.integrity {
            println!(
                "Integrity: {} (SQLite: {}; foreign keys: {}; blob scan complete: {}; Fast IMG state scan complete: {})",
                integrity.status,
                integrity.sqlite_integrity,
                integrity.foreign_keys,
                integrity.blob_scan_complete,
                integrity.fast_img_scan_complete
            );
            println!(
                "Checked: {} cache rows, {} protected state rows; corrupt: {} cache rows, {} protected state rows",
                integrity.checked_cache_rows,
                integrity.checked_protected_rows,
                integrity.corrupt_cache_rows,
                integrity.corrupt_protected_rows
            );
            for issue in &integrity.issues {
                println!("  {}: {}", issue.code, issue.message);
            }
            if integrity.issue_count > integrity.issues.len() as u64 {
                println!(
                    "  {} issues total; details limited to {}",
                    integrity.issue_count,
                    integrity.issues.len()
                );
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    run(Args::parse())
}

fn run(args: Args) -> Result<()> {
    anyhow::ensure!(
        !args.purge_session_state,
        "MFB_SESSION_CLEANUP_REQUIRES_HISTORY_SELECTION: session diagnostics, progress, scratch and locks were retained. Select exact runs in Processing History, or use --history-cleanup-preview and --history-cleanup with the confirmed snapshot token."
    );
    anyhow::ensure!(
        args.prune || args.preview_prune || args.cache.is_empty(),
        "cache policy flags require --prune or --preview-prune; no action taken"
    );
    anyhow::ensure!(
        args.history_directory.is_none()
            || args.history_stats
            || args.history_backup.is_some()
            || args.history_cleanup_preview.is_some()
            || args.history_cleanup.is_some()
            || args.history_prepare_readonly,
        "--history-directory requires an explicit History operation; no action taken"
    );
    if args.history_prepare_readonly {
        let directory = args
            .history_directory
            .as_deref()
            .context("History preparation requires its explicit directory")?;
        let receipt = dev::infra::history_store::prepare_history_read_only_access(directory)?;
        println!("{}", serde_json::to_string(&receipt)?);
        io::stdout().flush()?;
        return Ok(());
    }
    if let Some(encoded) = args
        .history_cleanup_preview
        .as_ref()
        .or(args.history_cleanup.as_ref())
    {
        anyhow::ensure!(
            encoded.len() <= 64 * 1024,
            "History cleanup JSON exceeds 64 KiB; nothing removed"
        );
        use dev::infra::history_store::{
            HistoryCleanupRequest, HistoryForceCleanupRequest, execute_history_cleanup,
            execute_history_force_cleanup, preview_history_cleanup, preview_history_force_cleanup,
        };

        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum CleanupRequest {
            Normal(HistoryCleanupRequest),
            Force(HistoryForceCleanupRequest),
        }

        let request: CleanupRequest =
            serde_json::from_str(encoded).context("invalid History cleanup request")?;
        let directory = args
            .history_directory
            .as_deref()
            .context("History cleanup requires its explicit directory")?;
        let response = if args.history_cleanup_preview.is_some() {
            match request {
                CleanupRequest::Normal(request) => {
                    serde_json::to_string(&preview_history_cleanup(directory, &request)?)?
                }
                CleanupRequest::Force(request) => {
                    serde_json::to_string(&preview_history_force_cleanup(directory, &request)?)?
                }
            }
        } else {
            // Hold the same lease as media workers until database and archive cleanup finish.
            use foundation::production_lock::{acquire_production_lease, is_production_task_busy};
            let _lease = acquire_production_lease().map_err(|error| {
                if is_production_task_busy(&error) {
                    error.context("MFB_HISTORY_CLEANUP_MEDIA_BUSY: finish the active media task or history cleanup before deleting records")
                } else {
                    error.context("acquire exclusive History cleanup lease")
                }
            })?;
            match request {
                CleanupRequest::Normal(request) => {
                    serde_json::to_string(&execute_history_cleanup(directory, &request)?)?
                }
                CleanupRequest::Force(request) => {
                    serde_json::to_string(&execute_history_force_cleanup(directory, &request)?)?
                }
            }
        };
        println!("{response}");
        io::stdout().flush()?;
        return Ok(());
    }
    if args.history_stats {
        let directory = args
            .history_directory
            .as_deref()
            .context("History inspection requires an explicit history directory")?;
        let report =
            dev::infra::history_store::inspect_history_store(directory, args.check_integrity)?;
        if args.json {
            println!("{}", serde_json::to_string(&report)?);
        } else {
            println!("History storage: {}", report.source);
            if let Some(overview) = &report.overview {
                println!(
                    "Schema {}, {} sessions, {} event rows ({} logical bytes); output rows {:?}, output logical bytes {:?}.",
                    overview.history_schema_version,
                    overview.sessions,
                    overview.events,
                    overview.event_bytes,
                    overview.output_rows,
                    overview.output_bytes
                );
                println!(
                    "Storage rows are not processed media counts or completed runs. Schema 1 has no captured output storage."
                );
                if overview.history_schema_version >= 2 && overview.output_rows.is_none() {
                    println!(
                        "Captured-output details were not scanned; --check-integrity computes their exact row and byte totals."
                    );
                }
            } else {
                println!("History database is absent; no database created.");
            }
            if let Some(integrity) = &report.integrity {
                println!(
                    "Structural checks: {} ({} issues). Event JSON and media correctness are not verified.",
                    integrity.status, integrity.issue_count
                );
            }
        }
        io::stdout().flush()?;
        if report
            .integrity
            .as_ref()
            .is_some_and(|integrity| integrity.status == "unhealthy")
        {
            std::process::exit(2);
        }
        return Ok(());
    }
    if let Some(destination) = &args.history_backup {
        let directory = args
            .history_directory
            .as_deref()
            .context("History backup requires an explicit history directory")?;
        let source = directory
            .canonicalize()?
            .join(dev::infra::history_store::HISTORY_DATABASE);
        let destination = sqlite_backup::resolve_destination(&source, destination)?;
        require_outside_core_storage(&backup_cache_directory()?, &destination)?;
        let receipt = dev::infra::history_store::backup_history_store(directory, &destination)?;
        if args.json {
            println!("{}", serde_json::to_string(&receipt)?);
        } else {
            println!(
                "History SQLite backup: {} -> {} ({} bytes, history schema {}, {} sessions, {} event rows, {} output rows). These are storage row counts, not media file counts.",
                receipt.source,
                receipt.destination,
                receipt.bytes,
                receipt.history_schema_version,
                receipt.sessions,
                receipt.events,
                receipt.output_rows
            );
        }
        io::stdout().flush()?;
        return Ok(());
    }
    if args.preview_core_recovery {
        let preview = foundation::mfb_sqlite_store::inspect_formal_state_recovery()?;
        if args.json {
            println!("{}", serde_json::to_string(&preview)?);
        } else {
            println!(
                "Active Core database is missing: {}\nHistorical migration snapshot: {} (schema {})\nSnapshot BLAKE3: {}\nThis snapshot predates relocation. Use it to inspect historical records.",
                preview.missing_database.display(),
                preview.snapshot.display(),
                preview.store_schema_version,
                preview.snapshot_blake3,
            );
        }
        io::stdout().flush()?;
        return Ok(());
    }
    if let Some(destination) = &args.export_core_recovery {
        let expected = args
            .expected_snapshot_blake3
            .as_deref()
            .context("Core recovery requires the preview digest")?;
        let receipt =
            foundation::mfb_sqlite_store::restore_formal_state_snapshot_to(destination, expected)?;
        if args.json {
            println!("{}", serde_json::to_string(&receipt)?);
        } else {
            println!(
                "Verified historical Core snapshot exported to {}\nThe active database remains unavailable at {}. Inspect the snapshot for historical evidence.",
                receipt.destination_snapshot.display(),
                receipt.missing_database.display(),
            );
        }
        io::stdout().flush()?;
        return Ok(());
    }
    if args.migrate_core_state {
        let receipt = foundation::mfb_sqlite_store::migrate_formal_state_location()?;
        if args.json {
            println!("{}", serde_json::to_string(&receipt)?);
        } else {
            println!(
                "Core database: {} → {}. Verified rollback snapshot: {}",
                receipt.source.display(),
                receipt.destination.display(),
                receipt.backup.display()
            );
        }
        io::stdout().flush()?;
        return Ok(());
    }
    if let Some(snapshot) = &args.check_core_backup {
        let expected = args
            .expected_backup_blake3
            .as_deref()
            .context("Core backup check requires the original receipt digest")?;
        let proof = check_core_backup(snapshot, expected)?;
        if args.json {
            println!("{}", serde_json::to_string(&proof)?);
        } else {
            println!(
                "Core SQLite backup verified: {} ({} bytes, store schema {}).\nSnapshot BLAKE3: {}",
                proof.snapshot.display(),
                proof.bytes,
                proof.store_schema_version,
                proof.snapshot_blake3,
            );
        }
        io::stdout().flush()?;
        return Ok(());
    }
    if let Some(destination) = &args.local_backup {
        let receipt = backup_local_store(&backup_cache_directory()?, destination)?;
        if args.json {
            println!("{}", serde_json::to_string(&receipt)?);
        } else {
            println!(
                "Core SQLite backup: {} -> {} ({} bytes, store schema {}). Dedicated rebuildable cache, History and PostgreSQL are excluded.\nSnapshot BLAKE3: {}",
                receipt.source,
                receipt.destination,
                receipt.bytes,
                receipt.store_schema_version,
                receipt.snapshot_blake3,
            );
        }
        io::stdout().flush()?;
        return Ok(());
    }

    if args.prune || args.preview_prune {
        let mut loaded = foundation::runtime_config::load(args.config.as_deref(), args.no_config)?;
        args.cache.apply_to(&mut loaded);
        loaded.config.validate()?;
        let cache_dir = if args.preview_prune {
            backup_cache_directory()?
        } else {
            get_mfb_state_root()?.join("cache")
        };
        if args.preview_prune {
            let report = cache_status_with_options(&cache_dir, false, Some(loaded.config.cache))?;
            if !args.json {
                if let Some(preview) = &report.pruning_preview {
                    println!(
                        "Read-only pruning preview at {}: expire {} rows / {} payload bytes; evict {} rows / {} payload bytes; retain {} rows / {} payload bytes. Formal state is excluded. A later prune re-evaluates current data.",
                        preview.snapshot_unix_secs,
                        preview.expired_rows,
                        preview.expired_payload_bytes,
                        preview.evicted_rows,
                        preview.evicted_payload_bytes,
                        preview.retained_rows,
                        preview.retained_payload_bytes
                    );
                } else {
                    println!("No cache store is present. No database was created.");
                }
            }
            return print_cache_status(&report, args.json);
        }
        let before = cache_status(&cache_dir)?;
        if !args.json {
            print_cache_status(&before, false)?;
            println!(
                "Pruning policy: {} payload bytes, {} idle seconds",
                loaded.config.cache.path_tree_max_bytes, loaded.config.cache.path_tree_ttl_seconds
            );
        }
        if !confirm_cleanup(
            args.yes,
            "Prune expired and over-budget directory snapshots only? Formal state is retained.",
        )? {
            anyhow::bail!("cache pruning cancelled; no action taken");
        }
        return print_cache_status(
            &prune_local_cache(&cache_dir, loaded.config.cache)?,
            args.json,
        );
    }

    if args.purge_animation_cache {
        perform_animation_cache_cleanup(args.yes)?;
        return Ok(());
    }

    let cache_dir = if args.stats {
        backup_cache_directory()?
    } else {
        get_mfb_state_root()?.join("cache")
    };
    let before = cache_status_with_integrity(&cache_dir, args.check_integrity)?;
    if args.stats {
        print_cache_status(&before, args.json)?;
        if args.json
            && before
                .integrity
                .as_ref()
                .is_some_and(|report| matches!(report.status, "unhealthy" | "error"))
        {
            // Keep machine-readable stdout valid even when inspection fails.
            io::stdout().flush()?;
            std::process::exit(1);
        }
        anyhow::ensure!(
            before
                .integrity
                .as_ref()
                .is_none_or(|report| matches!(report.status, "healthy" | "absent")),
            "cache integrity check failed; see inspection report"
        );
        return Ok(());
    }
    let target = args
        .path
        .as_deref()
        .map(Path::new)
        .map(Path::canonicalize)
        .transpose()?;
    if args.postgres {
        check_postgres_reachable().context("PostgreSQL is required for --postgres cleanup")?;
    }
    if !args.json {
        print_cache_status(&before, false)?;
    }
    if !confirm_cleanup(
        args.yes,
        "Clear rebuildable cache only? History and resume state are retained.",
    )? {
        anyhow::bail!("cache cleanup cancelled; no action taken");
    }
    if args.postgres {
        if let Some(target) = &target {
            purge_postgres_for_path(target)?;
            purge_postgres_inference_logs_for_path(target)?;
        } else {
            purge_postgres_full()?;
        }
    }
    let status = purge_local_cache(&cache_dir, target.as_deref())?;
    print_cache_status(&status, args.json)
}

#[cfg(test)]
mod tests {
    #[test]
    fn history_and_preview_inspection_flags_never_select_mutations() {
        use clap::Parser;
        let history = super::Args::try_parse_from([
            "cache_cleaner",
            "--history-stats",
            "--history-directory",
            "/synthetic/history",
            "--json",
            "--check-integrity",
        ])
        .unwrap();
        assert!(history.history_stats && history.check_integrity && !history.prune && !history.yes);
        let preview = super::Args::try_parse_from([
            "cache_cleaner",
            "--preview-prune",
            "--json",
            "--cache-max-bytes",
            "100",
            "--cache-ttl-seconds",
            "10",
        ])
        .unwrap();
        assert!(
            preview.preview_prune && !preview.prune && !preview.yes && !preview.cache.is_empty()
        );
        for flag in [
            "--yes",
            "--stats",
            "--prune",
            "--purge-session-state",
            "--postgres",
        ] {
            assert!(
                super::Args::try_parse_from(["cache_cleaner", "--preview-prune", flag]).is_err()
            );
            assert!(
                super::Args::try_parse_from([
                    "cache_cleaner",
                    "--history-stats",
                    "--history-directory",
                    "/synthetic/history",
                    flag
                ])
                .is_err()
            );
        }
        assert!(super::Args::try_parse_from(["cache_cleaner", "--history-stats"]).is_err());
    }

    #[test]
    fn pruning_preview_uses_the_same_read_only_namespace_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let conn = rusqlite::Connection::open(cache.join("mfb_store.sqlite")).unwrap();
        conn.execute_batch("CREATE TABLE store_metadata(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
            INSERT INTO store_metadata VALUES ('schema_version',2);
            CREATE TABLE blob_store(namespace TEXT NOT NULL,cache_key TEXT NOT NULL,schema_version INTEGER NOT NULL,root_path TEXT,payload BLOB NOT NULL,payload_blake3 BLOB NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(namespace,cache_key));
            CREATE INDEX idx_blob_ns_root ON blob_store(namespace,root_path);
            CREATE INDEX idx_blob_ns_updated ON blob_store(namespace,updated_at);
            INSERT INTO blob_store VALUES ('path_tree','one',1,NULL,X'6361636865',zeroblob(32),0),('checkpoint','state',1,NULL,X'70726F746563746564',zeroblob(32),0);").unwrap();
        drop(conn);
        let source = cache.join("mfb_store.sqlite");
        let before = std::fs::read(&source).unwrap();
        let policy = foundation::runtime_config::CachePolicy {
            path_tree_max_bytes: 1,
            path_tree_ttl_seconds: 10,
        };
        let status = super::cache_status_with_options(&cache, false, Some(policy)).unwrap();
        let preview = status.pruning_preview.unwrap();
        assert_eq!(preview.total_rows, 1);
        assert_eq!(preview.total_payload_bytes, 5);
        assert_eq!(
            preview.expired_rows + preview.evicted_rows + preview.retained_rows,
            1
        );
        assert_eq!(status.removed_rows, 0);
        assert_eq!(status.removed_files, 0);
        assert_eq!(
            status
                .namespaces
                .iter()
                .find(|namespace| namespace.name == "checkpoint")
                .unwrap()
                .rows,
            1
        );
        assert_eq!(std::fs::read(source).unwrap(), before);
        let absent = super::cache_status_with_options(
            &root.path().join("absent/cache"),
            false,
            Some(policy),
        )
        .unwrap();
        assert!(absent.database.is_none() && absent.pruning_preview.is_none());
        assert!(!root.path().join("absent").exists());
    }
    use super::*;

    fn cache_fixture(root: &Path) -> PathBuf {
        let cache = root.join("cache");
        fs::create_dir(&cache).unwrap();
        let conn = Connection::open(cache.join("mfb_store.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             INSERT INTO store_metadata VALUES ('schema_version', 2);
             CREATE TABLE blob_store (namespace TEXT NOT NULL, cache_key TEXT NOT NULL,
                 schema_version INTEGER NOT NULL, root_path TEXT, payload BLOB NOT NULL,
                 payload_blake3 BLOB NOT NULL, updated_at INTEGER NOT NULL,
                 PRIMARY KEY(namespace, cache_key));
             CREATE INDEX idx_blob_ns_root ON blob_store(namespace,root_path);
             CREATE INDEX idx_blob_ns_updated ON blob_store(namespace,updated_at);",
        )
        .unwrap();
        for namespace in [
            "path_tree",
            "checkpoint",
            "processed",
            "future_verification_state",
        ] {
            conn.execute(
                "INSERT INTO blob_store VALUES (?1, 'key', 2, '/synthetic', ?2, ?3, 123)",
                params![namespace, b"payload", &[7u8; 32]],
            )
            .unwrap();
        }
        cache
    }

    fn healthy_cache_fixture(root: &Path) -> PathBuf {
        let cache = cache_fixture(root);
        let conn = Connection::open(cache.join("mfb_store.sqlite")).unwrap();
        conn.execute(
            "UPDATE blob_store SET payload_blake3 = ?1",
            params![blake3::hash(b"payload").as_bytes()],
        )
        .unwrap();
        cache
    }

    fn fast_img_state_test_root() -> Option<PathBuf> {
        const CHILD: &str = "MFB_CACHE_FAST_IMG_STATE_TEST_ROOT";
        if let Some(root) = std::env::var_os(CHILD) {
            return Some(PathBuf::from(root));
        }
        let fixture = tempfile::tempdir().unwrap();
        let thread = std::thread::current();
        let name = thread.name().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, fixture.path())
            .env(
                foundation::constants::ENV_MFB_HOME_ROOT,
                fixture.path().join("mfb-state"),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
            "isolated Fast IMG state check failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        None
    }

    #[test]
    fn dedicated_cache_maintenance_retains_legacy_and_protected_state() -> Result<()> {
        use foundation::mfb_sqlite_store::{
            CACHE_STORE_FILE_NAME, NS_GATE1_PROOF, NS_PATH_TREE, blob_put,
        };

        let Some(root) = fast_img_state_test_root() else {
            return Ok(());
        };
        let mfb_root = root.join("mfb-state");
        fs::create_dir_all(&mfb_root)?;
        let target = root.canonicalize()?.join("synthetic");
        fs::create_dir(&target)?;
        let cache = healthy_cache_fixture(&mfb_root);
        let state_path = cache.join("mfb_store.sqlite");
        let before = fs::read(&state_path)?;
        assert!(!cache.join(CACHE_STORE_FILE_NAME).exists());
        blob_put(
            NS_PATH_TREE,
            "new-tree",
            2,
            Some(&target),
            b"independent tree",
        )?;
        blob_put(
            NS_GATE1_PROOF,
            "new-proof",
            2,
            Some(&target),
            b"independent proof",
        )?;
        let status = cache_status_with_options(
            &cache,
            true,
            Some(foundation::runtime_config::CachePolicy::default()),
        )?;
        assert_eq!(status.database.as_ref().unwrap().store_schema_version, 2);
        assert_eq!(
            status
                .rebuildable_database
                .as_ref()
                .unwrap()
                .store_schema_version,
            1
        );
        let cache_db = cache.join(CACHE_STORE_FILE_NAME);
        let standalone_cache = root.join("standalone-cache.sqlite");
        let cache_connection =
            foundation::mfb_sqlite_store::open_rebuildable_cache_at(&cache_db, true)?
                .context("missing synthetic rebuildable cache")?;
        let mut standalone_connection = Connection::open(&standalone_cache)?;
        foundation::mfb_sqlite_store::copy_sqlite_snapshot(
            &cache_connection,
            &mut standalone_connection,
        )?;
        drop(standalone_connection);
        drop(cache_connection);
        let actual = foundation::common_utils::calculate_blake3_hash(&standalone_cache)?;
        let result = check_core_backup(&standalone_cache, &actual).unwrap_err();
        assert!(
            result
                .to_string()
                .contains("not a recognized Core SQLite database")
        );
        assert_eq!(status.inactive_legacy_cache.as_ref().unwrap().rows, 1);
        assert_eq!(status.pruning_preview.as_ref().unwrap().total_rows, 1);
        let integrity = status.integrity.unwrap();
        assert_eq!(integrity.status, "healthy");
        assert_eq!(integrity.checked_cache_rows, 2);
        assert_eq!(integrity.checked_protected_rows, 3);
        assert_eq!(fs::read(&state_path)?, before);
        let purged = purge_local_cache(&cache, Some(&target))?;
        assert_eq!(purged.removed_rows, 2);
        assert_eq!(purged.inactive_legacy_cache.unwrap().rows, 1);
        assert_eq!(fs::read(&state_path)?, before);

        // Inactive old cache may be corrupt without compromising a Core backup.
        {
            let state = Connection::open(&state_path)?;
            state.execute(
                "UPDATE blob_store SET payload_blake3=zeroblob(32) WHERE namespace='path_tree'",
                [],
            )?;
        }
        let original_with_inactive_cache = fs::read(&state_path)?;
        let destination = root.join("preserved-core.sqlite");
        backup_local_store(&cache, &destination)?;
        let backup = open_cache_store(&destination, true)?;
        let records = read_cache_namespaces(&backup)?;
        assert_eq!(records.len(), 3);
        assert!(records.iter().all(|record| !record.rebuildable));
        assert_eq!(fs::read(&state_path)?, original_with_inactive_cache);
        Ok(())
    }

    fn fast_img_state_fixture(
        root: &Path,
    ) -> (
        PathBuf,
        Connection,
        foundation::pipeline::verification::WorkingCopyMarker,
    ) {
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgStageName, LibraryAssetRecord, PhotosSubmissionBackend,
            PhotosSubmissionIntent, PhotosSubmissionKind, SkippedSourceEntry, WorkingCopyMarker,
            write_marker_atomic,
        };
        let source_hash = blake3::hash(b"synthetic source").to_hex().to_string();
        let output_hash = blake3::hash(b"synthetic output").to_hex().to_string();
        let mut marker = WorkingCopyMarker::new(root.join("source"), root.join("output"), 3);
        marker.stage = FastImgStageName::Importing;
        marker.encoded_count = 1;
        marker.blake3_log.insert(
            "one.jpg".into(),
            Blake3Entry {
                src: source_hash.clone(),
                out: output_hash.clone(),
                out_rel: Some("one.JXL".into()),
                library_asset: Some(output_hash.clone()),
            },
        );
        marker.failed_sources.insert(
            "two.jpg".into(),
            SkippedSourceEntry {
                src: blake3::hash(b"synthetic failure").to_hex().to_string(),
                reason: "synthetic media failure".into(),
            },
        );
        marker.skipped_sources.insert(
            "three.jpg".into(),
            SkippedSourceEntry {
                src: blake3::hash(b"synthetic skip").to_hex().to_string(),
                reason: "synthetic retained input".into(),
            },
        );
        marker
            .record_photos_submissions(&[PhotosSubmissionIntent {
                kind: PhotosSubmissionKind::Encoded,
                source_rel: "one.jpg".into(),
                source_blake3: source_hash,
                output_rel: "one.JXL".into(),
                output_blake3: output_hash.clone(),
                album_name: "Synthetic archive".into(),
                photos_library_path: None,
                backend: PhotosSubmissionBackend::Applescript {
                    request_id: blake3::hash(b"synthetic request").to_hex().to_string(),
                },
            }])
            .unwrap();
        marker.photos_imported_assets.push(LibraryAssetRecord {
            rel_path: "one.JXL".into(),
            blake3: output_hash,
            photos_uuid: Some("SYNTHETIC-UUID".into()),
            sync_status: "photos_local".into(),
            ..Default::default()
        });
        let state_root = foundation::process_lock::get_mfb_root().unwrap();
        let cache = healthy_cache_fixture(&state_root);
        let store = cache.join("mfb_store.sqlite");
        let setup = Connection::open(&store).unwrap();
        setup.execute("DELETE FROM blob_store", []).unwrap();
        drop(setup);
        write_marker_atomic(&marker).unwrap();
        assert_eq!(
            foundation::mfb_sqlite_store::default_store_path().unwrap(),
            store
        );
        let connection = Connection::open(&store).unwrap();
        (cache, connection, marker)
    }

    #[test]
    fn relocated_core_is_inspected_and_backed_up_but_excluded_from_cache_maintenance() -> Result<()>
    {
        let Some(root) = fast_img_state_test_root() else {
            return Ok(());
        };
        let (cache, source, _) = fast_img_state_fixture(&root);
        source.execute(
            "INSERT INTO blob_store VALUES ('path_tree','inactive',1,NULL,?1,?2,0)",
            params![
                b"synthetic cache",
                blake3::hash(b"synthetic cache").as_bytes()
            ],
        )?;
        drop(source);
        let migration = foundation::mfb_sqlite_store::migrate_formal_state_location()?;
        assert_ne!(migration.source, migration.destination);
        assert!(migration.backup.is_file());
        let before = fs::read(&migration.destination)?;
        let report = cache_status_with_options(
            &cache,
            true,
            Some(foundation::runtime_config::CachePolicy::default()),
        )?;
        assert_eq!(
            report.core_store_path.as_deref(),
            migration.destination.to_str()
        );
        assert!(report.pruning_preview.is_none());
        assert_eq!(report.inactive_legacy_cache.unwrap().rows, 1);
        assert_eq!(report.integrity.unwrap().status, "healthy");
        assert_eq!(purge_local_cache(&cache, None)?.removed_rows, 0);
        let backup = root.join("core.sqlite");
        let receipt = backup_local_store(&cache, &backup)?;
        assert_eq!(
            receipt.source,
            migration.destination.canonicalize()?.to_str().unwrap()
        );
        let checked = check_core_backup(&backup, &receipt.snapshot_blake3)?;
        assert_eq!(
            checked.store_schema_version,
            foundation::mfb_sqlite_store::STORE_SCHEMA_VERSION
        );
        assert!(backup_local_store(&cache, &cache.join("core.sqlite")).is_err());
        let conn = open_cache_store(&backup, true)?;
        assert_eq!(foundation::fast_img_store::inspect_integrity(&conn)?, 7);
        assert!(
            read_cache_namespaces(&conn)?
                .iter()
                .all(|namespace| !namespace.rebuildable)
        );
        assert_eq!(fs::read(&migration.destination)?, before);
        let damaged = Connection::open(&migration.destination)?;
        damaged.execute(
            "UPDATE store_metadata SET value=value+1 WHERE key='core_state_location_v1'",
            [],
        )?;
        let refused = cache_status_with_integrity(&cache, true)?;
        assert_eq!(refused.integrity.unwrap().status, "error");
        assert!(backup_local_store(&cache, &root.join("refused.sqlite")).is_err());
        assert!(!root.join("refused.sqlite").exists());
        Ok(())
    }

    #[test]
    fn fast_img_state_is_counted_verified_backed_up_and_retained_by_cache_cleanup() {
        let Some(root) = fast_img_state_test_root() else {
            return;
        };
        let (cache, conn, _marker) = fast_img_state_fixture(&root);
        let protected_rows = foundation::fast_img_store::inspect_integrity(&conn).unwrap();
        assert_eq!(protected_rows, 7);
        for namespace in ["path_tree", "checkpoint"] {
            conn.execute(
                "INSERT INTO blob_store VALUES (?1,'synthetic',1,NULL,?2,?3,0)",
                params![
                    namespace,
                    b"synthetic payload",
                    blake3::hash(b"synthetic payload").as_bytes()
                ],
            )
            .unwrap();
        }
        let snapshot: Vec<u8> = conn
            .query_row("SELECT snapshot_blake3 FROM fast_img_tasks", [], |row| {
                row.get(0)
            })
            .unwrap();
        let status = cache_status_with_integrity(&cache, true).unwrap();
        let integrity = status.integrity.as_ref().unwrap();
        assert_eq!(integrity.status, "healthy");
        assert!(integrity.blob_scan_complete && integrity.fast_img_scan_complete);
        assert_eq!(integrity.checked_fast_img_rows, 7);
        assert_eq!(integrity.checked_cache_rows, 1);
        assert_eq!(integrity.checked_protected_rows, 8);
        let namespace = status
            .namespaces
            .iter()
            .find(|namespace| namespace.name == "fast_img_state")
            .unwrap();
        assert_eq!(namespace.rows, 7);
        assert!(!namespace.rebuildable);
        let header_bytes: u64 = conn
            .query_row(
                "SELECT SUM(length(header_json)) FROM fast_img_tasks",
                [],
                |row| nonnegative_column(row, 0),
            )
            .unwrap();
        assert_eq!(namespace.payload_bytes, header_bytes);
        assert!(namespace.payload_bytes > 0);
        let unchecked = cache_status(&cache).unwrap();
        assert!(unchecked.integrity.is_none());
        assert_eq!(
            unchecked
                .namespaces
                .iter()
                .find(|namespace| namespace.name == "fast_img_state")
                .unwrap()
                .rows,
            7
        );
        let destination = root.join("verified-core-backup.sqlite");
        let receipt = backup_local_store(&cache, &destination).unwrap();
        assert_eq!(
            receipt.store_schema_version,
            foundation::mfb_sqlite_store::STORE_SCHEMA_VERSION
        );
        let backup = open_cache_store(&destination, true).unwrap();
        assert_eq!(
            foundation::fast_img_store::inspect_integrity(&backup).unwrap(),
            protected_rows
        );
        let purged = purge_local_cache(&cache, None).unwrap();
        assert_eq!(purged.removed_rows, 1);
        conn.execute(
            "INSERT INTO blob_store VALUES ('path_tree','again',1,NULL,?1,?2,0)",
            params![
                b"synthetic payload",
                blake3::hash(b"synthetic payload").as_bytes()
            ],
        )
        .unwrap();
        let pruned = prune_local_cache(
            &cache,
            foundation::runtime_config::CachePolicy {
                path_tree_max_bytes: 1024,
                path_tree_ttl_seconds: 1,
            },
        )
        .unwrap();
        assert_eq!(pruned.removed_rows, 1);
        assert_eq!(
            foundation::fast_img_store::inspect_integrity(&conn).unwrap(),
            protected_rows
        );
        let after: Vec<u8> = conn
            .query_row("SELECT snapshot_blake3 FROM fast_img_tasks", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after, snapshot);
        conn.execute(
            "INSERT INTO blob_store VALUES ('fast_img_state','older-unknown-state',1,NULL,?1,?2,0)",
            params![
                b"retained older state",
                blake3::hash(b"retained older state").as_bytes()
            ],
        )
        .unwrap();
        let combined = cache_status_with_integrity(&cache, true).unwrap();
        let namespaces = combined
            .namespaces
            .iter()
            .filter(|namespace| namespace.name == "fast_img_state")
            .collect::<Vec<_>>();
        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].rows, 8);
        assert_eq!(combined.integrity.unwrap().checked_protected_rows, 9);
        assert!(!root.join("source").exists());
        assert!(!root.join("output").exists());
    }

    #[test]
    fn xmp_admission_is_counted_backed_up_and_protected_from_cache_clear() -> Result<()> {
        use foundation::fast_img_store::{
            FastImgAdmissionPlan, FastImgAdmissionRoute, FastImgAdmissionSidecar,
            FastImgSourceAdmission, initialize_with_admission, read_admission_plan,
        };
        use foundation::pipeline::verification::WorkingCopyMarker;

        let Some(root) = fast_img_state_test_root() else {
            return Ok(());
        };
        let root = root.canonicalize()?;
        let state = foundation::process_lock::get_mfb_root()?;
        let cache = healthy_cache_fixture(&state);
        let source = root.join("source");
        fs::create_dir(&source)?;
        let mut marker = WorkingCopyMarker::new(source, root.join("output"), 2);
        let plan = FastImgAdmissionPlan::from([
            (
                "one.jpg".into(),
                FastImgSourceAdmission {
                    source_blake3: blake3::hash(b"synthetic JPEG").to_hex().to_string(),
                    route: FastImgAdmissionRoute::Primary,
                    xmp: Some(FastImgAdmissionSidecar::Present {
                        relative_path: "one.JPG.XMP".into(),
                        blake3: blake3::hash(b"synthetic XMP").to_hex().to_string(),
                    }),
                    aae: Some(FastImgAdmissionSidecar::Absent),
                },
            ),
            (
                "two.jpg".into(),
                FastImgSourceAdmission {
                    source_blake3: blake3::hash(b"synthetic JPEG two").to_hex().to_string(),
                    route: FastImgAdmissionRoute::Primary,
                    xmp: Some(FastImgAdmissionSidecar::Absent),
                    aae: Some(FastImgAdmissionSidecar::Unresolved),
                },
            ),
        ]);
        initialize_with_admission(&mut marker, &plan)?;
        let conn = Connection::open(cache.join("mfb_store.sqlite"))?;
        let header = conn.query_row(
            "SELECT length(header_json) FROM fast_img_tasks",
            [],
            |row| nonnegative_column(row, 0),
        )?;
        let media_bytes = (2 * ("one.jpg".len() + 64 + "primary".len())) as u64;
        let xmp_bytes = ("one.jpg".len()
            + "present".len()
            + "one.JPG.XMP".len()
            + 64
            + "two.jpg".len()
            + "absent".len()) as u64;
        let aae_bytes =
            ("one.jpg".len() + "absent".len() + "two.jpg".len() + "unresolved".len()) as u64;
        let report = cache_status_with_integrity(&cache, true)?;
        let namespace = report
            .namespaces
            .iter()
            .find(|entry| entry.name == "fast_img_state")
            .context("missing Fast IMG state")?;
        assert_eq!(
            (
                namespace.rows,
                namespace.payload_bytes,
                namespace.rebuildable
            ),
            (7, header + media_bytes + xmp_bytes + aae_bytes, false)
        );
        let backup = root.join("xmp-admission-backup.sqlite");
        backup_local_store(&cache, &backup)?;
        purge_local_cache(&cache, None)?;
        assert_eq!(
            foundation::fast_img_store::inspect_integrity(&open_cache_store(&backup, true)?)?,
            7
        );
        assert_eq!(read_admission_plan(&marker)?, Some(plan));
        Ok(())
    }

    #[test]
    fn jpeg_execution_receipt_is_counted_backed_up_and_protected_from_cache_clear() {
        use foundation::fast_img_store::{JpegEncoderInvocation, write_marker_with_jpeg_execution};
        use foundation::pipeline::verification::{
            FastImgEncodingParameters, FastImgProcessingContract,
        };
        use foundation::runtime_config::{FallbackPolicy, JpegEffortSearch, ToolPolicy};
        let Some(root) = fast_img_state_test_root() else {
            return;
        };
        let (cache, conn, mut marker) = fast_img_state_fixture(&root);
        marker.processing_contract = Some(FastImgProcessingContract {
            schema: 1,
            tool_policy: ToolPolicy::Single,
            fallback_policy: FallbackPolicy::Strict,
            encoding: FastImgEncodingParameters::Jxl {
                jpeg_effort: 3,
                jpeg_effort_search: JpegEffortSearch::Fixed,
                archive: false,
                allow_expert_options: false,
            },
        });
        let source = root.canonicalize().unwrap().join("source/one.jpg");
        write_marker_with_jpeg_execution(
            &marker,
            "one.jpg",
            JpegEncoderInvocation {
                executable: "/synthetic/cjxl".into(),
                executable_blake3: blake3::hash(b"synthetic encoder").to_hex().to_string(),
                argv: [
                    source.to_str().unwrap(),
                    "/synthetic/candidate.JXL",
                    "--container=1",
                    "-e",
                    "3",
                    "--num_threads",
                    "1",
                    "--lossless_jpeg=1",
                ]
                .map(str::to_owned)
                .to_vec(),
                idle_timeout_seconds: 60,
                hard_timeout_seconds: 120,
                exit_code: 0,
            },
            None,
        )
        .unwrap();
        let payload: Vec<u8> = conn
            .query_row("SELECT payload FROM fast_img_jpeg_execution", [], |row| {
                row.get(0)
            })
            .unwrap();
        let observation: Vec<u8> = conn
            .query_row("SELECT payload FROM fast_img_primary_results", [], |row| {
                row.get(0)
            })
            .unwrap();
        let expected_bytes = conn
            .query_row(
                "SELECT SUM(length(header_json)) FROM fast_img_tasks",
                [],
                |row| nonnegative_column(row, 0),
            )
            .unwrap()
            + payload.len() as u64
            + observation.len() as u64
            + 64;
        let report = cache_status_with_integrity(&cache, true).unwrap();
        assert_eq!(report.integrity.unwrap().checked_fast_img_rows, 9);
        let namespace = report
            .namespaces
            .iter()
            .find(|namespace| namespace.name == "fast_img_state")
            .unwrap();
        assert_eq!(
            (
                namespace.rows,
                namespace.payload_bytes,
                namespace.rebuildable
            ),
            (9, expected_bytes, false)
        );
        let backup_path = root.join("jpeg-execution-backup.sqlite");
        backup_local_store(&cache, &backup_path).unwrap();
        purge_local_cache(&cache, None).unwrap();
        for database in [open_cache_store(&backup_path, true).unwrap(), conn] {
            assert_eq!(
                foundation::fast_img_store::inspect_integrity(&database).unwrap(),
                9
            );
            let kept_observation: Vec<u8> = database
                .query_row("SELECT payload FROM fast_img_primary_results", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(kept_observation, observation);
            let retained: Vec<u8> = database
                .query_row("SELECT payload FROM fast_img_jpeg_execution", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(retained, payload);
        }
    }

    #[test]
    fn fast_img_state_schema3_readers_preserve_version_without_migration() {
        let Some(root) = fast_img_state_test_root() else {
            return;
        };
        let (cache, conn, _marker) = fast_img_state_fixture(&root);
        // Reproduce the exact previous release shape using only disposable data.
        conn.execute_batch(
            "DROP TABLE fast_img_primary_results;
             DROP TABLE fast_img_admission_aae;
             DROP TABLE fast_img_admission_xmp;
             DROP TABLE fast_img_admission;
             DROP TABLE fast_img_avif_execution;
             DROP TABLE fast_img_jpeg_execution;
             DROP TABLE fast_img_photos_uncertain;
             DROP TABLE fast_img_tier2_failures;
             DROP TABLE fast_img_tier2_cleanup;
             DROP TABLE fast_img_primary_cleanup;
             DROP TABLE fast_img_retry_sources;
             DROP TABLE fast_img_retry_tasks;
             ALTER TABLE fast_img_migrations DROP COLUMN directory_snapshot_blake3;
             ALTER TABLE fast_img_migrations DROP COLUMN directory_snapshot;
             UPDATE store_metadata SET value=3 WHERE key='schema_version';",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO blob_store VALUES ('path_tree','synthetic',1,NULL,?1,?2,0)",
            params![b"cache", blake3::hash(b"cache").as_bytes()],
        )
        .unwrap();
        let header_bytes: u64 = conn
            .query_row(
                "SELECT SUM(length(header_json)) FROM fast_img_tasks",
                [],
                |row| nonnegative_column(row, 0),
            )
            .unwrap();
        conn.pragma_update(None, "journal_mode", "delete").unwrap();
        drop(conn);
        let source = cache.join("mfb_store.sqlite");
        let before = fs::read(&source).unwrap();
        let status = cache_status_with_options(
            &cache,
            true,
            Some(foundation::runtime_config::CachePolicy::default()),
        )
        .unwrap();
        assert_eq!(status.database.unwrap().store_schema_version, 3);
        assert_eq!(status.integrity.unwrap().status, "healthy");
        assert_eq!(status.pruning_preview.unwrap().total_rows, 1);
        let namespace = status
            .namespaces
            .iter()
            .find(|namespace| namespace.name == "fast_img_state")
            .unwrap();
        assert_eq!((namespace.rows, namespace.payload_bytes), (7, header_bytes));
        let destination = root.join("verified-schema3-backup.sqlite");
        let receipt = backup_local_store(&cache, &destination).unwrap();
        assert_eq!(receipt.store_schema_version, 3);
        let backup = open_cache_store(&destination, true).unwrap();
        assert_eq!(cache_store_schema_version(&backup).unwrap(), 3);
        assert!(
            backup
                .prepare("SELECT directory_snapshot FROM fast_img_migrations")
                .is_err()
        );
        assert_eq!(
            foundation::fast_img_store::inspect_integrity(&backup).unwrap(),
            7
        );
        assert_eq!(fs::read(&source).unwrap(), before);
    }

    #[test]
    fn fast_img_state_schema4_payload_counts_actual_directory_snapshot_bytes() {
        let Some(root) = fast_img_state_test_root() else {
            return;
        };
        let (_cache, conn, marker) = fast_img_state_fixture(&root);
        let raw_marker = serde_json::to_vec(&marker).unwrap();
        let directory_snapshot = b"synthetic metadata snapshot payload";
        let output: String = conn
            .query_row("SELECT working_copy FROM fast_img_tasks", [], |row| {
                row.get(0)
            })
            .unwrap();
        let origin = PathBuf::from(output).join(".mfb_wc");
        conn.execute(
            "INSERT INTO fast_img_migrations(origin_path,origin_blake3,source_json,task_key,retired,
                 directory_snapshot,directory_snapshot_blake3)
             SELECT ?1,?2,?3,task_key,1,?4,?5 FROM fast_img_tasks",
            params![
                origin.to_str().unwrap(),
                blake3::hash(&raw_marker).as_bytes(),
                &raw_marker,
                directory_snapshot,
                blake3::hash(directory_snapshot).as_bytes()
            ],
        )
        .unwrap();
        let header_bytes: u64 = conn
            .query_row(
                "SELECT SUM(length(header_json)) FROM fast_img_tasks",
                [],
                |row| nonnegative_column(row, 0),
            )
            .unwrap();
        // This checks storage accounting only; payload integrity and typed
        // metadata validation remain the independent full-inspection contract.
        let mut namespaces = Vec::new();
        add_fast_img_state_namespace(&conn, &mut namespaces, 8).unwrap();
        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].rows, 8);
        assert_eq!(
            namespaces[0].payload_bytes,
            header_bytes + u64::try_from(raw_marker.len() + directory_snapshot.len()).unwrap()
        );
        assert!(!namespaces[0].rebuildable);
    }

    #[test]
    fn fast_img_cleanup_evidence_counts_payload_bytes_and_v5_readers_do_not_migrate() {
        let Some(root) = fast_img_state_test_root() else {
            return;
        };
        let (cache, conn, _marker) = fast_img_state_fixture(&root);
        let claim = "合成 claim";
        conn.execute(
            "INSERT INTO fast_img_primary_cleanup (
                task_key,source_rel,entry_rel,kind,source_blake3,output_rel,output_blake3,
                entry_blake3,entry_claim,state,evidence_blake3
             ) SELECT task_key,'one.jpg','one.jpg','media','src','one.JXL','out',
                      'src',?1,'planned',zeroblob(32) FROM fast_img_tasks",
            [claim],
        )
        .unwrap();
        let tier2_claim = "合成 original-format claim";
        conn.execute(
            "INSERT INTO fast_img_tier2_cleanup (
                task_key,source_rel,entry_rel,kind,source_blake3,asset_blake3,library_blake3,
                entry_blake3,entry_claim,state,evidence_blake3
             ) SELECT task_key,'one.heic','one.heic','media','src','asset','library',
                      'src',?1,'planned',zeroblob(32) FROM fast_img_tasks",
            [tier2_claim],
        )
        .unwrap();
        // Storage accounting uses actual evidence bytes. Other tests exercise
        // durable receipt integrity independently from these synthetic columns.
        let header_bytes = conn
            .query_row(
                "SELECT SUM(length(header_json)) FROM fast_img_tasks",
                [],
                |row| nonnegative_column(row, 0),
            )
            .unwrap();
        let mut namespaces = Vec::new();
        add_fast_img_state_namespace(&conn, &mut namespaces, 8).unwrap();
        assert_eq!(
            namespaces[0].payload_bytes,
            header_bytes + u64::try_from(claim.len() + tier2_claim.len() + 64).unwrap()
        );
        assert!(!namespaces[0].rebuildable);

        conn.execute_batch(
            "DROP TABLE fast_img_primary_results;
             DROP TABLE fast_img_admission_aae;
             DROP TABLE fast_img_admission_xmp;
             DROP TABLE fast_img_admission;
             DROP TABLE fast_img_avif_execution;
             DROP TABLE fast_img_jpeg_execution;
             DROP TABLE fast_img_photos_uncertain;
             DROP TABLE fast_img_tier2_failures;
             DROP TABLE fast_img_tier2_cleanup;
             DROP TABLE fast_img_primary_cleanup;
             ALTER TABLE fast_img_retry_sources DROP COLUMN route;
             UPDATE store_metadata SET value=5 WHERE key='schema_version';",
        )
        .unwrap();
        conn.pragma_update(None, "journal_mode", "delete").unwrap();
        drop(conn);
        let store = cache.join("mfb_store.sqlite");
        let before = fs::read(&store).unwrap();
        let status = cache_status_with_options(
            &cache,
            true,
            Some(foundation::runtime_config::CachePolicy::default()),
        )
        .unwrap();
        assert_eq!(status.database.unwrap().store_schema_version, 5);
        assert_eq!(status.integrity.unwrap().status, "healthy");
        assert_eq!(status.pruning_preview.unwrap().total_rows, 0);
        let destination = root.join("verified-schema5-backup.sqlite");
        let receipt = backup_local_store(&cache, &destination).unwrap();
        assert_eq!(receipt.store_schema_version, 5);
        assert_eq!(fs::read(store).unwrap(), before);
    }

    #[test]
    fn fast_img_retry_manifest_is_protected_counted_and_included_in_verified_backup() {
        let Some(root) = fast_img_state_test_root() else {
            return;
        };
        let (cache, conn, mut marker) = fast_img_state_fixture(&root);
        use foundation::pipeline::verification::{
            FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName,
        };
        marker.stage = FastImgStageName::CleanupComplete;
        marker.processing_contract = Some(FastImgProcessingContract {
            schema: 1,
            tool_policy: foundation::runtime_config::ToolPolicy::Single,
            fallback_policy: foundation::runtime_config::FallbackPolicy::Strict,
            encoding: FastImgEncodingParameters::Jxl {
                jpeg_effort: 11,
                jpeg_effort_search: foundation::runtime_config::JpegEffortSearch::Full,
                archive: true,
                allow_expert_options: false,
            },
        });
        foundation::fast_img_store::write_marker(&marker).unwrap();
        fs::create_dir(&marker.src_dir).unwrap();
        fs::write(marker.src_dir.join("two.jpg"), b"synthetic failure").unwrap();
        let parent =
            foundation::fast_img_store::inspect_failed_retry_parent(&marker.working_copy).unwrap();
        let admissions =
            foundation::fast_img_store::capture_failed_retry_admissions(&parent).unwrap();
        let child = foundation::fast_img_store::create_or_resume_failed_retry(
            &parent,
            &admissions,
            &foundation::fast_img_store::FailedRetryDeliveryIntent::Local,
            None,
        )
        .unwrap();
        let status = cache_status_with_integrity(&cache, true).unwrap();
        assert_eq!(status.integrity.as_ref().unwrap().status, "healthy");
        assert_eq!(status.integrity.as_ref().unwrap().checked_fast_img_rows, 10);
        let expected: u64 = conn
            .query_row(
                "SELECT (SELECT SUM(length(header_json)) FROM fast_img_tasks)
            +(SELECT SUM(length(CAST(source_claim AS BLOB))) FROM fast_img_retry_sources)
            +(SELECT SUM(length(CAST(requested_delivery AS BLOB))) FROM fast_img_retry_tasks)",
                [],
                |r| nonnegative_column(r, 0),
            )
            .unwrap();
        let namespace = status
            .namespaces
            .iter()
            .find(|n| n.name == "fast_img_state")
            .unwrap();
        assert_eq!((namespace.rows, namespace.payload_bytes), (10, expected));
        let destination = root.join("verified-retry-backup.sqlite");
        backup_local_store(&cache, &destination).unwrap();
        let copy = open_cache_store(&destination, true).unwrap();
        assert_eq!(
            foundation::fast_img_store::inspect_integrity(&copy).unwrap(),
            10
        );
        purge_local_cache(&cache, None).unwrap();
        assert_eq!(
            foundation::fast_img_store::read_marker(&parent.marker().working_copy).unwrap(),
            *parent.marker()
        );
        assert!(
            foundation::fast_img_store::failed_retry_context(&child.working_copy)
                .unwrap()
                .is_some()
        );
        assert!(!child.working_copy.exists());
    }

    #[test]
    fn fast_img_state_corruption_or_migration_backup_drift_blocks_healthy_status_and_backup() {
        let Some(root) = fast_img_state_test_root() else {
            return;
        };
        let (cache, conn, marker) = fast_img_state_fixture(&root);
        let bytes = serde_json::to_vec(&marker).unwrap();
        let output: String = conn
            .query_row("SELECT working_copy FROM fast_img_tasks", [], |row| {
                row.get(0)
            })
            .unwrap();
        let origin = PathBuf::from(output).join(".mfb_wc");
        conn.execute(
            "INSERT INTO fast_img_migrations(origin_path,origin_blake3,source_json,task_key,retired)
             SELECT ?1,?2,?3,task_key,1 FROM fast_img_tasks",
            params![origin.to_str().unwrap(), blake3::hash(&bytes).as_bytes(), &bytes],
        )
        .unwrap();
        assert_eq!(
            foundation::fast_img_store::inspect_integrity(&conn).unwrap(),
            8
        );
        let before = cache_status_with_integrity(&cache, true).unwrap();
        assert_eq!(before.integrity.as_ref().unwrap().checked_fast_img_rows, 8);
        let namespace = before
            .namespaces
            .iter()
            .find(|namespace| namespace.name == "fast_img_state")
            .unwrap();
        let header: Vec<u8> = conn
            .query_row("SELECT header_json FROM fast_img_tasks", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            namespace.payload_bytes,
            u64::try_from(header.len() + bytes.len()).unwrap()
        );
        let destination = root.join("rejected-backup.sqlite");
        for corrupt_migration in [false, true] {
            if corrupt_migration {
                conn.execute("UPDATE fast_img_tasks SET header_json=?1", [&header])
                    .unwrap();
                conn.execute(
                    "UPDATE fast_img_migrations SET source_json=?1",
                    [b"corrupt migration".as_slice()],
                )
                .unwrap();
            } else {
                conn.execute(
                    "UPDATE fast_img_tasks SET header_json=?1",
                    [b"corrupt header".as_slice()],
                )
                .unwrap();
            }
            let status = cache_status_with_integrity(&cache, true).unwrap();
            let integrity = status.integrity.unwrap();
            assert_eq!(integrity.status, "error");
            assert!(integrity.blob_scan_complete);
            assert!(!integrity.fast_img_scan_complete);
            assert!(
                integrity
                    .issues
                    .iter()
                    .any(|issue| issue.code == "fast_img_state")
            );
            assert!(backup_local_store(&cache, &destination).is_err());
            assert!(!destination.exists());
        }
        assert!(
            !origin.exists(),
            "integrity inspection must not read or create migration media artifacts"
        );
        assert!(!root.join("source").exists());
        assert!(!root.join("output").exists());
    }

    #[test]
    fn backup_source_root_resolution_is_strict_and_read_only() {
        const CHILD: &str = "MFB_TEST_BACKUP_ROOT_CHILD";
        if let Some(case) = std::env::var_os(CHILD) {
            let case = case.into_string().expect("backup test case is not UTF-8");
            if case == "absent" {
                let cache = backup_cache_directory().unwrap();
                let root = cache.parent().unwrap();
                assert!(!root.exists());
                assert!(backup_local_store(&cache, &root.with_extension("backup.sqlite")).is_err());
                assert!(!root.exists());
            } else if case == "home" {
                let home = PathBuf::from(std::env::var(foundation::constants::ENV_HOME).unwrap());
                assert_eq!(
                    backup_cache_directory().unwrap(),
                    home.join(".modern_format_boost/cache")
                );
                assert!(!home.exists());
            } else {
                assert!(
                    backup_cache_directory().is_err(),
                    "invalid source must not select HOME or a temporary fallback"
                );
            }
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let name = format!(
            "{}::backup_source_root_resolution_is_strict_and_read_only",
            module_path!()
        );
        let (_, name) = name.split_once("::").unwrap();
        let cases = ["absent", "relative", "empty", "home", "no-home"];
        #[cfg(unix)]
        let cases = cases.into_iter().chain(["invalid-root", "invalid-home"]);
        for case in cases {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", name])
                .env(CHILD, case)
                .env_remove(foundation::constants::ENV_MFB_HOME_ROOT)
                .env_remove(foundation::constants::ENV_HOME)
                .env_remove(foundation::constants::ENV_USERPROFILE);
            match case {
                "absent" => {
                    command.env(
                        foundation::constants::ENV_MFB_HOME_ROOT,
                        root.path().join("absent-root"),
                    );
                }
                "relative" => {
                    command
                        .env(foundation::constants::ENV_MFB_HOME_ROOT, "relative-root")
                        .env(foundation::constants::ENV_HOME, root.path());
                }
                "empty" => {
                    command
                        .env(foundation::constants::ENV_MFB_HOME_ROOT, "")
                        .env(foundation::constants::ENV_HOME, root.path());
                }
                "home" => {
                    command.env(
                        foundation::constants::ENV_HOME,
                        root.path().join("absent-home"),
                    );
                }
                #[cfg(unix)]
                "invalid-root" | "invalid-home" => {
                    use std::os::unix::ffi::OsStringExt;
                    let key = if case == "invalid-root" {
                        foundation::constants::ENV_MFB_HOME_ROOT
                    } else {
                        foundation::constants::ENV_HOME
                    };
                    command
                        .env(key, std::ffi::OsString::from_vec(vec![0xff]))
                        .env(foundation::constants::ENV_USERPROFILE, root.path());
                    if case == "invalid-root" {
                        command.env(foundation::constants::ENV_HOME, root.path());
                    }
                }
                _ => {}
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
        }
    }

    #[test]
    fn database_overview_is_read_only_complete_and_absent_without_creation() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing-cache");
        let absent = cache_status(&missing).unwrap();
        assert!(absent.database.is_none());
        assert!(!missing.exists());
        let cache = healthy_cache_fixture(root.path());
        let source = cache.join("mfb_store.sqlite");
        let before = fs::read(&source).unwrap();
        let status = cache_status(&cache).unwrap();
        let overview = status.database.as_ref().unwrap();
        assert_eq!(overview.store_schema_version, 2,);
        assert_eq!(overview.sqlite_version, rusqlite::version());
        assert_eq!(overview.journal_mode, "delete");
        assert!(
            overview.page_size.is_power_of_two()
                && overview.page_count > 0
                && overview.free_pages <= overview.page_count
        );
        assert_eq!(status.namespaces.len(), 4);
        let value = serde_json::to_value(status).unwrap();
        assert!(value["database"]["page_size"].as_u64().unwrap() >= 512);
        assert_eq!(fs::read(source).unwrap(), before);
        assert_eq!(fs::read_dir(cache).unwrap().count(), 1);
    }

    #[test]
    fn local_backup_includes_committed_wal_without_changing_source_or_pending_rows() {
        let root = tempfile::tempdir().unwrap();
        let cache = healthy_cache_fixture(root.path());
        let source = cache.join("mfb_store.sqlite");
        let writer = Connection::open(&source).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        writer.execute("INSERT INTO blob_store VALUES ('checkpoint', 'committed-wal', 2, NULL, ?1, ?2, 123)", params![b"wal state", blake3::hash(b"wal state").as_bytes()]).unwrap();
        writer.execute_batch("BEGIN").unwrap();
        writer
            .execute(
                "INSERT INTO blob_store VALUES ('checkpoint', 'uncommitted', 2, NULL, ?1, ?2, 123)",
                params![b"pending state", blake3::hash(b"pending state").as_bytes()],
            )
            .unwrap();
        let wal = cache.join("mfb_store.sqlite-wal");
        let before = fs::read(&source).unwrap();
        let wal_before = fs::read(&wal).unwrap();
        assert!(
            !wal_before.is_empty(),
            "fixture must retain committed WAL with the writer still open"
        );
        let destination = root.path().join("core-backup.sqlite");
        let receipt = backup_local_store(&cache, &destination).unwrap();
        assert_eq!(receipt.schema_version, 1);
        assert_eq!(receipt.bytes, fs::metadata(&destination).unwrap().len());
        assert_eq!(
            receipt.source,
            source.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            receipt.destination,
            destination.canonicalize().unwrap().to_str().unwrap()
        );
        let backup = open_cache_store(&destination, true).unwrap();
        assert_eq!(
            backup
                .query_row(
                    "SELECT COUNT(*) FROM blob_store WHERE cache_key='committed-wal'",
                    [],
                    |row| nonnegative_column(row, 0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            backup
                .query_row(
                    "SELECT COUNT(*) FROM blob_store WHERE cache_key='uncommitted'",
                    [],
                    |row| nonnegative_column(row, 0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            backup
                .query_row("SELECT COUNT(*) FROM blob_store", [], |row| {
                    nonnegative_column(row, 0)
                })
                .unwrap(),
            5
        );
        assert_eq!(fs::read(&source).unwrap(), before);
        assert_eq!(fs::read(&wal).unwrap(), wal_before);
        let saved = fs::read(&destination).unwrap();
        assert!(backup_local_store(&cache, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), saved);
        assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".mfb-core-backup-")
        }));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn core_backup_check_binds_moved_snapshot_bytes_and_validates_contents() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cache = healthy_cache_fixture(root.path());
        let destination = root.path().join("core-backup.sqlite");
        let receipt = backup_local_store(&cache, &destination)?;
        let moved = root.path().join("合成 ? # retained.sqlite");
        fs::rename(&destination, &moved)?;
        let snapshot = fs::read(&moved)?;
        assert_eq!(
            receipt.snapshot_blake3,
            blake3::hash(&snapshot).to_hex().as_str()
        );
        let proof = check_core_backup(&moved, &receipt.snapshot_blake3)?;
        assert_eq!(proof.snapshot, moved);
        assert_eq!(proof.bytes, receipt.bytes);
        assert_eq!(proof.store_schema_version, receipt.store_schema_version);
        assert_eq!(fs::read(&moved)?, snapshot);
        assert!(check_core_backup(&moved, "not-hex").is_err());
        #[cfg(unix)]
        {
            let alias = root.path().join("backup-alias.sqlite");
            std::os::unix::fs::symlink(&moved, &alias)?;
            assert!(check_core_backup(&alias, &receipt.snapshot_blake3).is_err());
            fs::remove_file(&alias)?;
            fs::hard_link(&moved, &alias)?;
            assert!(check_core_backup(&moved, &receipt.snapshot_blake3).is_err());
            assert_eq!(fs::read(&alias)?, snapshot);
            fs::remove_file(&alias)?;
        }

        let sidecar = root.path().join("合成 ? # retained.sqlite-wal");
        fs::write(&sidecar, b"synthetic sidecar")?;
        let error = check_core_backup(&moved, &receipt.snapshot_blake3).unwrap_err();
        assert!(error.to_string().contains("has a sidecar"));
        assert_eq!(fs::read(&sidecar)?, b"synthetic sidecar");
        fs::remove_file(sidecar)?;

        let writer = open_cache_store(&moved, false)?;
        writer.execute("UPDATE blob_store SET payload_blake3=zeroblob(32)", [])?;
        drop(writer);
        let corrupted = fs::read(&moved)?;
        assert_eq!(corrupted.len(), snapshot.len());
        let error = check_core_backup(&moved, &receipt.snapshot_blake3).unwrap_err();
        assert!(error.to_string().contains("does not match"));
        let error =
            check_core_backup(&moved, blake3::hash(&corrupted).to_hex().as_str()).unwrap_err();
        assert!(error.to_string().contains("integrity verification failed"));
        assert_eq!(fs::read(&moved)?, corrupted);
        sqlite_backup::require_snapshot_without_sidecars(&moved)?;
        Ok(())
    }

    #[test]
    fn core_backup_check_requires_the_digest_and_excludes_other_operations() {
        let args = [
            "cache_cleaner",
            "--check-core-backup",
            "/synthetic/core.sqlite",
            "--expected-backup-blake3",
            "0",
            "--json",
        ];
        assert!(Args::try_parse_from(args).is_ok());
        assert!(Args::try_parse_from(&args[..3]).is_err());
        assert!(Args::try_parse_from(["cache_cleaner", "--expected-backup-blake3", "0"]).is_err());
        for flags in [
            vec!["--local-backup", "/synthetic/new.sqlite"],
            vec!["--preview-core-recovery"],
            vec!["--migrate-core-state", "--yes"],
            vec!["--history-stats", "--history-directory", "/synthetic/logs"],
            vec!["--stats", "--check-integrity"],
            vec!["--prune"],
            vec!["--yes"],
            vec!["/synthetic/input"],
        ] {
            assert!(Args::try_parse_from(args.into_iter().chain(flags)).is_err());
        }
    }

    #[test]
    fn local_backup_rejects_missing_existing_sidecar_workspace_and_unhealthy_sources() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("backup.sqlite");
        let missing = root.path().join("missing-cache");
        assert!(backup_local_store(&missing, &destination).is_err());
        assert!(!missing.exists() && !destination.exists());
        let cache = healthy_cache_fixture(root.path());
        let source = cache.join("mfb_store.sqlite");
        let before = fs::read(&source).unwrap();
        let state = cache.with_file_name("state");
        fs::create_dir(&state).unwrap();
        for name in ["mfb_store.sqlite", ".core-state-relocation-active.json"] {
            let reserved = state.join(name);
            assert!(backup_local_store(&cache, &reserved).is_err());
            assert!(!reserved.exists());
        }
        assert_eq!(
            cache_status(&cache).unwrap().core_store_path.as_deref(),
            source.to_str()
        );
        for content in [b"".as_slice(), b"existing backup".as_slice()] {
            fs::write(&destination, content).unwrap();
            assert!(backup_local_store(&cache, &destination).is_err());
            assert_eq!(fs::read(&destination).unwrap(), content);
            fs::remove_file(&destination).unwrap();
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = root.path().join(format!("backup.sqlite{suffix}"));
            fs::write(&sidecar, b"retain sidecar").unwrap();
            assert!(backup_local_store(&cache, &destination).is_err());
            assert!(!destination.exists());
            assert_eq!(fs::read(&sidecar).unwrap(), b"retain sidecar");
            fs::remove_file(sidecar).unwrap();
        }
        assert!(backup_local_store(&cache, &cache.join("new.sqlite")).is_err());
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join(".git"), b"synthetic worktree marker").unwrap();
        assert!(backup_local_store(&cache, &workspace.join("backup.sqlite")).is_err());
        assert_eq!(fs::read(&source).unwrap(), before);
        let conn = Connection::open(&source).unwrap();
        conn.execute(
            "UPDATE blob_store SET payload_blake3 = zeroblob(32) WHERE namespace='checkpoint'",
            [],
        )
        .unwrap();
        drop(conn);
        let corrupt = fs::read(&source).unwrap();
        assert!(backup_local_store(&cache, &destination).is_err());
        assert_eq!(fs::read(&source).unwrap(), corrupt);
        assert!(!destination.exists());
        let conn = Connection::open(&source).unwrap();
        conn.execute("UPDATE store_metadata SET value = 99", [])
            .unwrap();
        drop(conn);
        assert!(backup_local_store(&cache, &destination).is_err());
        fs::write(&source, b"not a database").unwrap();
        assert!(backup_local_store(&cache, &destination).is_err());
        assert_eq!(fs::read(source).unwrap(), b"not a database");
        assert!(!destination.exists());
        assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".mfb-core-backup-")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn local_backup_rejects_symlink_source_sidecars_and_destination() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let cache = healthy_cache_fixture(root.path());
        let source = cache.join("mfb_store.sqlite");
        let outside = root.path().join("outside.sqlite");
        fs::copy(&source, &outside).unwrap();
        let destination = root.path().join("backup.sqlite");
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = cache.join(format!("mfb_store.sqlite{suffix}"));
            symlink(&outside, &sidecar).unwrap();
            assert!(backup_local_store(&cache, &destination).is_err());
            fs::remove_file(sidecar).unwrap();
        }
        symlink(&outside, &destination).unwrap();
        assert!(backup_local_store(&cache, &destination).is_err());
        fs::remove_file(&destination).unwrap();
        let alias = root.path().join("cache-alias");
        symlink(&cache, &alias).unwrap();
        assert!(backup_local_store(&alias, &destination).is_err());
        fs::remove_file(&source).unwrap();
        symlink(&outside, &source).unwrap();
        assert!(backup_local_store(&cache, &destination).is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read(outside).unwrap(), fs::read(source).unwrap());
    }

    #[test]
    fn core_recovery_cli_requires_a_new_destination_and_explicit_snapshot_confirmation() {
        let preview = ["cache_cleaner", "--preview-core-recovery", "--json"];
        assert!(Args::try_parse_from(preview).unwrap().preview_core_recovery);
        assert!(Args::try_parse_from(preview.into_iter().chain(["--yes"])).is_err());
        let export = [
            "cache_cleaner",
            "--export-core-recovery",
            "/synthetic/new",
            "--expected-snapshot-blake3",
            "synthetic-digest",
            "--yes",
        ];
        assert!(
            Args::try_parse_from(export)
                .unwrap()
                .export_core_recovery
                .is_some()
        );
        for incomplete in [&export[..3], &export[..5]] {
            assert!(Args::try_parse_from(incomplete).is_err());
        }
        assert!(
            Args::try_parse_from([
                "cache_cleaner",
                "--expected-snapshot-blake3",
                "synthetic-digest"
            ])
            .is_err()
        );
        for flags in [
            vec!["--preview-core-recovery"],
            vec!["--stats"],
            vec!["--preview-prune"],
            vec!["--migrate-core-state"],
            vec![
                "--history-stats",
                "--history-directory",
                "/synthetic/history",
            ],
            vec!["--local-backup", "/synthetic/backup.sqlite"],
            vec!["/synthetic/input"],
        ] {
            assert!(Args::try_parse_from(export.into_iter().chain(flags)).is_err());
        }
    }

    #[test]
    fn core_location_migration_requires_explicit_confirmation_and_is_exclusive() {
        let command = ["cache_cleaner", "--migrate-core-state", "--yes"];
        assert!(Args::try_parse_from(command).unwrap().migrate_core_state);
        assert!(Args::try_parse_from(&command[..2]).is_err());
        for flags in [
            vec!["--stats"],
            vec!["--prune"],
            vec![
                "--history-stats",
                "--history-directory",
                "/synthetic/history",
            ],
            vec!["--local-backup", "/synthetic/backup.sqlite"],
            vec!["/synthetic/input"],
        ] {
            assert!(Args::try_parse_from(command.into_iter().chain(flags)).is_err());
        }
    }

    #[test]
    fn local_backup_cli_is_independent_of_cleanup_and_inspection() {
        let args = Args::try_parse_from([
            "cache_cleaner",
            "--local-backup",
            "/synthetic/backup.sqlite",
            "--json",
        ])
        .unwrap();
        assert!(
            args.local_backup.is_some() && args.json && !args.stats && !args.prune && !args.yes
        );
        for incompatible in [
            "--stats",
            "--prune",
            "--check-integrity",
            "--postgres",
            "--purge-animation-cache",
            "--purge-session-state",
            "--yes",
            "--no-config",
            "/synthetic/input",
        ] {
            assert!(
                Args::try_parse_from([
                    "cache_cleaner",
                    "--local-backup",
                    "/synthetic/backup.sqlite",
                    incompatible
                ])
                .is_err()
            );
        }
        assert!(Args::try_parse_from(["cache_cleaner", "--local-backup"]).is_err());
    }

    #[test]
    fn history_preparation_cli_is_explicit_and_excludes_other_actions() {
        let arguments = [
            "cache_cleaner",
            "--history-prepare-readonly",
            "--history-directory",
            "/synthetic/history",
            "--json",
        ];
        let args = Args::try_parse_from(arguments).unwrap();
        assert!(args.history_prepare_readonly && args.json && !args.yes);
        assert!(Args::try_parse_from(["cache_cleaner", "--history-prepare-readonly"]).is_err());
        for flag in [
            "--history-stats",
            "--stats",
            "--prune",
            "--check-integrity",
            "--yes",
        ] {
            assert!(Args::try_parse_from(arguments.into_iter().chain([flag])).is_err());
        }
    }

    #[test]
    fn history_backup_cannot_occupy_core_storage() -> Result<()> {
        let Some(root) = fast_img_state_test_root() else {
            return Ok(());
        };
        let cache = backup_cache_directory()?;
        fs::create_dir_all(cache.with_file_name("state"))?;
        let history = root.join("synthetic-history");
        fs::create_dir(&history)?;
        let destination = cache.with_file_name("state").join("mfb_store.sqlite");
        let args = Args::try_parse_from([
            "cache_cleaner",
            "--history-backup",
            destination.to_str().unwrap(),
            "--history-directory",
            history.to_str().unwrap(),
            "--json",
        ])?;
        let error = run(args).expect_err("reserved Core path cannot receive a History snapshot");
        assert!(
            error
                .to_string()
                .contains("outside managed cache and state directories")
        );
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn history_backup_cli_requires_explicit_source_and_excludes_other_actions() {
        let base = [
            "cache_cleaner",
            "--history-backup",
            "/synthetic/backup.sqlite",
            "--history-directory",
            "/synthetic/history",
        ];
        let mut arguments = base.to_vec();
        arguments.push("--json");
        let args = Args::try_parse_from(arguments).unwrap();
        assert_eq!(
            args.history_directory.unwrap(),
            PathBuf::from("/synthetic/history")
        );
        assert!(args.history_backup.is_some() && args.json && !args.yes);
        assert!(
            Args::try_parse_from([
                "cache_cleaner",
                "--history-backup",
                "/synthetic/backup.sqlite"
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from(["cache_cleaner", "--history-directory", "/synthetic/history"])
                .is_err()
        );
        for additional in [
            vec!["--stats"],
            vec!["--prune"],
            vec!["--check-integrity"],
            vec!["--postgres"],
            vec!["--purge-animation-cache"],
            vec!["--purge-session-state"],
            vec!["--yes"],
            vec!["--no-config"],
            vec!["--config", "/synthetic/config.json"],
            vec!["--cache-max-bytes", "10"],
            vec!["--cache-ttl-seconds", "10"],
            vec!["--local-backup", "/synthetic/core.sqlite"],
            vec!["/synthetic/input"],
        ] {
            let mut arguments = base.to_vec();
            arguments.extend(additional);
            assert!(Args::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn local_cleanup_retains_resume_proofs_history_models_and_unknown_files() {
        let root = tempfile::tempdir().unwrap();
        let cache = cache_fixture(root.path());
        fs::create_dir(root.path().join("logs")).unwrap();
        dev::infra::history_store::append_history_event(
            &root.path().join("logs"),
            "run",
            "MFB_HISTORY_FINISHED={\"schema_version\":1,\"outcome\":\"completed\"}",
        )
        .unwrap();
        let retained = [
            cache.join("models/weights.bin"),
            cache.join("unknown.sqlite"),
            root.path().join("tmp/recovery.bin"),
            root.path().join("locks/active.lock"),
        ];
        for path in &retained {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"retained").unwrap();
        }
        fs::write(cache.join("image_analysis_v2.db"), b"obsolete").unwrap();
        fs::write(cache.join("image_analysis_v2.db-wal"), b"obsolete wal").unwrap();
        let history = root.path().join("logs/history.sqlite3");
        let history_before = fs::read(&history).unwrap();
        let before = cache_status(&cache).unwrap();
        assert_eq!(before.namespaces.len(), 4);
        assert_eq!(before.legacy_analysis_files, 2);
        assert_eq!(before.legacy_analysis_bytes, 20);
        assert_eq!(
            before
                .namespaces
                .iter()
                .filter(|row| row.rebuildable)
                .count(),
            1
        );
        let status = purge_local_cache(&cache, None).unwrap();
        assert_eq!((status.removed_rows, status.removed_files), (1, 2));
        assert_eq!(status.namespaces.len(), 3);
        let conn = Connection::open(cache.join("mfb_store.sqlite")).unwrap();
        let state: Vec<(String, Vec<u8>, Vec<u8>, i64)> = conn.prepare(
            "SELECT namespace, payload, payload_blake3, updated_at FROM blob_store ORDER BY namespace",
        ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(state.len(), 3);
        for (_, payload, proof, updated_at) in state {
            assert_eq!(payload, b"payload");
            assert_eq!(proof, [7u8; 32]);
            assert_eq!(updated_at, 123);
        }
        for path in retained {
            assert_eq!(fs::read(path).unwrap(), b"retained");
        }
        assert_eq!(fs::read(history).unwrap(), history_before);
        let again = purge_local_cache(&cache, None).unwrap();
        assert_eq!((again.removed_rows, again.removed_files), (0, 0));
    }

    #[test]
    fn gate_verification_cache_cleanup_preserves_other_tasks_and_formal_state() {
        let root = tempfile::tempdir().unwrap();
        let cache = cache_fixture(root.path());
        let target = root.path().join("source_100%");
        fs::create_dir(&target).unwrap();
        let target = target.canonicalize().unwrap();
        let store = cache.join("mfb_store.sqlite");
        let conn = Connection::open(&store).unwrap();
        for (key, source) in [
            ("one", target.clone()),
            ("two", root.path().join("other-source")),
        ] {
            conn.execute(
                "INSERT INTO blob_store VALUES (?1, ?2, 1, ?3, ?4, ?5, 123)",
                params![
                    foundation::mfb_sqlite_store::NS_GATE1_PROOF,
                    key,
                    source.to_str().unwrap(),
                    b"proof",
                    blake3::hash(b"proof").as_bytes()
                ],
            )
            .unwrap();
        }
        drop(conn);
        let before = cache_status(&cache).unwrap();
        assert!(before.namespaces.iter().any(|row| {
            row.name == foundation::mfb_sqlite_store::NS_GATE1_PROOF
                && row.rebuildable
                && row.rows == 2
        }));
        assert_eq!(
            purge_local_cache(&cache, Some(&target))
                .unwrap()
                .removed_rows,
            1
        );
        let conn = Connection::open(&store).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT cache_key FROM blob_store WHERE namespace=?1",
                [foundation::mfb_sqlite_store::NS_GATE1_PROOF],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "two"
        );
        drop(conn);
        assert_eq!(purge_local_cache(&cache, None).unwrap().removed_rows, 2);
        let after = cache_status(&cache).unwrap();
        assert!(after.namespaces.iter().all(|row| !row.rebuildable));
        assert_eq!(after.namespaces.len(), 3);
    }

    #[test]
    fn targeted_cache_cleanup_matches_literal_components_without_resetting_state() {
        let root = tempfile::tempdir().unwrap();
        let cache = cache_fixture(root.path());
        let target = root.path().join("photos_100%");
        fs::create_dir(&target).unwrap();
        let absolute = target.canonicalize().unwrap().to_str().unwrap().to_owned();
        let conn = Connection::open(cache.join("mfb_store.sqlite")).unwrap();
        for (key, path) in [
            ("exact", absolute.clone()),
            ("child", format!("{absolute}/child")),
            ("sibling", format!("{absolute}other")),
            ("wildcard", absolute.replace("_100%", "X1000")),
        ] {
            conn.execute(
                "INSERT INTO blob_store VALUES ('path_tree', ?1, 2, ?2, ?3, ?4, 123)",
                params![key, path, b"cache", &[3u8; 32]],
            )
            .unwrap();
        }
        drop(conn);
        let status = purge_local_cache(&cache, Some(&target)).unwrap();
        assert_eq!(status.removed_rows, 2);
        assert_eq!(
            status
                .namespaces
                .iter()
                .find(|row| row.name == "path_tree")
                .unwrap()
                .rows,
            3
        );
        assert_eq!(
            status
                .namespaces
                .iter()
                .filter(|row| !row.rebuildable)
                .count(),
            3
        );
    }

    #[test]
    fn cache_inspection_does_not_create_a_store_or_modify_existing_database() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("absent-cache");
        let empty = cache_status(&missing).unwrap();
        assert_eq!(empty.store_bytes, 0);
        assert!(empty.namespaces.is_empty() && !missing.exists());
        let cache = cache_fixture(root.path());
        let store = cache.join("mfb_store.sqlite");
        let before = fs::read(&store).unwrap();
        let stats = cache_status(&cache).unwrap();
        assert_eq!(stats.store_bytes, u64::try_from(before.len()).unwrap());
        assert_eq!(fs::read(store).unwrap(), before);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        let json = serde_json::to_value(stats).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["removed_rows"], 0);
    }

    #[test]
    fn unknown_or_corrupt_cache_schema_fails_before_any_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let cache = cache_fixture(root.path());
        let legacy = cache.join("image_analysis_v2.db");
        fs::write(&legacy, b"retain until validated").unwrap();
        let store = cache.join("mfb_store.sqlite");
        Connection::open(&store)
            .unwrap()
            .execute("UPDATE store_metadata SET value = 99", [])
            .unwrap();
        let before = fs::read(&store).unwrap();
        assert!(
            purge_local_cache(&cache, None)
                .unwrap_err()
                .to_string()
                .contains("unsupported cache store schema")
        );
        assert_eq!(fs::read(&store).unwrap(), before);
        fs::write(&store, b"not a sqlite database").unwrap();
        assert!(purge_local_cache(&cache, None).is_err());
        assert_eq!(fs::read(&store).unwrap(), b"not a sqlite database");
        assert_eq!(fs::read(legacy).unwrap(), b"retain until validated");
    }

    #[cfg(unix)]
    #[test]
    fn cache_cleanup_refuses_symlinked_managed_files_and_directories() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let cache = cache_fixture(root.path());
        let outside = root.path().join("outside.db");
        fs::write(&outside, b"do not touch").unwrap();
        symlink(&outside, cache.join("image_analysis_v2.db")).unwrap();
        assert!(purge_local_cache(&cache, None).is_err());
        assert!(
            cache_status(&cache)
                .unwrap_err()
                .to_string()
                .contains("non-regular")
        );
        assert_eq!(fs::read(&outside).unwrap(), b"do not touch");
        let alias = root.path().join("cache-alias");
        symlink(&cache, &alias).unwrap();
        assert!(purge_local_cache(&alias, None).is_err());
        fs::remove_file(cache.join("image_analysis_v2.db")).unwrap();
        fs::remove_file(cache.join("mfb_store.sqlite")).unwrap();
        symlink(&outside, cache.join("mfb_store.sqlite")).unwrap();
        assert!(purge_local_cache(&cache, None).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"do not touch");
    }

    #[cfg(unix)]
    #[test]
    fn hardlinked_managed_cache_files_are_preserved_before_maintenance() -> Result<()> {
        for managed in [
            "mfb_store.sqlite",
            "mfb_store.sqlite-wal",
            "mfb_store.sqlite-shm",
            "mfb_store.sqlite-journal",
            "image_analysis_v2.db",
            "image_analysis_v2_main.db-wal",
        ] {
            let root = tempfile::tempdir()?;
            let cache = cache_fixture(root.path());
            let database = cache.join("mfb_store.sqlite");
            let path = cache.join(managed);
            let outside = root.path().join("preserved-recovery-file");
            if !path.exists() {
                fs::write(&path, b"synthetic recovery evidence")?;
            }
            fs::hard_link(&path, &outside)?;
            let before_database = fs::read(&database)?;
            let before_linked = fs::read(&outside)?;

            let integrity = cache_status_with_integrity(&cache, true)?
                .integrity
                .context("missing integrity report")?;
            assert_eq!(integrity.status, "error", "{managed}");
            assert!(
                integrity
                    .issues
                    .iter()
                    .any(|issue| issue.code == "managed_paths"),
                "{managed}"
            );
            for error in [
                cache_status(&cache).unwrap_err(),
                purge_local_cache(&cache, None).unwrap_err(),
                prune_local_cache(&cache, foundation::runtime_config::CachePolicy::default())
                    .unwrap_err(),
            ] {
                assert!(
                    format!("{error:#}").contains("multiple hard links"),
                    "{managed}: {error:#}"
                );
            }
            if managed.starts_with("mfb_store.sqlite") {
                let destination = root.path().join("backup.sqlite");
                let error = backup_local_store(&cache, &destination).unwrap_err();
                assert!(
                    format!("{error:#}").contains("multiple hard links"),
                    "{managed}: {error:#}"
                );
                assert!(!destination.exists());
            }
            assert_eq!(fs::read(&database)?, before_database, "{managed}");
            assert_eq!(fs::read(path)?, before_linked, "{managed}");
            assert_eq!(fs::read(outside)?, before_linked, "{managed}");
        }
        Ok(())
    }

    #[test]
    fn test_parse_row_count() {
        assert_eq!(parse_row_count("DELETE 5").unwrap(), 5);
        assert_eq!(parse_row_count("INSERT 0 10").unwrap(), 10);
        assert_eq!(parse_row_count("TRUNCATE TABLE").unwrap(), 0);
        assert_eq!(parse_row_count("DELETE 2\nDELETE 3").unwrap(), 5);
        assert_eq!(parse_row_count("DELETE 4294967296").unwrap(), 4_294_967_296);
        assert!(parse_row_count("DELETE -1").is_err());
        assert!(parse_row_count("DELETE").is_err());
        assert!(parse_row_count("").is_err());
        assert!(parse_row_count("unexpected command output").is_err());
        assert!(parse_row_count("DELETE 18446744073709551615\nDELETE 1").is_err());
    }

    #[test]
    fn postgres_paths_preserve_quotes_backslashes_and_wildcard_characters() {
        assert_eq!(
            postgres_path_literal(r"C:\media\it's 100%_"),
            r"E'C:\\media\\it''s 100%_'"
        );
    }

    #[test]
    fn test_cache_inspection_and_purge_modes_are_explicit() {
        let inspect = Args::try_parse_from(["cache_cleaner", "--stats", "--json"]).unwrap();
        assert!(inspect.stats && inspect.json && !inspect.postgres && !inspect.yes);
        assert!(
            Args::try_parse_from(["cache_cleaner", "--stats", "--purge-session-state"]).is_err()
        );
        assert!(Args::try_parse_from(["cache_cleaner", "--json", "--postgres"]).is_err());
        assert!(Args::try_parse_from(["cache_cleaner", "--check-integrity"]).is_err());
        assert!(
            Args::try_parse_from(["cache_cleaner", "--stats", "--check-integrity", "--json"])
                .unwrap()
                .check_integrity
        );
        for incompatible in [
            "--yes",
            "--postgres",
            "--purge-session-state",
            "--purge-animation-cache",
            "/synthetic",
        ] {
            assert!(
                Args::try_parse_from([
                    "cache_cleaner",
                    "--stats",
                    "--check-integrity",
                    incompatible
                ])
                .is_err()
            );
        }
        assert!(confirm_cleanup(true, "synthetic").unwrap());
    }

    #[test]
    fn integrity_counts_all_corrupt_rows_but_bounds_details_without_payloads() {
        let root = tempfile::tempdir().unwrap();
        let cache = cache_fixture(root.path());
        let store = cache.join("mfb_store.sqlite");
        let conn = Connection::open(&store).unwrap();
        for index in 0..30 {
            conn.execute(
                "INSERT INTO blob_store VALUES ('path_tree', ?1, 2, NULL, ?2, ?3, 123)",
                params![
                    index.to_string(),
                    b"private payload never print",
                    &[0u8; 31]
                ],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE blob_store SET payload = 'wrong storage type' WHERE namespace = 'checkpoint'",
            [],
        )
        .unwrap();
        drop(conn);
        let before = fs::read(&store).unwrap();
        let status = cache_status_with_integrity(&cache, true).unwrap();
        let json = serde_json::to_string(&status).unwrap();
        let report = status.integrity.unwrap();
        assert_eq!(report.status, "unhealthy");
        assert_eq!(report.checked_cache_rows, 31);
        assert_eq!(report.corrupt_cache_rows, 31);
        assert_eq!(report.checked_protected_rows, 3);
        assert_eq!(report.corrupt_protected_rows, 3);
        assert_eq!(report.issue_count, 34);
        assert_eq!(report.issues.len(), 20);
        assert!(report.blob_scan_complete);
        assert!(!json.contains("private payload"));
        assert!(!json.contains("wrong storage type"));
        assert_eq!(fs::read(store).unwrap(), before);
    }

    #[test]
    fn legacy_session_purge_refuses_bulk_removal_before_any_state_access() -> Result<()> {
        const CHILD: &str = "MFB_LEGACY_SESSION_PURGE_TEST";
        let thread = std::thread::current();
        let name = thread.name().context("session purge test name")?;
        if std::env::var(CHILD).as_deref() != Ok(name) {
            let output = Command::new(std::env::current_exe()?)
                .args([name, "--exact", "--nocapture", "--test-threads=1"])
                .env(CHILD, name)
                .output()?;
            anyhow::ensure!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }

        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let state = root.join("state");
        let logs = state.join("logs");
        let missing_state = root.join("uninitialized-state");
        let paths = [
            state.join("tmp/leftover.search.random.JXL"),
            state.join("tmp/recovery.log"),
            root.join(".mfb_progress/recovery.json"),
            state.join("locks/unowned.lock"),
            logs.join("Bundle_unowned/diagnostic.log"),
            logs.join("session.log"),
        ];
        for path in &paths {
            fs::create_dir_all(path.parent().context("synthetic fixture parent")?)?;
            fs::write(path, b"keep")?;
        }
        let _home = foundation::common_utils::EnvGuard::set(
            foundation::constants::ENV_HOME,
            root.to_str().context("synthetic home")?,
        );
        let _logs = foundation::common_utils::EnvGuard::set(
            foundation::constants::ENV_MFB_LOG_DIR,
            logs.to_str().context("synthetic logs")?,
        );
        for configured in [&state, &missing_state] {
            let _state = foundation::common_utils::EnvGuard::set(
                foundation::constants::ENV_MFB_HOME_ROOT,
                configured.to_str().context("synthetic state")?,
            );
            let args = Args::try_parse_from(["cache_cleaner", "--purge-session-state", "--yes"])?;
            let error = run(args).unwrap_err();
            assert!(
                error
                    .to_string()
                    .starts_with("MFB_SESSION_CLEANUP_REQUIRES_HISTORY_SELECTION:")
            );
        }
        assert!(!missing_state.exists());
        for path in paths {
            assert_eq!(fs::read(path)?, b"keep");
        }
        Ok(())
    }
}
