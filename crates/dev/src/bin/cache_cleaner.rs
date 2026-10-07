//! Modern Format Boost - Cache Cleaner in Rust.
//! Clears conversion/analysis caches scoped by target or full purges.

use anyhow::{Context, Result, anyhow};
use clap::Parser;
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
#[command(name = "cache_cleaner", about = "Modern Format Boost Cache Cleaner")]
struct Args {
    #[arg(long, conflicts_with_all = ["path", "postgres", "purge_animation_cache", "purge_session_state"], help = "Inspect local cache without deleting or rebuilding anything")]
    stats: bool,

    #[arg(long, conflicts_with_all = ["postgres", "purge_animation_cache", "purge_session_state"], help = "Return versioned local cache statistics as JSON")]
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
    let version: i32 = conn.query_row(
        "SELECT value FROM store_metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        version == foundation::mfb_sqlite_store::STORE_SCHEMA_VERSION,
        "unsupported cache store schema {version}; database retained"
    );
    Ok(conn)
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

fn cache_status(cache_dir: &Path) -> Result<CacheStatus> {
    ensure_cache_directory(cache_dir)?;
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
    };
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
        let mut conn = open_cache_store(&store, true)?;
        let snapshot = conn.transaction()?;
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
        snapshot.commit()?;
    }
    for path in legacy_analysis_sqlite_paths(cache_dir) {
        if let Some(bytes) = regular_file_size(&path)? {
            status.legacy_analysis_bytes = status
                .legacy_analysis_bytes
                .checked_add(bytes)
                .context("legacy cache size overflow")?;
            status.legacy_analysis_files += 1;
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
    println!("{YELLOW}{prompt}{RESET}");
    print!("   {CYAN}Type 'yes' to proceed: {RESET}");
    io::stdout().flush()?;
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
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.purge_animation_cache {
        perform_animation_cache_cleanup(args.yes)?;
        return Ok(());
    }

    if args.purge_session_state {
        perform_session_state_cleanup(args.yes)?;
        return Ok(());
    }

    let cache_dir = get_mfb_state_root()?.join("cache");
    let before = cache_status(&cache_dir)?;
    if args.stats {
        return print_cache_status(&before, args.json);
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
                 PRIMARY KEY(namespace, cache_key));",
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
        assert!(confirm_cleanup(true, "synthetic").unwrap());
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
