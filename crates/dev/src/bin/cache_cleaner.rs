//! Modern Format Boost - Cache Cleaner in Rust.
//! Clears conversion/analysis caches scoped by target or full purges.

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use dev::infra::sqlite_backup;
use dev::infra::ui_tokens::pick_symbol;
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use std::collections::HashSet;
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

    #[arg(long, conflicts_with_all = ["stats", "history_stats", "prune", "preview_prune", "check_integrity", "path", "postgres", "purge_animation_cache", "purge_session_state", "yes", "config", "no_config"], help = "Create a verified snapshot of Core SQLite state and cache at a new local destination; History and PostgreSQL are not included")]
    local_backup: Option<PathBuf>,

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
        help = "Remove session logs, progress trackers, temp files, and stale locks"
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
    // Backup reads an existing root; runtime's directory-creating fallback is intentionally not used.
    let root = match std::env::var(foundation::constants::ENV_MFB_HOME_ROOT) {
        Ok(root) => PathBuf::from(root),
        Err(std::env::VarError::NotPresent) => {
            let home = match std::env::var(foundation::constants::ENV_HOME) {
                Ok(home) => home,
                Err(std::env::VarError::NotPresent) => {
                    std::env::var(foundation::constants::ENV_USERPROFILE)
                        .context("backup source requires HOME or USERPROFILE")?
                }
                Err(error) => return Err(error).context("read HOME for backup source"),
            };
            let home = PathBuf::from(home);
            anyhow::ensure!(
                home.is_absolute(),
                "backup source home must be absolute; no fallback selected"
            );
            home.join(".modern_format_boost")
        }
        Err(error) => {
            return Err(error)
                .context("read MFB_HOME_ROOT for backup source; no fallback selected");
        }
    };
    anyhow::ensure!(
        root.is_absolute(),
        "backup source root must be absolute; no fallback selected"
    );
    Ok(root.join("cache"))
}

fn get_mfb_progress_root() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".mfb_progress"))
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

fn get_project_root() -> Result<PathBuf> {
    let exe_path = std::env::current_exe()?;
    let mut dir = exe_path.parent();
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() && d.join("crates").is_dir() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    // Fallback to current working directory
    let cwd = std::env::current_dir()?;
    Ok(cwd)
}

fn is_lock_stale(path: &Path) -> bool {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => {
            let fd = file.as_raw_fd();
            unsafe {
                if libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) == 0 {
                    let _ = libc::flock(fd, libc::LOCK_UN);
                    true
                } else {
                    false
                }
            }
        }
        Err(_err) => false,
    }
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
    pruning_preview: Option<foundation::mfb_sqlite_store::CachePrunePreview>,
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
            2 | foundation::mfb_sqlite_store::STORE_SCHEMA_VERSION
        ),
        "unsupported cache store schema {version}; database retained"
    );
    // The legacy v2 store already uses BLAKE3; reading or backing it up must not
    // migrate it or claim it has the new protected tables. CRC32 v1 is not this
    // reader's payload format.
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

fn database_overview(conn: &Connection) -> Result<DatabaseOverview> {
    let overview = DatabaseOverview {
        store_schema_version: cache_store_schema_version(conn)?,
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
) -> Result<()> {
    *phase = "sqlite_integrity";
    let mut statement = conn.prepare("PRAGMA integrity_check")?;
    let mut rows = statement.query([])?;
    integrity.sqlite_integrity = "ok";
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
    integrity.foreign_keys = "ok";
    while rows.next()?.is_some() {
        integrity.foreign_keys = "failed";
        integrity.issue("foreign_key", "SQLite reported a foreign key violation.");
    }
    *phase = "blob_hashes";
    let mut statement =
        conn.prepare("SELECT namespace = ?1, payload, payload_blake3 FROM blob_store")?;
    let mut rows = statement.query([foundation::mfb_sqlite_store::NS_PATH_TREE])?;
    while let Some(row) = rows.next()? {
        let rebuildable = row.get::<_, Option<bool>>(0)?.unwrap_or(false);
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
    *phase = "fast_img_state";
    let fast_img_rows = u64::try_from(foundation::fast_img_store::inspect_integrity(conn)?)
        .context("Fast IMG protected row count exceeds u64")?;
    integrity.checked_protected_rows = integrity
        .checked_protected_rows
        .checked_add(fast_img_rows)
        .context("protected state row count overflow")?;
    integrity.checked_fast_img_rows = fast_img_rows;
    integrity.fast_img_scan_complete = true;
    integrity.status = if integrity.issue_count == 0 {
        "healthy"
    } else {
        "unhealthy"
    };
    Ok(())
}

fn require_healthy_store(conn: &mut Connection) -> Result<()> {
    let snapshot = conn.transaction()?;
    validate_cache_schema(&snapshot)?;
    database_overview(&snapshot)?;
    let mut integrity = CacheIntegrity::new();
    inspect_store_integrity(&snapshot, &mut integrity, &mut "sqlite_integrity")?;
    anyhow::ensure!(
        integrity.status == "healthy"
            && integrity.blob_scan_complete
            && integrity.fast_img_scan_complete,
        "Core database integrity verification failed; backup not published"
    );
    snapshot.commit()?;
    Ok(())
}

fn backup_local_store(cache_dir: &Path, destination: &Path) -> Result<BackupReceipt> {
    ensure_cache_directory(cache_dir)?;
    let source = cache_dir.join("mfb_store.sqlite");
    for suffix in ["", "-wal", "-shm", "-journal"] {
        regular_file_size(&cache_dir.join(format!("mfb_store.sqlite{suffix}")))?;
    }
    anyhow::ensure!(
        regular_file_size(&source)?.is_some(),
        "Core database is absent; no backup created"
    );
    let source = cache_dir.canonicalize()?.join("mfb_store.sqlite");
    let destination = sqlite_backup::resolve_destination(&source, destination)?;
    let parent = destination
        .parent()
        .context("backup destination has no parent")?;
    let mut conn = open_cache_store(&source, true)?;
    require_healthy_store(&mut conn)?;
    let store_schema_version = cache_store_schema_version(&conn)?;
    let staged = tempfile::Builder::new()
        .prefix(".mfb-core-backup-")
        .tempdir_in(parent)?;
    let backup = staged.path().join("mfb_store.sqlite");
    // The main database remains READ_ONLY; query_only would also block SQLite's new output database.
    conn.pragma_update(None, "query_only", false)?;
    conn.execute(
        "VACUUM main INTO ?1",
        [backup
            .to_str()
            .context("backup staging path is not UTF-8")?],
    )?;
    drop(conn);
    let mut verified = open_cache_store(&backup, true)?;
    require_healthy_store(&mut verified)?;
    anyhow::ensure!(
        cache_store_schema_version(&verified)? == store_schema_version,
        "Core database backup schema differs from its source; backup not published"
    );
    drop(verified);
    let bytes = sqlite_backup::synchronize_private_snapshot(&backup)?;
    let receipt = BackupReceipt {
        schema_version: 1,
        source: source
            .to_str()
            .context("Core database path is not UTF-8")?
            .to_owned(),
        destination: destination
            .to_str()
            .context("backup destination path is not UTF-8")?
            .to_owned(),
        store_schema_version,
        bytes,
    };
    sqlite_backup::publish_snapshot(&backup, &destination)?;
    Ok(receipt)
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
    // These are the actual explicit evidence BLOBs. Relational columns are not
    // described as payload bytes; store_bytes remains the physical store size.
    let payload_bytes = conn.query_row(
        "SELECT COALESCE((SELECT SUM(length(header_json)) FROM fast_img_tasks),0)
              + COALESCE((SELECT SUM(length(source_json)) FROM fast_img_migrations),0)",
        [],
        |row| nonnegative_column(row, 0),
    )?;
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
        store_bytes: 0,
        namespaces: Vec::new(),
        legacy_analysis_bytes: 0,
        legacy_analysis_files: 0,
        removed_rows: 0,
        removed_files: 0,
        integrity: check_integrity.then(CacheIntegrity::new),
        pruning: None,
        database: None,
        pruning_preview: None,
    };
    let mut phase = "managed_paths";
    let inspection = (|| -> Result<()> {
        ensure_cache_directory(cache_dir)?;
        for suffix in ["", "-wal", "-shm", "-journal"] {
            if let Some(bytes) =
                regular_file_size(&cache_dir.join(format!("mfb_store.sqlite{suffix}")))?
            {
                status.store_bytes = status
                    .store_bytes
                    .checked_add(bytes)
                    .context("cache store size overflow")?;
            }
        }
        let store = cache_dir.join("mfb_store.sqlite");
        if regular_file_size(&store)?.is_some() {
            phase = "store_open";
            let mut conn = open_cache_connection(&store, true)?;
            let snapshot = conn.transaction()?;
            phase = "schema";
            validate_cache_schema(&snapshot)?;
            phase = "database_overview";
            status.database = Some(database_overview(&snapshot)?);
            if let Some(policy) = preview {
                status.pruning_preview = Some(
                    foundation::mfb_sqlite_store::preview_path_tree_pruning(&snapshot, policy)?,
                );
            }
            let fast_img_rows = if let Some(integrity) = &mut status.integrity {
                inspect_store_integrity(&snapshot, integrity, &mut phase)?;
                integrity.checked_fast_img_rows
            } else {
                phase = "fast_img_state";
                u64::try_from(foundation::fast_img_store::inspect_row_count(&snapshot)?)
                    .context("Fast IMG protected row count exceeds u64")?
            };
            phase = "namespace_statistics";
            {
                let mut statement = snapshot.prepare(
                "SELECT namespace, COUNT(*), SUM(length(payload)) FROM blob_store GROUP BY namespace ORDER BY namespace",
            )?;
                status.namespaces = statement
                    .query_map([], |row| {
                        let name: String = row.get(0)?;
                        Ok(CacheNamespace {
                            rebuildable: name == foundation::mfb_sqlite_store::NS_PATH_TREE,
                            name,
                            rows: nonnegative_column(row, 1)?,
                            payload_bytes: nonnegative_column(row, 2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
            }
            add_fast_img_state_namespace(&snapshot, &mut status.namespaces, fast_img_rows)?;
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
                "managed_paths" => "A managed path is unreadable or is not a regular file or directory; inspection could not complete.",
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

fn purge_local_cache(cache_dir: &Path, target: Option<&Path>) -> Result<CacheStatus> {
    // Validate the entire managed scope before deleting; unknown namespaces/files are state, not cache.
    cache_status(cache_dir)?;
    let store = cache_dir.join("mfb_store.sqlite");
    let mut removed_rows = 0;
    if regular_file_size(&store)?.is_some() {
        let mut conn = open_cache_store(&store, false)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rows = if let Some(target) = target {
            let absolute = target.canonicalize()?;
            let absolute = absolute
                .to_str()
                .context("target path is not valid UTF-8")?;
            let prefix = format!("{}/", absolute.trim_end_matches('/'));
            tx.execute(
                "DELETE FROM blob_store WHERE namespace = ?1 AND (root_path = ?2 OR substr(root_path, 1, length(?3)) = ?3)",
                params![foundation::mfb_sqlite_store::NS_PATH_TREE, absolute, prefix],
            )?
        } else {
            tx.execute(
                "DELETE FROM blob_store WHERE namespace = ?1",
                [foundation::mfb_sqlite_store::NS_PATH_TREE],
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
    cache_status(cache_dir)?;
    let store = cache_dir.join("mfb_store.sqlite");
    let removed = if regular_file_size(&store)?.is_some() {
        let mut conn = open_cache_store(&store, false)?;
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

fn purge_conversion_resume_state(
    progress_dir: &Path,
    tmp_dir: &Path,
    lock_dir: &Path,
) -> Result<()> {
    if progress_dir.is_dir() {
        println!("{DIM}   Removing MFB progress directory...{RESET}");
        fs::remove_dir_all(progress_dir)?;
        println!("   {GREEN} MFB progress purged");
    }

    if tmp_dir.is_dir() {
        println!("{DIM}   Purging isolated temp directory...{RESET}");
        fs::remove_dir_all(tmp_dir)?;
        fs::create_dir_all(tmp_dir)?;
        println!("   {GREEN} Isolated temp space cleared");
    }

    if lock_dir.is_dir() {
        println!("{DIM}   Scanning for stale session locks...{RESET}");
        let mut deleted_locks = 0;
        let mut active_locks = 0;

        for entry in fs::read_dir(lock_dir)? {
            let path = entry?.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("lock") {
                if is_lock_stale(&path) {
                    fs::remove_file(&path)?;
                    deleted_locks += 1;
                } else {
                    active_locks += 1;
                }
            }
        }

        if deleted_locks > 0 {
            println!("   {GREEN} {deleted_locks} stale locks purged");
        }
        if active_locks > 0 {
            println!("   {YELLOW} {active_locks} active sessions skipped (protected)");
        }
    }

    Ok(())
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

fn get_dir_size(path: &Path) -> Result<String> {
    if path.is_file() {
        return Ok(format_size(fs::metadata(path)?.len()));
    }
    if !path.is_dir() {
        return Err(anyhow!(
            "size probe target does not exist: {}",
            path.display()
        ));
    }
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry.with_context(|| format!("walk size probe {}", path.display()))?;
        if entry.file_type().is_file() {
            let len = entry
                .metadata()
                .with_context(|| format!("read size metadata {}", entry.path().display()))?
                .len();
            total = total
                .checked_add(len)
                .ok_or_else(|| anyhow!("size probe overflow while scanning {}", path.display()))?;
        }
    }
    Ok(format_size(total))
}

fn display_size_or_na(path: &Path, label: &str) -> String {
    match get_dir_size(path) {
        Ok(size) => size,
        Err(err) => {
            eprintln!("  {YELLOW}WARN:{RESET} size probe failed for {label}: {err}");
            "N/A".to_string()
        }
    }
}

fn is_training_lane_log_dir(target: &Path) -> bool {
    let protected: HashSet<&str> = [
        "static_high",
        "static_low",
        "loop_high",
        "loop_low",
        "static",
        "all_high",
        "loop",
        "loop_video",
    ]
    .iter()
    .copied()
    .collect();
    if let Some(name) = target.file_name().and_then(|f| f.to_str()) {
        protected.contains(name)
    } else {
        false
    }
}

fn purge_log_dir_session_artifacts(target: &Path) -> Result<(i32, i32)> {
    let mut removed_logs = 0;
    let mut removed_dirs = 0;
    if !target.is_dir() || is_training_lane_log_dir(target) {
        return Ok((0, 0));
    }

    for entry in fs::read_dir(target)? {
        let path = entry?.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if ext == "log" || ext == "jsonl" {
                    remove_session_file(&path, &mut removed_logs)?;
                }
            } else if let Some(name) = path.file_name().and_then(|f| f.to_str())
                && ((name.starts_with("diagnostic_report_") && name.ends_with(".txt"))
                    || name == "deleted_offending_files.txt")
            {
                remove_session_file(&path, &mut removed_logs)?;
            }
        } else if path.is_dir()
            && let Some(name) = path.file_name().and_then(|f| f.to_str())
            && (name.starts_with("Bundle_") || name == "dev_verify")
        {
            fs::remove_dir_all(&path)
                .with_context(|| format!("remove session artifacts {}", path.display()))?;
            removed_dirs += 1;
        }
    }

    Ok((removed_logs, removed_dirs))
}

fn remove_session_file(path: &Path, removed_logs: &mut i32) -> Result<()> {
    fs::remove_file(path).with_context(|| format!("remove session artifact {}", path.display()))?;
    *removed_logs = removed_logs
        .checked_add(1)
        .context("session removal count overflow")?;
    Ok(())
}

fn purge_session_logs_only(log_dir: &Path) -> Result<()> {
    if !log_dir.is_dir() {
        return Ok(());
    }
    println!(
        "{}   Clearing conversion session logs from {} (training lanes preserved)...{}",
        DIM,
        log_dir.display(),
        RESET
    );
    let (removed_logs, removed_dirs) = purge_log_dir_session_artifacts(log_dir)?;
    println!("   {GREEN} Session logs cleared ({removed_logs} files, {removed_dirs} directories)");
    Ok(())
}

fn show_stats(
    cache_dir: &Path,
    db_file: &Path,
    log_dir: &Path,
    mfb_progress_dir: &Path,
) -> Result<()> {
    println!("{BOLD}Current Cache Status:{RESET}");

    if cache_dir.is_dir() {
        let size = display_size_or_na(cache_dir, "cache directory");
        println!(
            "   {} Directory: {}{}{}",
            pick_symbol("📂", "[DIR]"),
            DIM,
            cache_dir.display(),
            RESET
        );
        println!(
            "   {} Total Size: {}{}{}{}",
            pick_symbol("📦", "[PKG]"),
            BOLD,
            GREEN,
            size,
            RESET
        );

        if db_file.is_file() {
            let db_size = display_size_or_na(db_file, "cache database");
            println!(
                "   {}  Database:  {}{}{} ({})",
                pick_symbol("🗄️", "[DB]"),
                DIM,
                db_file
                    .file_name()
                    .unwrap_or(std::ffi::OsStr::new(""))
                    .to_string_lossy(),
                RESET,
                db_size
            );
        }
    } else {
        println!("   {YELLOW}Empty: No cache directory found.{RESET}");
    }

    let log_size = if log_dir.is_dir() {
        display_size_or_na(log_dir, "log directory")
    } else {
        "N/A".to_string()
    };
    println!(
        "   {} Logs:      {}{}{}",
        pick_symbol("📝", "[LOG]"),
        DIM,
        log_size,
        RESET
    );

    if mfb_progress_dir.is_dir() {
        let prog_size = display_size_or_na(mfb_progress_dir, "progress directory");
        println!(
            "   {} Progress:  {}{}{}",
            pick_symbol("🔄", "~"),
            DIM,
            prog_size,
            RESET
        );
    }

    let project_root = get_project_root()?;
    let target_dir = project_root.join("target");
    if target_dir.is_dir() {
        let target_size = display_size_or_na(&target_dir, "Rust build directory");
        println!(
            "   {} Rust Build: {}{}{}{}",
            pick_symbol("🦀", "[RUST]"),
            BOLD,
            YELLOW,
            target_size,
            RESET
        );
    }

    let local_cache = project_root.join(".cache");
    if local_cache.is_dir() {
        let local_size = display_size_or_na(&local_cache, "runtime cache directory");
        println!(
            "   {} Runtime:    {}{}{}{}",
            pick_symbol("⚡", "[FAST]"),
            BOLD,
            YELLOW,
            local_size,
            RESET
        );
    }

    let lock_dir = get_mfb_state_root()?.join("locks");
    if lock_dir.is_dir() {
        let mut lock_count = 0;
        for entry in fs::read_dir(&lock_dir)? {
            match entry {
                Ok(e) => {
                    if e.path().extension().and_then(|ex| ex.to_str()) == Some("lock") {
                        lock_count += 1;
                    }
                }
                Err(_err) => {}
            }
        }
        if lock_count > 0 {
            println!(
                "   {} Session Locks: {}{}{} active/stale",
                pick_symbol("🔒", "[LOCK]"),
                BOLD,
                YELLOW,
                lock_count
            );
        }
    }
    println!();
    Ok(())
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

fn perform_session_state_cleanup(yes: bool) -> Result<()> {
    let state_root = get_mfb_state_root()?;
    let cache_dir = state_root.join("cache");
    let log_dir = dev::infra::log_paths::unified_log_dir();
    let progress_dir = get_mfb_progress_root()?;
    let tmp_dir = state_root.join("tmp");
    let lock_dir = state_root.join("locks");
    let store_file = cache_dir.join("mfb_store.sqlite");

    draw_header(true);
    show_stats(&cache_dir, &store_file, &log_dir, &progress_dir)?;
    println!(
        "   {BOLD}Target:{RESET} {DIM}session state only (logs, progress, temp, stale \
         locks){RESET}\n"
    );

    if !confirm_cleanup(
        yes,
        "Delete session diagnostics, filesystem progress and temporary state?",
    )? {
        return Ok(());
    }

    purge_session_logs_only(&log_dir)?;
    purge_conversion_resume_state(&progress_dir, &tmp_dir, &lock_dir)?;
    println!("\n{GREEN} Session-State Cleanup Complete\n");
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
        println!(
            "Store on disk (includes retained state): {}",
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
    let args = Args::parse();
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
        let request: dev::infra::history_store::HistoryCleanupRequest =
            serde_json::from_str(encoded).context("invalid History cleanup request")?;
        let directory = args
            .history_directory
            .as_deref()
            .context("History cleanup requires its explicit directory")?;
        if args.history_cleanup_preview.is_some() {
            let preview = dev::infra::history_store::preview_history_cleanup(directory, &request)?;
            println!("{}", serde_json::to_string(&preview)?);
        } else {
            let receipt = dev::infra::history_store::execute_history_cleanup(directory, &request)?;
            println!("{}", serde_json::to_string(&receipt)?);
        }
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
                if overview.history_schema_version == 2 && overview.output_rows.is_none() {
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
        let receipt = dev::infra::history_store::backup_history_store(directory, destination)?;
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
    if let Some(destination) = &args.local_backup {
        let receipt = backup_local_store(&backup_cache_directory()?, destination)?;
        if args.json {
            println!("{}", serde_json::to_string(&receipt)?);
        } else {
            println!(
                "Core SQLite backup: {} -> {} ({} bytes, store schema {}). History and PostgreSQL are not included.",
                receipt.source, receipt.destination, receipt.bytes, receipt.store_schema_version
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
                    println!(
                        "Core database is absent; no database created and no pruning performed."
                    );
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

    if args.purge_session_state {
        perform_session_state_cleanup(args.yes)?;
        return Ok(());
    }

    let cache_dir = get_mfb_state_root()?.join("cache");
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
        let conn = rusqlite::Connection::open(root.path().join("mfb_store.sqlite")).unwrap();
        conn.execute_batch("CREATE TABLE store_metadata(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
            INSERT INTO store_metadata VALUES ('schema_version',2);
            CREATE TABLE blob_store(namespace TEXT NOT NULL,cache_key TEXT NOT NULL,schema_version INTEGER NOT NULL,root_path TEXT,payload BLOB NOT NULL,payload_blake3 BLOB NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(namespace,cache_key));
            CREATE INDEX idx_blob_ns_root ON blob_store(namespace,root_path);
            CREATE INDEX idx_blob_ns_updated ON blob_store(namespace,updated_at);
            INSERT INTO blob_store VALUES ('path_tree','one',1,NULL,X'6361636865',zeroblob(32),0),('checkpoint','state',1,NULL,X'70726F746563746564',zeroblob(32),0);").unwrap();
        drop(conn);
        let source = root.path().join("mfb_store.sqlite");
        let before = std::fs::read(&source).unwrap();
        let policy = foundation::runtime_config::CachePolicy {
            path_tree_max_bytes: 1,
            path_tree_ttl_seconds: 10,
        };
        let status = super::cache_status_with_options(root.path(), false, Some(policy)).unwrap();
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
        let absent =
            super::cache_status_with_options(&root.path().join("absent"), false, Some(policy))
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
        write_marker_atomic(&marker).unwrap();
        let store = foundation::mfb_sqlite_store::default_store_path().unwrap();
        let connection = Connection::open(&store).unwrap();
        (store.parent().unwrap().to_path_buf(), connection, marker)
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
        assert_eq!(receipt.store_schema_version, 3);
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
    fn local_backup_rejects_missing_existing_sidecar_workspace_and_unhealthy_sources() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("backup.sqlite");
        let missing = root.path().join("missing-cache");
        assert!(backup_local_store(&missing, &destination).is_err());
        assert!(!missing.exists() && !destination.exists());
        let cache = healthy_cache_fixture(root.path());
        let source = cache.join("mfb_store.sqlite");
        let before = fs::read(&source).unwrap();
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
    fn test_get_dir_size_uses_rust_filesystem_walk() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::create_dir(tempdir.path().join("nested")).unwrap();
        fs::write(tempdir.path().join("one.bin"), [0u8; 600]).unwrap();
        fs::write(tempdir.path().join("nested").join("two.bin"), [1u8; 424]).unwrap();

        assert_eq!(get_dir_size(tempdir.path()).unwrap(), "1.0 KB");
    }

    #[test]
    fn test_get_dir_size_missing_path_is_error() {
        let tempdir = tempfile::tempdir().unwrap();
        let missing = tempdir.path().join("missing-cache-dir");

        let err = get_dir_size(&missing).unwrap_err();
        assert!(err.to_string().contains("size probe target does not exist"));
    }

    #[test]
    fn test_is_lock_stale() {
        let tempdir = tempfile::tempdir().unwrap();
        let lock_file = tempdir.path().join("test.lock");

        // If lock file doesn't exist or isn't locked, is_lock_stale should return false
        // or handle gracefully Wait, is_lock_stale attempts to open the file
        // read/write, which fails if the file doesn't exist:
        assert!(!is_lock_stale(&lock_file));

        // Create the file:
        fs::write(&lock_file, b"").unwrap();
        // Without an active lock, opening and locking succeeds, so flock returns 0, and
        // unlocks, returning true (stale):
        assert!(is_lock_stale(&lock_file));
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
    fn test_purge_log_dir_session_artifacts_skips_training_lane_dirs() {
        let tempdir = tempfile::tempdir().unwrap();
        let lane_dir = tempdir.path().join("static_high");
        fs::create_dir(&lane_dir).unwrap();
        let log_file = lane_dir.join("run_training_20260608_055626.log");
        let audit_file = lane_dir.join("training_session_audit.jsonl");
        fs::write(&log_file, "training").unwrap();
        fs::write(&audit_file, "audit").unwrap();

        assert_eq!(purge_log_dir_session_artifacts(&lane_dir).unwrap(), (0, 0));
        assert!(log_file.exists());
        assert!(audit_file.exists());
    }

    #[test]
    fn test_purge_log_dir_session_artifacts_counts_only_removed_files() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let log_file = target.join("session.log");
        let bundle_dir = target.join("Bundle_20260608");
        dev::infra::history_store::append_history_event(
            target,
            "synthetic-session",
            "MFB_HISTORY_FINISHED={\"schema_version\":1,\"outcome\":\"completed\"}",
        )
        .unwrap();
        let history = target.join(dev::infra::history_store::HISTORY_DATABASE);
        let history_before = fs::read(&history).unwrap();
        fs::write(&log_file, "log").unwrap();
        fs::create_dir(&bundle_dir).unwrap();

        assert_eq!(purge_log_dir_session_artifacts(target).unwrap(), (1, 1));
        assert!(!log_file.exists());
        assert!(!bundle_dir.exists());
        assert_eq!(fs::read(history).unwrap(), history_before);
    }
}
