//! 📥 Ingest Audit Tool - Media Index System
//!
//! Syncs production decision logs (JSONL) into the `SQLite` Media Index.

#![allow(unused_imports)]

use shared_utils::{
    log_anomaly, log_corruption, log_detail, log_failure, log_fatal, log_hint, log_ignore,
    log_skip, log_success,
};

use anyhow::{Context, Result};
use clap::Parser;
use dev::media_index::MediaIndex;
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Syncs JSONL production logs into the Media Index SQLite database."
)]
struct Args {
    /// Path to the `live_audit.jsonl`
    #[arg(short, long, default_value = "debug/live_audit.jsonl")]
    input: PathBuf,

    /// Path to the `media_index.sqlite`
    #[arg(short, long, default_value = "debug/media_index.sqlite")]
    db: PathBuf,
}

#[derive(Deserialize)]
struct AuditRecord {
    blake3: String,
    session_id: String,
    actual_format: String,
    actual_params_json: String,
    #[serde(rename = "audit_at")]
    _audit_at: i64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.input.exists() {
        let input_display = args.input.display();
        log_detail!("ℹ️ No audit log found at {input_display}. Nothing to ingest.");
        return Ok(());
    }

    let index = MediaIndex::open(&args.db)?;
    let mut count = 0;

    let file = File::open(&args.input).context("Failed to open audit log")?;
    let reader = BufReader::new(file);

    let input_display = args.input.display();
    let db_display = args.db.display();
    log_detail!("📥 Ingesting logs from {input_display} into {db_display}...");

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(record) = serde_json::from_str::<AuditRecord>(&line) {
            index.log_live_details(
                &record.blake3,
                &record.session_id,
                &record.actual_format,
                &record.actual_params_json,
                None, // VMAF not in basic audit yet
            )?;
            count += 1;
        }
    }

    log_detail!("✅ Successfully ingested {count} decision records.");
    Ok(())
}
