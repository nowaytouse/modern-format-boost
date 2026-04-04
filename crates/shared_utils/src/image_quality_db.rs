//! Static Image Quality Database
//!
//! Provides a KNN-based quality lookup for static (non-animated) images backed by
//! PostgreSQL + pgvector. Architecture mirrors the animated-media pipeline:
//!
//! - **Layer 0**: BPP heuristic fallback when DB is unavailable or empty (`confidence = 0.0`).
//! - **Layer 6**: HNSW + L2 KNN lookup against `quality_samples` table.
//! - **Level 4**: Fire-and-forget inference logging to `quality_inference_log` for
//!   future calibration and blind-spot discovery.
//!
//! Controlled by the `MODERN_FORMAT_DISABLE_IMAGE_QUALITY_DB` environment variable.

use crate::image_analyzer::ImageAnalysis;
use crate::progress_mode::emit_stderr;
use anyhow::{Context, Result};
use postgres::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Public types ─────────────────────────────────────────────────────────────

/// Result of a KNN or heuristic quality lookup.
///
/// `confidence = 0.0` signals a heuristic-only (BPP fallback) result with no DB backing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    /// Fraction of neighbors labeled "high" quality. 0.0 = all low, 1.0 = all high.
    pub score: f64,
    /// How many neighbors were found relative to the target K. 0.0 = heuristic only.
    pub confidence: f64,
}

/// Record logged to `quality_inference_log` on every `lookup_image_quality` call.
#[derive(Debug, Clone)]
pub struct QualityInferenceRecord {
    pub knn_score: f64,
    pub knn_confidence: f64,
    pub knn_neighbor_count: usize,
    pub bpp_fallback_score: Option<f64>,
    pub final_verdict: String,
}

// ── Schema initialisation ────────────────────────────────────────────────────

/// Create or migrate the static image quality schema.
///
/// Safe to call on every startup — all DDL is idempotent.
pub fn init_quality_schema(conn: &mut Client) -> Result<()> {
    emit_stderr("🐘 Initializing Static Image Quality Database (PostgreSQL + pgvector)...");

    // Core sample table
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

    // HNSW index for efficient L2 nearest-neighbour search.
    // Replaces the old ivfflat index; pgvector 0.5+ required.
    // The old cosine index (if present) is left intact — it won't conflict.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_quality_hnsw
         ON quality_samples USING hnsw (features vector_l2_ops)",
        &[],
    )?;

    // Level 4 inference log — mirrors the animated-media `inference_log` table.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS quality_inference_log (
            id BIGSERIAL PRIMARY KEY,
            file_hash TEXT,
            source_path TEXT,
            -- Raw signal snapshot
            entropy DOUBLE PRECISION,
            compression_ratio DOUBLE PRECISION,
            spatial_bpp DOUBLE PRECISION,
            log_pixels DOUBLE PRECISION,
            aspect_ratio DOUBLE PRECISION,
            is_lossless BOOLEAN,
            -- Decision metadata
            knn_score DOUBLE PRECISION NOT NULL,
            knn_confidence DOUBLE PRECISION NOT NULL,
            knn_neighbor_count INTEGER NOT NULL,
            bpp_fallback_score DOUBLE PRECISION,
            final_verdict TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;

    Ok(())
}

// ── Feature vector ───────────────────────────────────────────────────────────

/// Compute the 6-dimensional feature vector for a static image.
///
/// Dimensions (in order):
/// `[entropy, compression_ratio, spatial_bpp, log10(pixels), aspect_ratio, is_lossless]`
fn get_quality_features(analysis: &ImageAnalysis) -> pgvector::Vector {
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = analysis.file_size as f64 / total_pixels.max(1.0);
    let aspect_ratio = if analysis.height > 0 {
        f64::from(analysis.width) / f64::from(analysis.height)
    } else {
        1.0
    };

    pgvector::Vector::from(vec![
        analysis.features.entropy as f32,
        analysis.features.compression_ratio as f32,
        spatial_bpp as f32,
        (total_pixels.log10() as f32).max(0.0),
        aspect_ratio as f32,
        if analysis.is_lossless {
            1.0_f32
        } else {
            0.0_f32
        },
    ])
}

// ── BPP heuristic fallback (Layer 0) ─────────────────────────────────────────

/// Estimate quality purely from signal features when the DB is unavailable.
///
/// Returns `confidence = 0.0` to clearly signal this is heuristic-only.
fn bpp_heuristic_quality(analysis: &ImageAnalysis) -> QualityScore {
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = analysis.file_size as f64 / total_pixels.max(1.0);

    // High entropy + low BPP (efficient encoding) → high quality signal.
    // Scale: entropy [0, 8 bits max], spatial_bpp typical range [0.05, 20.0].
    let entropy_score = (analysis.features.entropy / 8.0).clamp(0.0, 1.0);
    let bpp_score = (1.0 - (spatial_bpp / 20.0).clamp(0.0, 1.0)).max(0.0);
    let lossless_bonus = if analysis.is_lossless { 0.1 } else { 0.0 };

    let score = (entropy_score * 0.5 + bpp_score * 0.5 + lossless_bonus).clamp(0.0, 1.0);
    QualityScore {
        score,
        confidence: 0.0,
    }
}

// ── DB Maturity ───────────────────────────────────────────────────────────────

/// Validates whether the static database has enough diverse samples to merit KNN lookup.
fn check_quality_db_maturity(conn: &mut Client) -> bool {
    let Ok(rows) = conn.query(
        "SELECT labeled_quality, count(*) FROM quality_samples WHERE labeled_quality IN ('high', 'low') GROUP BY labeled_quality",
        &[],
    ) else {
        return false;
    };

    let mut high_count: i64 = 0;
    let mut low_count: i64 = 0;
    for row in rows {
        let class: String = row.get(0);
        let count: i64 = row.get(1);
        if class == "high" {
            high_count = count;
        } else if class == "low" {
            low_count = count;
        }
    }

    let total = high_count + low_count;
    total >= crate::constants::MIN_QUALITY_SAMPLES_TOTAL
        && high_count >= crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS
        && low_count >= crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS
}

// ── KNN Lookup ────────────────────────────────────────────────────────────────

/// Look up the quality profile of a static image against the KNN database.
///
/// Returns `None` only for animated images.
/// Returns a heuristic `QualityScore` (confidence = 0.0) when the DB is unavailable or empty.
pub fn lookup_image_quality(analysis: &ImageAnalysis) -> Option<QualityScore> {
    // Animated assets are handled by the GIF/Video pipeline, not this DB.
    if analysis.is_animated {
        return None;
    }

    let disable_db = std::env::var(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB)
        .map(|v| v == "1" || v.to_ascii_lowercase() == "true")
        .unwrap_or(false)
        || std::env::var(crate::constants::ENV_DISABLE_DB_FEEDBACK)
            .map(|v| v == "1")
            .unwrap_or(false);

    if disable_db {
        let heuristic = bpp_heuristic_quality(analysis);
        return Some(heuristic);
    }

    // Use the shared pg client (which prints the "DB unavailable" warning at most once).
    let Ok(mut conn) = crate::gif_value_db::open_pg_client() else {
        return Some(bpp_heuristic_quality(analysis));
    };

    if !check_quality_db_maturity(&mut conn) {
        log::info!("🔬 Static Image Database is immature (needs >={} total, >={} per class). Bypassing KNN.", 
            crate::constants::MIN_QUALITY_SAMPLES_TOTAL, crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS);
        let bpp = bpp_heuristic_quality(analysis);
        // We still log the inference record even if immature, to gather blind spots
        let record = QualityInferenceRecord {
            knn_score: 0.0,
            knn_confidence: 0.0,
            knn_neighbor_count: 0,
            bpp_fallback_score: Some(bpp.score),
            final_verdict: "immature_bypass".to_string(),
        };
        log_quality_inference_record(&mut conn, analysis, None, &record);
        return Some(bpp);
    }

    let features = get_quality_features(analysis);

    // L2 nearest-neighbour query against the HNSW index.
    // `<->` is the L2 (Euclidean) distance operator in pgvector.
    let target_k: i64 = 11;
    let Ok(rows) = conn.query(
        "SELECT labeled_quality, features <-> $1 AS distance
         FROM quality_samples
         WHERE labeled_quality IS NOT NULL
         ORDER BY distance ASC
         LIMIT $2",
        &[&features, &target_k],
    ) else {
        return Some(bpp_heuristic_quality(analysis));
    };

    if rows.is_empty() {
        // DB connected but empty — return BPP heuristic.
        return Some(bpp_heuristic_quality(analysis));
    }

    let mut high_count = 0usize;
    let total_count = rows.len();
    for row in &rows {
        let label: String = row.get(0);
        if label.contains("high") {
            high_count += 1;
        }
    }

    let knn_score = high_count as f64 / total_count as f64;
    let knn_confidence = (total_count as f64 / target_k as f64).min(1.0);
    let bpp = bpp_heuristic_quality(analysis);

    let record = QualityInferenceRecord {
        knn_score,
        knn_confidence,
        knn_neighbor_count: total_count,
        bpp_fallback_score: Some(bpp.score),
        final_verdict: if knn_score >= 0.5 {
            "high".to_string()
        } else {
            "low".to_string()
        },
    };

    // Fire-and-forget inference log — never blocks the pipeline.
    log_quality_inference_record(&mut conn, analysis, None, &record);

    Some(QualityScore {
        score: knn_score,
        confidence: knn_confidence,
    })
}

// ── Level 4: Inference Logging ────────────────────────────────────────────────

/// Write one inference record to `quality_inference_log`. Fails silently.
pub fn log_quality_inference_record(
    conn: &mut Client,
    analysis: &ImageAnalysis,
    path: Option<&Path>,
    record: &QualityInferenceRecord,
) {
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = analysis.file_size as f64 / total_pixels.max(1.0);
    let log_pixels = total_pixels.log10();
    let aspect_ratio = if analysis.height > 0 {
        f64::from(analysis.width) / f64::from(analysis.height)
    } else {
        1.0
    };

    let file_hash: Option<String> =
        path.and_then(|p| crate::common_utils::calculate_blake3_hash(p).ok());
    let source_path: Option<String> = path.map(|p| p.display().to_string());
    let neighbor_count_i32 = record.knn_neighbor_count as i32;

    let result = conn.execute(
        "INSERT INTO quality_inference_log (
            file_hash, source_path,
            entropy, compression_ratio, spatial_bpp, log_pixels, aspect_ratio, is_lossless,
            knn_score, knn_confidence, knn_neighbor_count,
            bpp_fallback_score, final_verdict
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        &[
            &file_hash,
            &source_path,
            &analysis.features.entropy,
            &analysis.features.compression_ratio,
            &spatial_bpp,
            &log_pixels,
            &aspect_ratio,
            &analysis.is_lossless,
            &record.knn_score,
            &record.knn_confidence,
            &neighbor_count_i32,
            &record.bpp_fallback_score,
            &record.final_verdict,
        ],
    );

    if let Err(e) = result {
        log::warn!("⚠️ Failed to write quality inference log (non-fatal): {e}");
    }
}

// ── Sample ingestion ──────────────────────────────────────────────────────────

/// Ingest a labelled static image into the quality training set.
pub fn ingest_quality_sample(
    conn: &mut Client,
    path: &Path,
    label: &str,
    labeled_by: &str,
) -> Result<()> {
    use crate::image_analyzer::analyze_image;

    let analysis = analyze_image(path).context("Failed to analyze image for quality DB")?;

    // Ignore animated images in the Static Quality DB.
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
