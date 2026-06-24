//! Unified log directory and filename conventions in Rust.

use crate::infra::hardening::optional_env;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, TimeZone};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const MFB_LOG_SESSION_STAMP: &str = "%Y%m%d_%H%M%S";
pub const MFB_LOG_AUDIT_DAY_STAMP: &str = "%Y%m%d";
pub const MFB_DEFAULT_HOME_DIRNAME: &str = ".modern_format_boost";
pub const TRAINING_BUNDLE_PREFIX: &str = "TrainingBundle_";
pub const SESSION_BUNDLE_PREFIX: &str = "Bundle_";

pub const TRAINING_LOG_LANES: &[&str] = &["static_high", "static_low", "loop_high", "loop_low"];
pub const LEGACY_TRAINING_LOG_LANES: &[&str] = &["static", "all_high", "loop", "loop_video"];

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// Return repo root when starting at `start` or current working directory.
#[must_use]
pub fn find_mfb_workspace_root(start: Option<&Path>) -> Option<PathBuf> {
    let mut dir_path = match start {
        Some(p) => p.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(d) => d,
            Err(_err) => return None,
        },
    };

    // Up to 16 parent iterations
    for _ in 0..16 {
        if dir_path.join("Cargo.toml").is_file() && dir_path.join("crates").is_dir() {
            return Some(dir_path);
        }
        if let Some(parent) = dir_path.parent() {
            dir_path = parent.to_path_buf();
        } else {
            break;
        }
    }
    None
}

/// Returns true if `path` is `<workspace>/logs` or
/// `<workspace>/target/training*`.
#[must_use]
pub fn is_forbidden_log_path(path: &Path) -> bool {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let workspace =
        match find_mfb_workspace_root(Some(&resolved)).or_else(|| find_mfb_workspace_root(None)) {
            Some(ws) => ws,
            None => return false,
        };

    let rel = match resolved.strip_prefix(&workspace) {
        Ok(r) => r,
        Err(_) => return false,
    };

    let parts: Vec<&str> = rel.iter().map(|s| s.to_str().unwrap_or("")).collect();
    if parts.is_empty() {
        return false;
    }

    if parts[0] == "logs" {
        return true;
    }

    if parts.len() >= 2 && parts[0] == "target" && parts[1].starts_with("training") {
        return true;
    }

    false
}

fn default_user_log_dir() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home)
            .join(MFB_DEFAULT_HOME_DIRNAME)
            .join("logs"),
        Err(_err) => std::env::temp_dir().join("modern_format_boost_logs"),
    }
}

#[must_use]
pub fn persistent_log_dir() -> PathBuf {
    match std::env::var("MFB_HOME_ROOT") {
        Ok(home_root) => {
            let candidate = PathBuf::from(home_root).join("logs");
            if !is_forbidden_log_path(&candidate) {
                return candidate;
            }
            let fallback = default_user_log_dir();
            if !is_forbidden_log_path(&fallback) {
                eprintln!(
                    "[MFB] Refusing workspace MFB_HOME_ROOT log dir {} — using {}",
                    candidate.display(),
                    fallback.display()
                );
                return fallback;
            }
        }
        Err(_err) => {}
    }
    default_user_log_dir()
}

#[must_use]
pub fn coerce_log_dir(candidate: &Path) -> PathBuf {
    if !is_forbidden_log_path(candidate) {
        return candidate.to_path_buf();
    }
    let mut fallback = persistent_log_dir();
    if is_forbidden_log_path(&fallback) {
        fallback = default_user_log_dir();
    }
    if is_forbidden_log_path(&fallback) {
        fallback = std::env::temp_dir().join("modern_format_boost_logs");
    }
    eprintln!(
        "[MFB] Refusing workspace log dir {} — using {}",
        candidate.display(),
        fallback.display()
    );
    fallback
}

#[must_use]
pub fn unified_log_dir() -> PathBuf {
    // Aligns with foundation::logging::LogConfig::unified_log_dir
    foundation::logging::LogConfig::unified_log_dir()
}

pub fn ensure_unified_log_dir() -> Result<PathBuf> {
    let log_dir = unified_log_dir();
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create log directory: {}", log_dir.display()))?;
    let canonical = log_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize log directory: {}",
            log_dir.display()
        )
    })?;
    unsafe {
        std::env::set_var("MFB_LOG_DIR", canonical.as_os_str());
    }
    Ok(canonical)
}

pub fn format_session_stamp(when: Option<DateTime<Local>>) -> String {
    let dt = when.unwrap_or_else(Local::now);
    dt.format(MFB_LOG_SESSION_STAMP).to_string()
}

pub fn parse_session_stamp(stamp: &str) -> Result<DateTime<Local>> {
    match DateTime::parse_from_str(stamp, MFB_LOG_SESSION_STAMP) {
        Ok(dt) => return Ok(dt.with_timezone(&Local)),
        Err(_err) => {}
    }
    // Try without timezone/offset using naive parse + local timezone conversion
    match chrono::NaiveDateTime::parse_from_str(stamp, MFB_LOG_SESSION_STAMP) {
        Ok(naive) => {
            if let Some(local_dt) = Local.from_local_datetime(&naive).single() {
                return Ok(local_dt);
            }
        }
        Err(_err) => {}
    }
    match chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d_%H-%M-%S") {
        Ok(naive) => {
            if let Some(local_dt) = Local.from_local_datetime(&naive).single() {
                return Ok(local_dt);
            }
        }
        Err(_err) => {}
    }
    Err(anyhow!("Unrecognized session log stamp: {stamp}"))
}

#[must_use]
pub fn training_lane_slug(
    training_mode: &str,
    label: Option<&str>,
    loop_intent_label: &str,
) -> String {
    let mode = training_mode.trim().to_lowercase();
    if mode == "static" {
        match label {
            Some("low") => "static_low".to_string(),
            Some("high") => "static_high".to_string(),
            _ => "static".to_string(),
        }
    } else if mode == "loop" {
        let li = loop_intent_label.trim().to_lowercase();
        if li == "high" {
            "loop_high".to_string()
        } else if li == "low" {
            "loop_low".to_string()
        } else if li == "video" {
            "loop_video".to_string()
        } else {
            "loop".to_string()
        }
    } else if mode == "all" {
        let li = loop_intent_label.trim().to_lowercase();
        if li == "high" {
            "all_high".to_string()
        } else {
            "all".to_string()
        }
    } else {
        mode
    }
}

#[must_use]
pub fn ensure_training_session_stamp() -> String {
    let stamp = std::env::var("MFB_TRAINING_SESSION_STAMP")
        .unwrap_or_default()
        .trim()
        .to_string();
    if stamp.is_empty() {
        let new_stamp = format_session_stamp(None);
        unsafe {
            std::env::set_var("MFB_TRAINING_SESSION_STAMP", &new_stamp);
        }
        new_stamp
    } else {
        stamp
    }
}

#[must_use]
pub fn training_lane_pid_is_active(lane_dir: &Path) -> bool {
    let pid_file = lane_dir.join("run_training.pid");
    if !pid_file.is_file() {
        return false;
    }
    let contents = match fs::read_to_string(&pid_file) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pid: i32 = match contents.trim().parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    if pid <= 0 {
        return false;
    }
    unsafe { kill(pid, 0) == 0 }
}

/// Append machine-readable JSONL audit record.
/// Honors `MFB_SESSION_AUDIT` env var when called via helper functions.
pub fn append_jsonl_audit_record(audit_path: &Path, event: &str) -> Result<()> {
    if let Some(parent) = audit_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            eprintln!(
                "[LOG] audit parent create failed ({}): {err}",
                parent.display()
            );
            err
        })?;
    }
    let stamp = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let record = serde_json::json!({
        "ts": stamp,
        "event": event
    });
    let record_str = serde_json::to_string(&record)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(audit_path)?;
    writeln!(file, "{record_str}")?;
    Ok(())
}

/// Resolve audit log path from `MFB_SESSION_AUDIT` env var (py parity).
#[must_use]
pub fn audit_log_path_from_env() -> Option<PathBuf> {
    optional_env("MFB_SESSION_AUDIT").map(PathBuf::from)
}

/// Append session audit line if `MFB_SESSION_AUDIT` is set.
pub fn append_session_audit_if_enabled(line: &str) -> Result<()> {
    if let Some(path) = audit_log_path_from_env() {
        append_jsonl_audit_record(&path, line)?;
    }
    Ok(())
}

pub fn archive_training_session_bundle(
    log_dir: &Path,
    session_stamp: &str,
    scope: Option<&str>,
) -> Result<Option<PathBuf>> {
    let log_dir = log_dir.to_path_buf();
    if !log_dir.is_dir() {
        return Ok(None);
    }
    let stamp = session_stamp.trim();
    if stamp.is_empty() {
        return Ok(None);
    }

    let bundle = log_dir.join(format!("{TRAINING_BUNDLE_PREFIX}{stamp}"));
    let mut moved = Vec::new();

    let mut move_file = |name: &str| -> Result<()> {
        let src = log_dir.join(name);
        if !src.is_file() {
            return Ok(());
        }
        fs::create_dir_all(&bundle)?;
        let dest = bundle.join(name);
        if dest.exists() {
            fs::remove_file(&dest)?;
        }
        fs::rename(&src, &dest)?;
        moved.push(name.to_string());
        Ok(())
    };

    let mut exit_snapshot = None;
    let exit_src = log_dir.join("training_session_exit.json");
    if exit_src.is_file() {
        match fs::read_to_string(&exit_src) {
            Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(json_val) => {
                    exit_snapshot = Some(json_val);
                }
                Err(_err) => {}
            },
            Err(_err) => {}
        }
    }

    move_file(&format!("run_training_{stamp}.log"))?;
    move_file("training_session_audit.jsonl")?;
    move_file("training_session_exit.json")?;
    move_file(&format!("replica_audit_{stamp}.jsonl"))?;

    let tier_live = log_dir.join("training_tier_audit.jsonl");
    if tier_live.is_file() {
        fs::create_dir_all(&bundle)?;
        let dest = bundle.join(format!("training_tier_audit_{stamp}.jsonl"));
        if dest.exists() {
            fs::remove_file(&dest)?;
        }
        fs::rename(&tier_live, &dest)?;
        let file_name = dest
            .file_name()
            .and_then(|name| name.to_str())
            .context("training tier audit destination missing UTF-8 file name")?;
        moved.push(file_name.to_string());
    }

    if moved.is_empty() {
        return Ok(None);
    }

    moved.sort();

    let mut manifest = serde_json::json!({
        "session_stamp": stamp,
        "training_lane": match std::env::var("MFB_TRAINING_LANE") {
            Ok(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            Err(_err) => None,
        },
        "scope": scope,
        "files": moved,
        "archived_at": Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
    });

    if let Some(snapshot) = exit_snapshot {
        manifest["exit"] = snapshot;
    }

    let manifest_path = bundle.join("manifest.json");
    fs::write(
        &manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;

    Ok(Some(bundle))
}

pub fn archive_drag_drop_session_bundle(
    log_dir: &Path,
    session_stamp: &str,
    session_log: Option<&Path>,
    verbose_log: Option<&Path>,
    session_audit: Option<&Path>,
    session_started_at: Option<DateTime<Local>>,
) -> Result<Option<PathBuf>> {
    let log_dir = log_dir.to_path_buf();
    if !log_dir.is_dir() {
        return Ok(None);
    }
    let stamp = session_stamp.trim();
    if stamp.is_empty() {
        return Ok(None);
    }

    let session_dt = match session_started_at {
        Some(dt) => dt,
        None => parse_session_stamp(stamp).unwrap_or_else(|_| Local::now()),
    };

    let in_session_window = |path: &Path| -> bool {
        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let mtime = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return false,
        };
        let mtime_dt: DateTime<Local> = DateTime::from(mtime);
        mtime_dt >= session_dt - chrono::Duration::seconds(10)
    };

    let bundle = log_dir.join(format!("{SESSION_BUNDLE_PREFIX}{stamp}"));
    let mut moved = HashSet::new();

    let mut move_path = |src: &Path| -> Result<()> {
        if !src.is_file() {
            return Ok(());
        }
        let resolved = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
        let log_resolved = log_dir.canonicalize().unwrap_or_else(|_| log_dir.clone());
        if resolved.parent() != Some(&log_resolved) {
            return Ok(());
        }
        fs::create_dir_all(&bundle)?;
        let filename = src
            .file_name()
            .context("no filename")?
            .to_str()
            .context("invalid utf8 filename")?;
        let dest = bundle.join(filename);
        if dest.exists() {
            fs::remove_file(&dest)?;
        }
        fs::rename(src, &dest)?;
        moved.insert(filename.to_string());
        Ok(())
    };

    if let Some(p) = session_log {
        move_path(p)?;
    }
    if let Some(p) = verbose_log {
        move_path(p)?;
    }
    if let Some(p) = session_audit {
        move_path(p)?;
    }

    // Scan glob patterns
    let patterns = vec![
        "img_*.log".to_string(),
        "vid_*.log".to_string(),
        "img_*.jsonl".to_string(),
        "vid_*.jsonl".to_string(),
        format!("MFB_Session_{}.log", stamp),
        format!("MFB_*_{}.log", stamp),
        format!("verbose_{}.log", stamp),
        format!("session_audit_{}.jsonl", stamp),
        "diagnostic_report_*.txt".to_string(),
    ];

    let mut candidates = Vec::new();
    for entry in fs::read_dir(&log_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && let Some(fname) = path.file_name().and_then(|f| f.to_str())
        {
            for pat in &patterns {
                if glob_match(pat, fname) && in_session_window(&path) {
                    candidates.push(path.clone());
                    break;
                }
            }
        }
    }

    // Sort by mtime
    candidates.sort_by(|a, b| {
        let mtime_a = fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mtime_b = fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        mtime_a.cmp(&mtime_b)
    });

    for cand in candidates {
        move_path(&cand)?;
    }

    if moved.is_empty() {
        return Ok(None);
    }

    let mut moved_vec: Vec<String> = moved.into_iter().collect();
    moved_vec.sort();

    let manifest = serde_json::json!({
        "session_stamp": stamp,
        "session_id": match std::env::var("MFB_SESSION_ID") {
            Ok(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            Err(_err) => None,
        },
        "files": moved_vec,
        "archived_at": Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "log_root": log_dir.canonicalize().unwrap_or(log_dir).to_string_lossy().to_string(),
    });

    let manifest_path = bundle.join("manifest.json");
    fs::write(
        &manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;

    Ok(Some(bundle))
}

pub fn iter_training_log_dirs(log_root: &Path) -> Result<Vec<PathBuf>> {
    let log_root = log_root.to_path_buf();
    let mut dirs = vec![log_root.clone()];
    for lane in TRAINING_LOG_LANES
        .iter()
        .chain(LEGACY_TRAINING_LOG_LANES.iter())
    {
        let lane_dir = log_root.join(lane);
        if lane_dir.is_dir() {
            dirs.push(lane_dir);
        }
    }
    Ok(dirs)
}

fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            name.starts_with(parts[0]) && name.ends_with(parts[1])
        } else if parts.len() == 3 {
            name.starts_with(parts[0]) && name.ends_with(parts[2]) && name.contains(parts[1])
        } else {
            name.contains(parts[0])
        }
    } else {
        name == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("img_*.log", "img_test.log"));
        assert!(glob_match("img_*.log", "img_123.log"));
        assert!(!glob_match("img_*.log", "video_test.log"));
        assert!(glob_match("test", "test"));
        assert!(glob_match("a*b*c", "abbbc"));
    }

    #[test]
    fn test_training_lane_slug() {
        assert_eq!(training_lane_slug("static", Some("low"), ""), "static_low");
        assert_eq!(
            training_lane_slug("static", Some("high"), ""),
            "static_high"
        );
        assert_eq!(training_lane_slug("static", None, ""), "static");
        assert_eq!(training_lane_slug("loop", None, "high"), "loop_high");
        assert_eq!(training_lane_slug("loop", None, "low"), "loop_low");
        assert_eq!(training_lane_slug("loop", None, "video"), "loop_video");
        assert_eq!(training_lane_slug("all", None, "high"), "all_high");
    }

    #[test]
    fn test_session_stamp() {
        let stamp = format_session_stamp(None);
        let parsed = parse_session_stamp(&stamp);
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_find_mfb_workspace_root() {
        let root = find_mfb_workspace_root(None);
        // Inside cargo test, we are in the workspace
        assert!(root.is_some());
    }

    #[test]
    fn test_is_forbidden_log_path() {
        if let Some(root) = find_mfb_workspace_root(None) {
            let forbidden = root.join("logs");
            assert!(is_forbidden_log_path(&forbidden));

            let not_forbidden = root.join("src");
            assert!(!is_forbidden_log_path(&not_forbidden));
        }
    }
}
