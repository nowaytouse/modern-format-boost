//! Backfill stale quality embedding measurement sentinels.

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use std::process::Command;

const DEFAULT_CONNSTR: &str = "postgresql://localhost/modern_format_boost";
const EMBED_SLOT_INDICES: &[usize] = &[12, 17, 18, 19, 20];
const PGVECTOR_MISSING_MEASUREMENT: f64 = -1.0;
const NULLABLE_EMBED_FEATURES: &[&str] = &[
    "embedding_012",
    "embedding_017",
    "embedding_018",
    "embedding_019",
    "embedding_020",
];
const QUALITY_TABLES: &[&str] = &[
    "image_quality_samples",
    "animated_image_quality_samples",
    "video_quality_samples",
];

#[derive(Parser, Debug)]
#[command(
    name = "normalize_stale_embed_measurement_slots",
    about = "Normalize stale 0.0 optional measurement embedding slots to pgvector-safe sentinel"
)]
struct Args {
    #[arg(long, default_value = DEFAULT_CONNSTR)]
    connstr: String,

    #[arg(long)]
    dry_run: bool,
}

fn parse_pgvector(text: &str) -> Result<Vec<f64>> {
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<f64>()
                .with_context(|| format!("parse pgvector component {part:?}"))
        })
        .collect()
}

fn format_pgvector(values: &[f64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn normalize_vector(embedding: &[f64]) -> (Vec<f64>, usize) {
    let mut changed = 0;
    let mut out = embedding.to_vec();
    for index in EMBED_SLOT_INDICES {
        if let Some(value) = out.get_mut(*index)
            && *value == 0.0
        {
            *value = PGVECTOR_MISSING_MEASUREMENT;
            changed += 1;
        }
    }
    (out, changed)
}

fn psql(connstr: &str, sql: &str) -> Result<String> {
    let output = Command::new("psql")
        .arg(connstr)
        .args(["-At", "-F", "\t", "-c", sql])
        .output()
        .with_context(|| "psql is required for DB sentinel backfill".to_string())?;
    if !output.status.success() {
        return Err(anyhow!(
            "psql failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn update_embedding(connstr: &str, table: &str, id: &str, vector: &str) -> Result<()> {
    let escaped = vector.replace('\'', "''");
    let id_escaped = id.replace('\'', "''");
    let sql = format!(
        "UPDATE {table} SET embedding = '{escaped}'::vector WHERE id::text = '{id_escaped}'"
    );
    let _ = psql(connstr, &sql)?;
    Ok(())
}

fn run(connstr: &str, dry_run: bool) -> Result<(usize, usize)> {
    let mut total_rows = 0;
    let mut total_slots = 0;
    for table in QUALITY_TABLES {
        let rows = psql(
            connstr,
            &format!("SELECT id::text, embedding::text FROM {table} WHERE embedding IS NOT NULL"),
        )?;
        for line in rows.lines().filter(|line| !line.trim().is_empty()) {
            let Some((id, embedding_text)) = line.split_once('\t') else {
                return Err(anyhow!("unexpected psql row for {table}: {line:?}"));
            };
            let vec = parse_pgvector(embedding_text)?;
            let max_slot = EMBED_SLOT_INDICES
                .iter()
                .copied()
                .max()
                .ok_or_else(|| anyhow!("EMBED_SLOT_INDICES cannot be empty"))?;
            if vec.len() < max_slot + 1 {
                continue;
            }
            let (normalized, changed) = normalize_vector(&vec);
            if changed == 0 {
                continue;
            }
            total_rows += 1;
            total_slots += changed;
            if dry_run {
                println!(
                    "[dry-run] {table} id={id}: rewrite slots {:?} ({})",
                    EMBED_SLOT_INDICES,
                    NULLABLE_EMBED_FEATURES.join(", ")
                );
                continue;
            }
            update_embedding(connstr, table, id, &format_pgvector(&normalized))?;
        }
    }
    Ok((total_rows, total_slots))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (rows, slots) = run(&args.connstr, args.dry_run)?;
    let mode = if args.dry_run {
        "would update"
    } else {
        "updated"
    };
    println!(
        "{mode} {rows} row(s); {slots} embed slot(s) ({})",
        NULLABLE_EMBED_FEATURES.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_format_pgvector() -> Result<()> {
        let values = parse_pgvector("[1,0,-1.5]")?;
        assert_eq!(values, vec![1.0, 0.0, -1.5]);
        assert_eq!(format_pgvector(&values), "[1,0,-1.5]");
        Ok(())
    }

    #[test]
    fn test_normalize_vector_rewrites_optional_slots() {
        let mut values = vec![2.0; 21];
        for index in EMBED_SLOT_INDICES {
            values[*index] = 0.0;
        }
        let (normalized, changed) = normalize_vector(&values);
        assert_eq!(changed, EMBED_SLOT_INDICES.len());
        for index in EMBED_SLOT_INDICES {
            assert_eq!(normalized[*index], PGVECTOR_MISSING_MEASUREMENT);
        }
    }
}
