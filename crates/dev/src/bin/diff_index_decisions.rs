#![allow(unused_imports)]

//! 🔍 Decision Diff Tool - Media Index System
//!
//! Compares two decision snapshots or compares a snapshot against live production audit data
//! to detect "Decision Drift" during development or production runs.

use shared_utils::{
    log_anomaly, log_corruption, log_detail, log_failure, log_fatal, log_hint, log_ignore,
    log_skip, log_success,
};

use anyhow::{Context, Result};
use clap::Parser;
use dev::media_index::MediaIndex;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Compares Media Index decision snapshots for version-to-version or mock-to-live diffing."
)]
struct Args {
    /// Path to the `media_index.sqlite`
    #[arg(short, long, default_value = "debug/media_index.sqlite")]
    db: PathBuf,

    /// Reference tag (the 'old' or 'baseline' decision)
    #[arg(name = "LEFT_TAG")]
    left: String,

    /// Target tag to compare against. If omitted, and --live is set, compares against live audit.
    #[arg(name = "RIGHT_TAG")]
    right: Option<String>,

    /// Compare the `LEFT_TAG` against the `live_audit` table instead of another snapshot
    #[arg(long)]
    live: bool,
}

struct Decision {
    format: String,
    reason: String,
    params_json: String,
}

fn main() -> Result<()> {
    use core::fmt::Write;
    let args = Args::parse();
    let index = MediaIndex::open(&args.db)?;

    let db_display = args.db.display();
    log_detail!("🔍 Comparing decisions in: {db_display}");

    // 1. Load Left Side (Baseline Snapshot)
    let left_map = load_snapshot(&index, &args.left)?;
    let left_count = left_map.len();
    let left_tag = &args.left;
    log_detail!("📈 Baseline [{left_tag}]: {left_count} decisions");

    // 2. Load Right Side (Comparison Snapshot or Live Audit)
    let (right_name, right_map) = if args.live {
        let map = load_live_audit(&index)?;
        ("LIVE_AUDIT".to_string(), map)
    } else {
        let tag = args
            .right
            .clone()
            .context("RIGHT_TAG is required when not using --live")?;
        let map = load_snapshot(&index, &tag)?;
        (format!("SNAPSHOT [{tag}]"), map)
    };
    let right_count = right_map.len();
    log_detail!("📈 Target   [{right_name}]: {right_count} decisions");
    log_detail!("--------------------------------------------------");

    let mut format_changes = 0;
    let mut total_diffs = 0;

    // 3. Diff Analysis
    for (blake3, left_dec) in &left_map {
        if let Some(right_dec) = right_map.get(blake3) {
            let mut diff = false;
            let mut diff_msg = String::new();

            if left_dec.format != right_dec.format {
                format_changes += 1;
                diff = true;
                write!(
                    diff_msg,
                    "FORMAT: {} -> {}",
                    left_dec.format, right_dec.format
                )
                .expect("String formatting should not fail");
            }

            if left_dec.params_json != right_dec.params_json {
                diff = true;
                if !diff_msg.is_empty() {
                    diff_msg.push_str(" | ");
                }
                diff_msg.push_str("PARAMS changed");
            }

            if diff {
                total_diffs += 1;
                // Try to resolve path from media_entries
                let path = get_path(&index, blake3).unwrap_or_else(|_| "unknown_file".to_string());
                log_detail!("⚠️  DRIFT: {path} ({diff_msg})");
                let left_reason = &left_dec.reason;
                let right_reason = &right_dec.reason;
                log_detail!("   - Left Reason:  {left_reason}");
                log_detail!("   - Right Reason: {right_reason}");
            }
        }
    }

    log_detail!("--------------------------------------------------");
    log_detail!("📊 Diff Summary:");
    let matched_count = left_map
        .keys()
        .filter(|k| right_map.contains_key(*k))
        .count();
    log_detail!("   - Total Files Matched: {matched_count}");
    log_detail!("   - Total Drifts:        {total_diffs}");
    log_detail!("   - Format Changes:      {format_changes}");

    if total_diffs == 0 {
        log_detail!("✅ No decision drift detected between snapshots.");
    }

    Ok(())
}

fn load_snapshot(index: &MediaIndex, tag: &str) -> Result<HashMap<String, Decision>> {
    let mut stmt = index.conn_prepare(
        "SELECT blake3, decided_format, decision_reason, decided_params_json FROM decision_snapshots WHERE version_tag = ?1"
    )?;
    let mut map = HashMap::new();
    let iter = stmt.query_map([tag], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Decision {
                format: row.get(1)?,
                reason: row.get(2)?,
                params_json: row.get(3)?,
            },
        ))
    })?;

    for res in iter {
        let (k, v) = res?;
        map.insert(k, v);
    }
    Ok(map)
}

fn load_live_audit(index: &MediaIndex) -> Result<HashMap<String, Decision>> {
    let mut stmt =
        index.conn_prepare("SELECT blake3, actual_format, actual_params_json FROM live_audit")?;
    let mut map = HashMap::new();
    let iter = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Decision {
                format: row.get(1)?,
                reason: "Live production execution".to_string(),
                params_json: row.get(2)?,
            },
        ))
    })?;

    for res in iter {
        let (k, v) = res?;
        map.insert(k, v);
    }
    Ok(map)
}

fn get_path(index: &MediaIndex, blake3: &str) -> Result<String> {
    let path: String = index.conn.query_row(
        "SELECT rel_path FROM media_entries WHERE blake3 = ?1",
        [blake3],
        |row| row.get(0),
    )?;
    Ok(path)
}
