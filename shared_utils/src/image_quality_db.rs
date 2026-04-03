use crate::image_analyzer::{analyze_image, ImageAnalysis};
use crate::progress_mode::emit_stderr;
use anyhow::{Context, Result};
use postgres::{Client, NoTls};
use serde::{Deserialize, Serialize};
use std::path::Path;

const PG_DEFAULT_CONNSTR: &str = "host=localhost dbname=modern_format_boost";
const QUALITY_IMPORT_KEY: &str = "quality_dataset_seeds_v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualitySampleMatch {
    pub quality_score: f64, // 0.0 to 1.0
    pub confidence: f64,
    pub neighbor_count: usize,
}

pub fn init_quality_schema(conn: &mut Client) -> Result<()> {
    emit_stderr("🐘 Initializing Static Image Quality Database (PostgreSQL + pgvector)...");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS quality_samples (
            file_hash TEXT PRIMARY KEY,
            source_path TEXT,
            format TEXT,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            file_size_bytes BIGINT NOT NULL,
            entropy DOUBLE PRECISION NOT NULL,
            compression_ratio DOUBLE PRECISION NOT NULL,
            spatial_bpp DOUBLE PRECISION NOT NULL,
            total_pixels BIGINT NOT NULL,
            is_lossless BOOLEAN NOT NULL,
            labeled_quality TEXT, -- 'high', 'low'
            labeled_by TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            features vector(6) -- [entropy, compression_ratio, spatial_bpp, log_pixels, aspect_ratio, is_lossless]
        )",
        &[],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_quality_lookup 
         ON quality_samples USING ivfflat (features l2_distance) WITH (lists = 100)",
        &[],
    )?;

    Ok(())
}

fn get_quality_features(analysis: &ImageAnalysis) -> Vec<f32> {
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = analysis.file_size as f64 / total_pixels.max(1.0);
    let aspect_ratio = if analysis.height > 0 {
        f64::from(analysis.width) / f64::from(analysis.height)
    } else {
        1.0
    };

    vec![
        analysis.features.entropy as f32,
        analysis.features.compression_ratio as f32,
        spatial_bpp as f32,
        (total_pixels.log10() as f32).max(0.0),
        aspect_ratio as f32,
        if analysis.is_lossless { 1.0 } else { 0.0 },
    ]
}

pub fn ingest_quality_sample(
    conn: &mut Client,
    path: &Path,
    label: &str,
    labeled_by: &str,
) -> Result<()> {
    let analysis = analyze_image(path).context("Failed to analyze image for quality DB")?;

    // Ignore animated images in the Static Quality DB
    if analysis.is_animated {
        return Ok(());
    }

    let file_hash = crate::common_utils::calculate_blake3_hash(path)?;
    let total_pixels = i64::from(analysis.width) * i64::from(analysis.height);
    let spatial_bpp = analysis.file_size as f64 / (total_pixels as f64).max(1.0);

    let features = get_quality_features(&analysis);

    conn.execute(
        "INSERT INTO quality_samples (
            file_hash, source_path, format, width, height, file_size_bytes,
            entropy, compression_ratio, spatial_bpp, total_pixels, is_lossless,
            labeled_quality, labeled_by, features
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT (file_hash) DO UPDATE SET 
            labeled_quality = $12, 
            labeled_by = $13",
        &[
            &file_hash,
            &path.to_string_lossy().to_string(),
            &analysis.format,
            &(analysis.width as i32),
            &(analysis.height as i32),
            &(analysis.file_size as i64),
            &analysis.features.entropy,
            &analysis.features.compression_ratio,
            &spatial_bpp,
            &total_pixels,
            &analysis.is_lossless,
            &label,
            &labeled_by,
            &features,
        ],
    )?;

    Ok(())
}

pub fn lookup_image_quality(analysis: &ImageAnalysis) -> Option<QualityScore> {
    if analysis.is_animated {
        return None;
    }

    let mut conn = Client::connect(PG_DEFAULT_CONNSTR, NoTls).ok()?;
    let features = get_quality_features(analysis);

    let rows = conn
        .query(
            "SELECT labeled_quality, features <=> $1 as distance
         FROM quality_samples
         ORDER BY distance ASC
         LIMIT 11",
            &[&features],
        )
        .ok()?;

    if rows.is_empty() {
        return None;
    }

    let mut high_count = 0;
    let mut total_count = 0;
    for row in rows {
        let label: String = row.get(0);
        if label.contains("high") {
            high_count += 1;
        }
        total_count += 1;
    }

    Some(QualityScore {
        score: high_count as f64 / total_count as f64,
        confidence: (total_count as f64 / 11.0).min(1.0),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    pub score: f64,
    pub confidence: f64,
}
