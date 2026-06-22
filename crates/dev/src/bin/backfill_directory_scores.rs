use anyhow::{Context, Result};
use clap::Parser;
use foundation::config_load::load_consumer_json;
use postgres::{Client, NoTls};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

const DEFAULT_CONNSTR: &str = "postgresql://localhost/modern_format_boost";
const BACKFILL_PROGRESS_INTERVAL: usize = 1000;
const BACKFILL_HEARTBEAT_SECS: f32 = 10.0;

#[derive(Parser)]
#[command(about = "Backfill metadata.directory_loop_intent_score")]
struct Args {
    #[arg(long, default_value = DEFAULT_CONNSTR)]
    connstr: String,

    #[arg(long)]
    no_refresh_stats: bool,
}

#[derive(Deserialize, Debug)]
struct DirectoryScoringConfig {
    base_score: f64,
    max_depth: usize,
    match_weight: f64,
}

#[derive(Deserialize, Debug)]
struct KeywordsConfig {
    keywords: HashMap<String, Vec<String>>,
    scoring: DirectoryScoringConfig,
}

fn load_keywords_config(project_root: &Path) -> Result<KeywordsConfig> {
    let config_path = project_root.join("crates/dev/src/config/directory_keywords.json");
    let obj =
        load_consumer_json(&config_path, "backfill_directory_scores.py").context("load config")?;
    let value = Value::Object(obj);
    let config: KeywordsConfig = serde_json::from_value(value).context("parse config")?;
    Ok(config)
}

fn compute_directory_score(
    source_path: &str,
    keywords: &[String],
    scoring: &DirectoryScoringConfig,
) -> Option<f64> {
    let path = Path::new(source_path);
    let parent = path.parent()?;
    let parts: Vec<String> = parent
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();

    let max_depth = scoring.max_depth;
    let skip = if parts.len() > max_depth {
        parts.len() - max_depth
    } else {
        0
    };
    let last_parts = &parts[skip..];

    let mut matches = 0;
    for part in last_parts {
        if keywords.iter().any(|k| part.contains(k)) {
            matches += 1;
        }
    }

    let max_depth_f64 = (max_depth.max(1)) as f64;
    let score = scoring.base_score + (matches as f64 / max_depth_f64) * scoring.match_weight;
    Some(score.clamp(0.0, 1.0))
}

fn main() -> Result<()> {
    let args = Args::parse();

    let project_root = std::env::current_dir().unwrap();
    let config = load_keywords_config(&project_root)?;

    let mut all_keywords = Vec::new();
    for kws in config.keywords.values() {
        all_keywords.extend(kws.clone());
    }

    let mut client = Client::connect(&args.connstr, NoTls).context("connect postgres")?;

    println!("Fetching loop_samples...");
    let rows = client
        .query("SELECT blake3, source_path FROM loop_samples", &[])
        .context("select")?;

    let mut updates = Vec::new();
    let mut skipped_unknown = 0;

    let started = Instant::now();
    let mut last_progress_at = started;
    let mut last_progress_index = 0;
    let total_rows = rows.len();

    println!(
        "Scoring {} loop_samples rows (heartbeat_every={} rows, heartbeat_max_silence={:.1}s)...",
        total_rows, BACKFILL_PROGRESS_INTERVAL, BACKFILL_HEARTBEAT_SECS
    );

    for (index, row) in rows.iter().enumerate() {
        let blake3: Vec<u8> = row.get(0);
        let source_path: Option<String> = row.get(1);

        let score = if let Some(sp) = source_path {
            compute_directory_score(&sp, &all_keywords, &config.scoring)
        } else {
            None
        };

        if let Some(s) = score {
            updates.push((s, blake3));
        } else {
            skipped_unknown += 1;
        }

        let now = Instant::now();
        if index == 0
            || index == total_rows - 1
            || index - last_progress_index >= BACKFILL_PROGRESS_INTERVAL
            || now.duration_since(last_progress_at).as_secs_f32() >= BACKFILL_HEARTBEAT_SECS
        {
            let elapsed = now.duration_since(started).as_secs_f32();
            let speed = (index + 1) as f32 / elapsed;
            println!(
                "   [Progress] {}/{} rows processed ({:.1} rows/s)...",
                index + 1,
                total_rows,
                speed
            );
            last_progress_at = now;
            last_progress_index = index;
        }
    }

    println!(
        "Found {} directories to score, {} skipped.",
        updates.len(),
        skipped_unknown
    );

    let mut tx = client.transaction()?;

    let stmt = tx.prepare("UPDATE loop_samples SET metadata = jsonb_set(COALESCE(metadata, '{}'::jsonb), '{directory_loop_intent_score}', $1::jsonb) WHERE blake3 = $2")?;

    println!("Writing updates to database...");
    for (i, (score, blake3)) in updates.iter().enumerate() {
        let score_json: Value = serde_json::to_value(score)?;
        tx.execute(&stmt, &[&score_json, blake3])?;
        if i % 5000 == 0 && i > 0 {
            println!("   [Write] {}/{} rows updated...", i, updates.len());
        }
    }

    tx.commit()?;
    println!("Database update completed.");

    Ok(())
}
