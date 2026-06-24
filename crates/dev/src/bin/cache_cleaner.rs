//! Modern Format Boost - Cache Cleaner in Rust.
//! Clears conversion/analysis caches scoped by target or full purges.

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use dev::infra::ui_tokens::pick_symbol;
use rusqlite::Connection;
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
    #[arg(
        long = "purge-animation-cache",
        help = "Remove cache rows for animation-capable image formats"
    )]
    purge_animation_cache: bool,

    #[arg(
        long = "purge-session-state",
        help = "Remove session logs, progress trackers, temp files, and stale locks"
    )]
    purge_session_state: bool,

    #[arg(help = "Target file or directory for fine-grained cleanup")]
    path: Option<String>,

    #[arg(long = "yes", short = 'y', help = "Skip interactive confirmation")]
    yes: bool,
}

fn get_mfb_state_root() -> Result<PathBuf> {
    match std::env::var("MFB_HOME_ROOT") {
        Ok(env_root) => {
            if !env_root.trim().is_empty() {
                return Ok(PathBuf::from(env_root.trim()));
            }
        }
        Err(_err) => {}
    }
    // Shared utils has get_mfb_root helper
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

#[derive(Debug, Clone)]
struct CommandSpec {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
}

impl CommandSpec {
    fn display(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(self.program.display().to_string());
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }

    fn run(&self) -> Result<std::process::ExitStatus> {
        Command::new(&self.program)
            .args(&self.args)
            .current_dir(&self.cwd)
            .status()
            .with_context(|| format!("run {}", self.display()))
    }
}

fn smart_build_command_spec(project_root: &Path, force: bool) -> CommandSpec {
    // Always use release profile for smart_build to ensure production-optimized builds
    let profile = "release";
    let sibling = project_root
        .join("target")
        .join(profile)
        .join("smart_build");
    let mut args = Vec::new();
    let program = if sibling.is_file() {
        sibling
    } else {
        args.extend([
            "run".to_string(),
            "--release".to_string(),
            "--locked".to_string(),
            "-p".to_string(),
            "dev".to_string(),
            "--bin".to_string(),
            "smart_build".to_string(),
            "--".to_string(),
        ]);
        PathBuf::from("cargo")
    };
    if force {
        args.push("--force".to_string());
    }
    CommandSpec {
        program,
        args,
        cwd: project_root.to_path_buf(),
    }
}

fn run_post_cleanup_rebuild(project_root: &Path, force: bool) -> Result<()> {
    println!("\n{BOLD} Verifying img/vid binaries after cache purge...");
    println!("{DIM} Running smart_build (incremental if artifacts are current)...{RESET}");

    let spec = smart_build_command_spec(project_root, force);
    let status = spec.run()?;
    if !status.success() {
        return Err(anyhow!("Rebuild failed with exit status: {status}"));
    }

    println!("\n{GREEN} smart_build finished (img/vid binaries verified)");
    Ok(())
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
        "   {GREEN} PostgreSQL: analysis + inference-log caches truncated (training tables untouched)"
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

fn purge_postgres_for_path(target_path: &Path) -> Result<i32> {
    let target_abs = target_path.canonicalize()?.to_string_lossy().into_owned();
    let escaped_abs = target_abs.replace('\'', "''");
    let mut total = 0;

    if target_path.is_dir() {
        let pattern = format!("{}/%", target_abs.trim_end_matches('/'));
        let escaped_pattern = pattern.replace('\'', "''");

        let q1 = format!(
            "DELETE FROM path_index WHERE file_path = '{escaped_abs}' OR file_path LIKE '{escaped_pattern}';"
        );
        let out1 = run_pg_query(&q1)?;
        total += parse_row_count(&out1)?;

        for table in &["analysis_records", "quality_records", "video_records"] {
            let q = format!(
                "DELETE FROM {table} WHERE content_hash NOT IN (SELECT content_hash FROM path_index);"
            );
            let out = run_pg_query(&q)?;
            total += parse_row_count(&out)?;
        }
    } else {
        let q1 = format!("DELETE FROM path_index WHERE file_path = '{escaped_abs}';");
        let out1 = run_pg_query(&q1)?;
        total += parse_row_count(&out1)?;

        for table in &["analysis_records", "quality_records", "video_records"] {
            let q = format!(
                "DELETE FROM {table} WHERE content_hash NOT IN (SELECT content_hash FROM path_index);"
            );
            let out = run_pg_query(&q)?;
            total += parse_row_count(&out)?;
        }
    }

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

fn purge_postgres_inference_logs_for_path(target_path: &Path) -> Result<i32> {
    let target_abs = target_path.canonicalize()?.to_string_lossy().into_owned();
    let escaped_abs = target_abs.replace('\'', "''");
    let mut total = 0;

    for table in PG_INFERENCE_LOG_TABLES {
        let q = if target_path.is_dir() {
            let pattern = format!("{}/%", target_abs.trim_end_matches('/'));
            let escaped_pattern = pattern.replace('\'', "''");
            format!(
                "DELETE FROM {table} WHERE source_path = '{escaped_abs}' OR source_path LIKE '{escaped_pattern}';"
            )
        } else {
            format!("DELETE FROM {table} WHERE source_path = '{escaped_abs}';")
        };
        let out = run_pg_query(&q)?;
        total += parse_row_count(&out)?;
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

fn purge_postgres_animation_cache() -> Result<i32> {
    let mut array_elems = Vec::new();
    for ext in ANIMATION_CACHE_EXTENSIONS {
        array_elems.push(format!("'%.{ext}'"));
    }
    let array_str = format!("ARRAY[{}]", array_elems.join(", "));

    // We can run these deletes in sequence
    let q_temp = format!(
        "CREATE TEMP TABLE mfb_animation_cache_purge AS \
         SELECT DISTINCT content_hash FROM path_index WHERE lower(file_path) LIKE ANY({array_str});"
    );

    let mut full_query = q_temp;
    full_query.push_str("DELETE FROM analysis_records WHERE content_hash IN (SELECT content_hash FROM mfb_animation_cache_purge);");
    full_query.push_str("DELETE FROM quality_records WHERE content_hash IN (SELECT content_hash FROM mfb_animation_cache_purge);");
    full_query.push_str("DELETE FROM video_records WHERE content_hash IN (SELECT content_hash FROM mfb_animation_cache_purge);");
    full_query.push_str("DELETE FROM path_index WHERE content_hash IN (SELECT content_hash FROM mfb_animation_cache_purge);");

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

fn parse_row_count(stdout: &str) -> Result<i32> {
    // psql DELETE or TRUNCATE output is like: "DELETE 5" or "TRUNCATE TABLE"
    // We can parse numbers from the output lines
    let mut count = 0;
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            if parts[0] == "DELETE" {
                let c = parts[1].parse::<i32>().context("parse DELETE count")?;
                count += c;
            } else if parts[0] == "INSERT" {
                let val = if parts.len() >= 3 { parts[2] } else { parts[1] };
                let c = val.parse::<i32>().context("parse INSERT count")?;
                count += c;
            }
        }
    }
    Ok(count)
}

fn sqlite_store_path() -> Result<PathBuf> {
    let root = get_mfb_state_root()?;
    Ok(root.join("cache").join("mfb_store.sqlite"))
}

fn purge_sqlite_blob_namespace_all(namespace: &str) -> Result<i32> {
    let store = sqlite_store_path()?;
    if !store.is_file() {
        return Ok(0);
    }
    let conn = Connection::open(&store)?;
    let deleted = conn.execute("DELETE FROM blob_store WHERE namespace = ?", [namespace])?;
    Ok(deleted as i32)
}

fn purge_sqlite_blob_namespace_under(namespace: &str, target_path: &Path) -> Result<i32> {
    let store = sqlite_store_path()?;
    if !store.is_file() {
        return Ok(0);
    }
    let target_abs = target_path.canonicalize()?.to_string_lossy().into_owned();
    let pattern = format!("{}/%", target_abs.trim_end_matches('/'));
    let conn = Connection::open(&store)?;
    let deleted = conn.execute(
        "DELETE FROM blob_store WHERE namespace = ? AND (root_path = ? OR root_path LIKE ?)",
        [namespace.to_string(), target_abs, pattern],
    )?;
    Ok(deleted as i32)
}

fn invoke_purge_path_tree_cache(cli_args: &[&str]) -> Result<i32> {
    let project_root = get_project_root()?;
    let bin_path = project_root.join("target/release/purge_path_tree_cache");

    let mut cmd = if bin_path.is_file() {
        Command::new(bin_path)
    } else {
        let mut c = Command::new("cargo");
        c.arg("run")
            .arg("--release")
            .arg("-p")
            .arg("foundation")
            .arg("--bin")
            .arg("purge_path_tree_cache")
            .arg("--");
        c
    };

    cmd.args(cli_args);
    let output = cmd.current_dir(&project_root).output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(anyhow!("purge_path_tree_cache failed: {}", err.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let mut last_val = 0;
    for line in stdout.lines() {
        match line.trim().parse::<i32>() {
            Ok(val) => {
                last_val = val;
            }
            Err(_err) => {}
        }
    }
    Ok(last_val)
}

fn clean_mfb_progress(target_path: &Path) -> Result<(i32, i32)> {
    let store = sqlite_store_path()?;
    if !store.is_file() {
        return Ok((0, 0));
    }
    let progress_root = get_mfb_progress_root()?;
    let target_abs = target_path.canonicalize()?.to_string_lossy().into_owned();
    let is_dir = target_path.is_dir();

    let mut conn = Connection::open(&store)?;
    let mut deleted_count = 0;
    let mut modified_count = 0;

    let mut stmt =
        conn.prepare("SELECT cache_key, payload FROM blob_store WHERE namespace = 'checkpoint'")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut to_delete = Vec::new();
    let mut to_update = Vec::new();

    for r in rows {
        let (cache_key, payload) = r?;
        match serde_json::from_str::<serde_json::Value>(&payload) {
            Ok(mut blob) => {
                let target_dir = blob
                    .get("header")
                    .and_then(|h| h.get("target_dir"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");

                if is_dir
                    && (target_abs == target_dir
                        || target_dir.starts_with(&format!("{target_abs}/")))
                {
                    to_delete.push(cache_key.clone());
                    let lock_file = progress_root.join(format!("{cache_key}.lock"));
                    if lock_file.is_file() {
                        let _ = fs::remove_file(lock_file);
                    }
                    println!("   {GREEN} Removed checkpoint tracker: {target_dir}");
                    continue;
                }

                if !is_dir
                    && (target_abs == target_dir
                        || target_abs.starts_with(&format!("{target_dir}/")))
                    && let Some(entries) = blob.get_mut("entries").and_then(|e| e.as_object_mut())
                    && entries.remove(&target_abs).is_some()
                {
                    to_update.push((cache_key, blob));
                }
            }
            Err(_err) => {}
        }
    }
    stmt.finalize()?;

    let tx = conn.transaction()?;
    for key in to_delete {
        tx.execute(
            "DELETE FROM blob_store WHERE namespace = 'checkpoint' AND cache_key = ?",
            [&key],
        )?;
        deleted_count += 1;
    }

    for (key, blob) in to_update {
        let payload = serde_json::to_string(&blob)?;
        tx.execute(
            "UPDATE blob_store SET payload = ? WHERE namespace = 'checkpoint' AND cache_key = ?",
            [payload, key],
        )?;
        modified_count += 1;
        println!(
            "   {} Pruned file from checkpoint: {}",
            GREEN,
            target_path
                .file_name()
                .unwrap_or(std::ffi::OsStr::new(""))
                .to_string_lossy()
        );
    }
    tx.commit()?;

    if progress_root.is_dir() {
        for entry in fs::read_dir(&progress_root)? {
            let path = entry?.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("txt") {
                let _ = fs::remove_file(&path);
                deleted_count += 1;
                println!(
                    "   {} Removed orphan legacy progress file: {}",
                    GREEN,
                    path.file_name()
                        .unwrap_or(std::ffi::OsStr::new(""))
                        .to_string_lossy()
                );
            }
        }
    }

    Ok((deleted_count, modified_count))
}

fn clean_path_tree(target_path: &Path) -> Result<i32> {
    let target_abs = target_path.canonicalize()?.to_string_lossy().into_owned();
    let deleted = invoke_purge_path_tree_cache(&["--under", &target_abs])?;
    if deleted > 0 {
        println!(
            "   {} path_tree_snapshots (PG + SQLite): removed {} row(s) under {}",
            GREEN,
            deleted,
            target_path
                .file_name()
                .unwrap_or(std::ffi::OsStr::new(""))
                .to_string_lossy()
        );
    }
    Ok(deleted)
}

fn clean_all_path_tree() -> Result<i32> {
    let deleted = invoke_purge_path_tree_cache(&["--all"])?;
    if deleted > 0 {
        println!("   {GREEN} Removed {deleted} path-tree cache entries (PG + SQLite)");
    }
    Ok(deleted)
}

fn legacy_analysis_sqlite_paths() -> Result<Vec<PathBuf>> {
    let root = get_mfb_state_root()?;
    let cache_dir = root.join("cache");
    Ok(vec![
        cache_dir.join("image_analysis_v2.db"),
        cache_dir.join("image_analysis_v2_main.db"),
    ])
}

fn remove_legacy_analysis_sqlite_files() -> Result<i32> {
    let mut removed = 0;
    for path in legacy_analysis_sqlite_paths()? {
        if path.is_file() {
            fs::remove_file(&path)?;
            removed += 1;
            println!(
                "   {} Removed legacy analysis DB: {}",
                GREEN,
                path.file_name()
                    .unwrap_or(std::ffi::OsStr::new(""))
                    .to_string_lossy()
            );
        }
    }
    Ok(removed)
}

fn purge_mfb_store_blob_namespaces_full() -> Result<i32> {
    let mut total = 0;
    for namespace in &["path_tree", "checkpoint", "processed"] {
        total += purge_sqlite_blob_namespace_all(namespace)?;
    }
    if total > 0 {
        println!(
            "   {GREEN} mfb_store.sqlite: purged {total} blob_store row(s) (path_tree/checkpoint/processed)"
        );
    }
    Ok(total)
}

fn purge_conversion_resume_state(
    progress_dir: &Path,
    tmp_dir: &Path,
    lock_dir: &Path,
) -> Result<()> {
    if progress_dir.is_dir() {
        println!("{DIM}   Removing MFB progress directory...{RESET}");
        let _ = fs::remove_dir_all(progress_dir);
        println!("   {GREEN} MFB progress purged");
    }

    if tmp_dir.is_dir() {
        println!("{DIM}   Purging isolated temp directory...{RESET}");
        let _ = fs::remove_dir_all(tmp_dir);
        let _ = fs::create_dir_all(tmp_dir);
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
                    let _ = fs::remove_file(&path);
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
                    remove_session_file(&path, &mut removed_logs);
                }
            } else if let Some(name) = path.file_name().and_then(|f| f.to_str())
                && ((name.starts_with("diagnostic_report_") && name.ends_with(".txt"))
                    || name == "deleted_offending_files.txt")
            {
                remove_session_file(&path, &mut removed_logs);
            }
        } else if path.is_dir()
            && let Some(name) = path.file_name().and_then(|f| f.to_str())
            && (name.starts_with("Bundle_") || name == "dev_verify")
        {
            match fs::remove_dir_all(&path) {
                Ok(()) => removed_dirs += 1,
                Err(err) => warn_cleanup_failure(&path, &err),
            }
        }
    }

    Ok((removed_logs, removed_dirs))
}

fn remove_session_file(path: &Path, removed_logs: &mut i32) {
    match fs::remove_file(path) {
        Ok(()) => *removed_logs += 1,
        Err(err) => warn_cleanup_failure(path, &err),
    }
}

fn warn_cleanup_failure(path: &Path, err: &std::io::Error) {
    eprintln!(
        "  {YELLOW}WARN:{RESET} cleanup failed for {}: {err}",
        path.display()
    );
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
        format!(
            "{} CACHE & LOG CLEANUP UTILITY v1.1",
            pick_symbol("🧹", "[SWEEP]")
        )
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
            "   {RED}  WARNING: Critical processing data will be permanently deleted.{RESET}\n"
        );
    }
}

fn perform_animation_cache_cleanup() -> Result<()> {
    check_postgres_reachable()?;
    draw_header(true);
    println!("   {BOLD}Target:{RESET} {DIM}animation-capable cache entries{RESET}");
    println!("   {YELLOW}Purging cached static/unknown verdicts and routing snapshots...{RESET}\n");

    purge_postgres_animation_cache()?;
    clean_all_path_tree()?;
    remove_legacy_analysis_sqlite_files()?;
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
    let store_file = sqlite_store_path()?;

    draw_header(true);
    show_stats(&cache_dir, &store_file, &log_dir, &progress_dir)?;
    println!(
        "   {BOLD}Target:{RESET} {DIM}session state only (logs, progress, temp, stale locks){RESET}\n"
    );

    if !yes && sys_stdin_stdout_isatty() {
        println!("{YELLOW}  CONFIRM: Clear session state artifacts only?{RESET}");
        print!("   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "yes" && input.trim().to_lowercase() != "y" {
            println!("\n{RED} Session-state cleanup cancelled by user.\n");
            return Ok(());
        }
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

fn perform_full_cleanup(yes: bool) -> Result<(bool, bool)> {
    let state_root = get_mfb_state_root()?;
    let cache_dir = state_root.join("cache");
    let store_file = sqlite_store_path()?;
    let log_dir = dev::infra::log_paths::unified_log_dir();
    let progress_dir = get_mfb_progress_root()?;
    let tmp_dir = state_root.join("tmp");
    let lock_dir = state_root.join("locks");

    draw_header(false);
    show_stats(&cache_dir, &store_file, &log_dir, &progress_dir)?;

    if let Err(pg_err) = check_postgres_reachable() {
        println!("\n{RED} PostgreSQL is required before cache cleanup can run.{RESET}");
        println!("   {DIM}Reason: {pg_err}{RESET}");
        println!(
            "   {}Connection: {} (override with MFB_PG_CONNSTR){}\n",
            DIM,
            pg_connstr(),
            RESET
        );
        return Ok((false, false));
    }

    println!("{RED}  The following caches will be PERMANENTLY cleared:{RESET}");
    println!(
        "   - PostgreSQL analysis cache (records, path_index, path_tree_snapshots, cache_metadata)"
    );
    println!("   - PostgreSQL inference-log telemetry (loop/image/animated/video)");
    println!("   - Legacy analysis SQLite files (image_analysis_v2*.db, if present)");
    println!("   - mfb_store.sqlite (path-tree, checkpoint, processed blobs)");
    println!("   - Batch resume state (~/.mfb_progress/, tmp/, stale locks)");
    println!(
        "   {GREEN} - Training corpora (loop_samples, *_quality_samples, metadata) are preserved{RESET}"
    );
    println!("   {GREEN} - Training lane logs and local training SQLite are preserved{RESET}");
    println!();

    if !yes && sys_stdin_stdout_isatty() {
        println!("{YELLOW}  CONFIRM: Start full cache cleanup?{RESET}");
        print!("   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "yes" && input.trim().to_lowercase() != "y" {
            println!("\n{RED} Cleanup cancelled by user.{RESET}");
            println!("{DIM}   No action taken.{RESET}");
            return Ok((false, false));
        }
    }

    println!("\n{YELLOW} Executing full cache cleanup...{RESET}");
    purge_postgres_full()?;
    let _ = remove_legacy_analysis_sqlite_files();
    let _ = purge_mfb_store_blob_namespaces_full();

    if cache_dir.is_dir() {
        println!("{DIM}   Clearing cache directory (preserving models)...{RESET}");
        for entry in fs::read_dir(&cache_dir)? {
            let path = entry?.path();
            if path.file_name().and_then(|f| f.to_str()) == Some("models") {
                continue;
            }
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
        println!("   {GREEN} Local cache directory cleared");
    }

    purge_conversion_resume_state(&progress_dir, &tmp_dir, &lock_dir)?;
    println!("\n{GREEN} Full Cache Cleanup Complete\n");
    Ok((true, true))
}

fn perform_targeted_cleanup(target_path: &Path) -> Result<()> {
    if !target_path.exists() {
        return Err(anyhow!("Path does not exist: {}", target_path.display()));
    }

    check_postgres_reachable()?;
    draw_header(true);
    println!("   {BOLD}Target:{RESET} {DIM}");
    println!("   {YELLOW}Scanning metadata associated with this path...{RESET}\n");

    // 0. PostgreSQL targeted purge
    purge_postgres_for_path(target_path)?;
    purge_postgres_inference_logs_for_path(target_path)?;

    // 1. Progress Tracker
    let _ = clean_mfb_progress(target_path);

    // 2. Path Tree Cache
    let _ = clean_path_tree(target_path);

    // 3. Processed list blobs
    let removed = purge_sqlite_blob_namespace_under("processed", target_path)?;
    if removed > 0 {
        println!("   {GREEN} mfb_store processed blobs: removed {removed} row(s)");
    }

    println!("\n{GREEN} Targeted Cleanup Complete\n");
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.purge_animation_cache {
        perform_animation_cache_cleanup()?;
        return Ok(());
    }

    if args.purge_session_state {
        perform_session_state_cleanup(args.yes)?;
        return Ok(());
    }

    if let Some(target) = args.path {
        perform_targeted_cleanup(Path::new(&target))?;
    } else {
        let (completed, rebuild) = perform_full_cleanup(args.yes)?;
        if !completed {
            std::process::exit(1);
        }
        if rebuild {
            let root = get_project_root()?;
            let _ = run_post_cleanup_rebuild(&root, false);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_row_count() {
        assert_eq!(parse_row_count("DELETE 5").unwrap(), 5);
        assert_eq!(parse_row_count("INSERT 0 10").unwrap(), 10);
        assert_eq!(parse_row_count("TRUNCATE TABLE").unwrap(), 0);
        assert_eq!(parse_row_count("DELETE 2\nDELETE 3").unwrap(), 5);
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

        // If lock file doesn't exist or isn't locked, is_lock_stale should return false or handle gracefully
        // Wait, is_lock_stale attempts to open the file read/write, which fails if the file doesn't exist:
        assert!(!is_lock_stale(&lock_file));

        // Create the file:
        fs::write(&lock_file, b"").unwrap();
        // Without an active lock, opening and locking succeeds, so flock returns 0, and unlocks, returning true (stale):
        assert!(is_lock_stale(&lock_file));
    }

    #[test]
    fn test_post_cleanup_rebuild_uses_rust_smart_build_not_python() {
        let spec = smart_build_command_spec(Path::new("/repo"), true);
        let rendered = spec.display();
        assert!(rendered.contains("smart_build"));
        assert!(rendered.contains("--force"));
        assert!(!rendered.contains("python"));
        assert!(!rendered.contains(".py"));
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
        fs::write(&log_file, "log").unwrap();
        fs::create_dir(&bundle_dir).unwrap();

        assert_eq!(purge_log_dir_session_artifacts(target).unwrap(), (1, 1));
        assert!(!log_file.exists());
        assert!(!bundle_dir.exists());
    }
}
