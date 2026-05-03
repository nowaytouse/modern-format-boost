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
/// `fallback_reason` explains why KNN was unavailable or unusable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    /// Fraction of neighbors labeled "high" quality. 0.0 = all low, 1.0 = all high.
    pub score: f64,
    /// How many neighbors were found relative to the target K. 0.0 = heuristic only.
    pub confidence: f64,
    /// Explicit reason why this score came from the heuristic path instead of KNN.
    pub fallback_reason: Option<String>,
}

/// Record logged to `quality_inference_log` on every `lookup_image_quality` call.
#[derive(Debug, Clone)]
pub struct QualityInferenceRecord {
    pub knn_score: Option<f64>,
    pub knn_confidence: Option<f64>,
    pub knn_neighbor_count: Option<usize>,
    pub bpp_fallback_score: Option<f64>,
    pub final_verdict: String,
}

// ── Schema initialisation ────────────────────────────────────────────────────

/// Create or migrate the static image quality schema.
///
/// Safe to call on every startup — all DDL is idempotent.
/// Initialize the `quality_samples` and `quality_inference_log` tables.
///
/// # Errors
/// Returns an error if the database schema cannot be initialized.
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
            knn_score DOUBLE PRECISION,
            knn_confidence DOUBLE PRECISION,
            knn_neighbor_count INTEGER,
            bpp_fallback_score DOUBLE PRECISION,
            final_verdict TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;

    apply_schema_migrations(conn)
}

fn apply_schema_migrations(conn: &mut Client) -> Result<()> {
    // Ensure exhaustive schema migration for existing tables
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS entropy DOUBLE PRECISION",
        &[],
    );
    let _ = conn.execute("ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS compression_ratio DOUBLE PRECISION", &[]);
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS spatial_bpp DOUBLE PRECISION",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS log_pixels DOUBLE PRECISION",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS aspect_ratio DOUBLE PRECISION",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS is_lossless BOOLEAN",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS knn_score DOUBLE PRECISION",
        &[],
    );
    let _ = conn.execute("ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS knn_confidence DOUBLE PRECISION", &[]);
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS knn_neighbor_count INTEGER",
        &[],
    );
    let _ = conn.execute("ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS bpp_fallback_score DOUBLE PRECISION", &[]);
    let _ = conn.execute("ALTER TABLE quality_inference_log ADD COLUMN IF NOT EXISTS final_verdict TEXT DEFAULT 'low'", &[]);

    // Remove legacy defaults/constraints that silently collapsed unknown KNN fields into zero.
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ALTER COLUMN knn_score DROP DEFAULT",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ALTER COLUMN knn_confidence DROP DEFAULT",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ALTER COLUMN knn_neighbor_count DROP DEFAULT",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ALTER COLUMN knn_score DROP NOT NULL",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ALTER COLUMN knn_confidence DROP NOT NULL",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ALTER COLUMN knn_neighbor_count DROP NOT NULL",
        &[],
    );
    let _ = conn.execute(
        "ALTER TABLE quality_inference_log ALTER COLUMN final_verdict SET NOT NULL",
        &[],
    );

    Ok(())
}

// ── Feature vector ───────────────────────────────────────────────────────────

/// Compute the 6-dimensional normalized feature vector for a static image.
///
/// Weighting and Scaling Philosophy:
/// - Entropy [0-8] -> [0-1]: High entropy is the primary high-quality signal.
/// - BPP [0.1-30] -> [0-1] (log): Spatial density indicator. We use log to prevent outliers.
/// - Pixels -> [0-1] (log10): Size context.
/// - Lossless -> [0, 2.0]: Very strong anchor to prioritize format fidelity.
fn get_quality_features(analysis: &ImageAnalysis) -> pgvector::Vector {
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = crate::numeric_cast::u64_to_f64(analysis.file_size) / total_pixels.max(1.0);
    let aspect_ratio = if analysis.height > 0 {
        f64::from(analysis.width) / f64::from(analysis.height)
    } else {
        1.0
    };

    pgvector::Vector::from(vec![
        (crate::numeric_cast::f64_to_f32_lossy(analysis.features.entropy) / 8.0).clamp(0.0, 1.0),
        ((crate::numeric_cast::f64_to_f32_lossy(analysis.features.compression_ratio)).ln_1p()
            / 3.0),
        ((crate::numeric_cast::f64_to_f32_lossy(spatial_bpp)).ln_1p() / 3.5),
        (crate::numeric_cast::f64_to_f32_lossy(total_pixels.max(1.0).log10()) / 10.0),
        ((crate::numeric_cast::f64_to_f32_lossy(aspect_ratio)).ln_1p() / 2.5),
        if analysis.is_lossless {
            2.0_f32
        } else {
            0.0_f32
        },
    ])
}

// ── BPP heuristic fallback (Layer 0) ─────────────────────────────────────────

/// Estimate quality purely from signal features when the DB is unavailable.
///
/// Returns `confidence = 0.0` to clearly signal this is heuristic-only.
fn bpp_heuristic_score(analysis: &ImageAnalysis) -> f64 {
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = crate::numeric_cast::u64_to_f64(analysis.file_size) / total_pixels.max(1.0);

    // High entropy + low BPP (efficient encoding) → high quality signal.
    // Scale: entropy [0, 8 bits max], spatial_bpp typical range [0.05, 20.0].
    let entropy_score = (analysis.features.entropy / 8.0).clamp(0.0, 1.0);
    let bpp_score = (1.0 - (spatial_bpp / 20.0).clamp(0.0, 1.0)).max(0.0);
    let lossless_bonus = if analysis.is_lossless { 0.1 } else { 0.0 };

    (entropy_score * 0.5 + bpp_score * 0.5 + lossless_bonus).clamp(0.0, 1.0)
}

fn bpp_heuristic_quality(analysis: &ImageAnalysis, reason: impl Into<String>) -> QualityScore {
    QualityScore {
        score: bpp_heuristic_score(analysis),
        confidence: 0.0,
        fallback_reason: Some(reason.into()),
    }
}

// ── DB Maturity ───────────────────────────────────────────────────────────────

/// Validates whether the static database has enough diverse samples to merit KNN lookup.
pub fn check_quality_db_maturity(conn: &mut Client) -> bool {
    let (high_count, low_count) = get_class_counts(conn);

    let total = high_count + low_count;
    total >= crate::constants::MIN_QUALITY_SAMPLES_TOTAL
        && high_count >= crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS
        && low_count >= crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS
}

/// Retrieves the count of samples in 'high' and 'low' quality classes.
pub fn get_class_counts(conn: &mut Client) -> (i64, i64) {
    let Ok(rows) = conn.query(
        "SELECT 
            CASE WHEN labeled_quality LIKE '%high%' THEN 'high' 
                 WHEN labeled_quality LIKE '%low%' THEN 'low' 
                 ELSE 'other' 
            END as class, 
            count(*) 
         FROM quality_samples 
         GROUP BY 1",
        &[],
    ) else {
        return (0, 0);
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
    (high_count, low_count)
}

// ── KNN Lookup ────────────────────────────────────────────────────────────────

/// Look up the quality profile of a static image against the KNN database.
///
/// Returns `None` only for animated images.
/// Returns a heuristic `QualityScore` (confidence = 0.0) when the DB is unavailable or empty.
#[must_use]
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
pub fn lookup_image_quality(analysis: &ImageAnalysis) -> Option<QualityScore> {
    // Animated assets are handled by the GIF/Video pipeline, not this DB.
    if analysis.is_animated {
        return None;
    }

    let disable_db = std::env::var(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB)
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        || std::env::var(crate::constants::ENV_DISABLE_DB_FEEDBACK).is_ok_and(|v| v == "1");

    if disable_db {
        emit_stderr("  ⚠️ Static image quality DB disabled — using heuristic score only");
        let heuristic = bpp_heuristic_quality(analysis, "Static image quality DB disabled");
        return Some(heuristic);
    }

    let force_knn = std::env::var(crate::constants::ENV_FORCE_QUALITY_KNN)
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    if force_knn {
        crate::progress_mode::emit_stderr("  🐘 [OVERRIDE] Forcing KNN Database lookup...");
    }

    // Use the shared pg client (which prints the "DB unavailable" warning at most once).
    let Ok(mut conn) = crate::database::open_pg_client() else {
        emit_stderr("  ⚠️ Static image quality DB unavailable — using heuristic score only");
        return Some(bpp_heuristic_quality(
            analysis,
            "Static image quality DB unavailable",
        ));
    };

    if !force_knn && !check_quality_db_maturity(&mut conn) {
        let (high_total, low_total) = get_class_counts(&mut conn);
        log::info!(
            "🔬 Static Image Database is immature (high={high_total}, low={low_total}). Bypassing KNN."
        );
        emit_stderr(&format!(
            "  ⚠️ Static image quality DB immature (high={high_total}, low={low_total}) — using heuristic score only"
        ));
        let bpp = bpp_heuristic_quality(analysis, "Static image quality DB immature");
        // We still log the inference record even if immature, to gather blind spots
        let record = QualityInferenceRecord {
            knn_score: None,
            knn_confidence: None,
            knn_neighbor_count: None,
            bpp_fallback_score: Some(bpp.score),
            final_verdict: "heuristic_db_immature".to_string(),
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
        emit_stderr("  ⚠️ Static image KNN query failed — using heuristic score only");
        let bpp = bpp_heuristic_quality(analysis, "Static image KNN query failed");
        let record = QualityInferenceRecord {
            knn_score: None,
            knn_confidence: None,
            knn_neighbor_count: None,
            bpp_fallback_score: Some(bpp.score),
            final_verdict: "heuristic_query_failed".to_string(),
        };
        log_quality_inference_record(&mut conn, analysis, None, &record);
        return Some(bpp);
    };

    let mut total_weight = 0.0f64;
    let mut high_weight = 0.0f64;
    let total_count = rows.len();

    if total_count == 0 {
        emit_stderr("  ⚠️ Static image KNN returned no neighbors — using heuristic score only");
        let bpp = bpp_heuristic_quality(analysis, "Static image KNN returned no neighbors");
        let record = QualityInferenceRecord {
            knn_score: None,
            knn_confidence: None,
            knn_neighbor_count: Some(0),
            bpp_fallback_score: Some(bpp.score),
            final_verdict: "heuristic_no_neighbors".to_string(),
        };
        log_quality_inference_record(&mut conn, analysis, None, &record);
        return Some(bpp);
    }

    // Calculate Dynamic Class Balancing Factors based on database distribution.
    let (high_total, low_total) = get_class_counts(&mut conn);
    let total_db_samples = high_total + low_total;

    // Factor = Total / (NumClasses * ClassCount)
    let high_factor = if high_total > 0 {
        crate::numeric_cast::i64_to_f64(total_db_samples)
            / (2.0 * crate::numeric_cast::i64_to_f64(high_total))
    } else {
        1.0
    };
    let low_factor = if low_total > 0 {
        crate::numeric_cast::i64_to_f64(total_db_samples)
            / (2.0 * crate::numeric_cast::i64_to_f64(low_total))
    } else {
        1.0
    };

    for row in rows {
        let label: String = row.get(0);
        let distance: f64 = row.get(1);

        // Inverse Distance Weighting (IDW)
        let mut weight = 1.0f64 / (distance + 0.01);

        if label.contains("high") {
            weight *= high_factor;
            high_weight += weight;
        } else {
            weight *= low_factor;
        }
        total_weight += weight;
    }

    if total_weight <= 0.0 {
        emit_stderr(
            "  ⚠️ Static image KNN produced zero usable weight — using heuristic score only",
        );
        let bpp = bpp_heuristic_quality(analysis, "Static image KNN produced zero usable weight");
        let record = QualityInferenceRecord {
            knn_score: None,
            knn_confidence: None,
            knn_neighbor_count: Some(total_count),
            bpp_fallback_score: Some(bpp.score),
            final_verdict: "heuristic_zero_knn_weight".to_string(),
        };
        log_quality_inference_record(&mut conn, analysis, None, &record);
        return Some(bpp);
    }

    let knn_score = high_weight / total_weight;
    let knn_confidence = (crate::numeric_cast::usize_to_f64(total_count)
        / crate::numeric_cast::i64_to_f64(target_k.max(1)))
    .min(1.0);
    let bpp_score = bpp_heuristic_score(analysis);

    let record = QualityInferenceRecord {
        knn_score: Some(knn_score),
        knn_confidence: Some(knn_confidence),
        knn_neighbor_count: Some(total_count),
        bpp_fallback_score: Some(bpp_score),
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
        fallback_reason: None,
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
    let spatial_bpp = crate::numeric_cast::u64_to_f64(analysis.file_size) / total_pixels.max(1.0);
    let log_pixels = total_pixels.log10();
    let aspect_ratio = if analysis.height > 0 {
        f64::from(analysis.width) / f64::from(analysis.height)
    } else {
        1.0
    };

    let file_hash: Option<String> =
        path.and_then(|p| crate::common_utils::calculate_blake3_hash(p).ok());
    let source_path: Option<String> = path.map(|p| p.display().to_string());
    let neighbor_count_i32 = record
        .knn_neighbor_count
        .and_then(|n| i32::try_from(n).ok());

    let f64_safe = |v: f64| if v.is_finite() { Some(v) } else { None };
    let entropy = f64_safe(analysis.features.entropy);
    let compression = f64_safe(analysis.features.compression_ratio);
    let bpp = f64_safe(spatial_bpp);
    let lp = f64_safe(log_pixels);
    let ar = f64_safe(aspect_ratio);
    let is_lossless = analysis.is_lossless;
    let knn_score = record.knn_score.and_then(f64_safe);
    let knn_conf = record.knn_confidence.and_then(f64_safe);
    let bpp_fallback = record.bpp_fallback_score.and_then(f64_safe);
    let verdict = &record.final_verdict;

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
            &entropy,
            &compression,
            &bpp,
            &lp,
            &ar,
            &is_lossless,
            &knn_score,
            &knn_conf,
            &neighbor_count_i32,
            &bpp_fallback,
            verdict,
        ],
    );

    if let Err(e) = result {
        log::warn!(
            "⚠️ Failed to write quality inference log (non-fatal): {e} | Verdict: {}",
            record.final_verdict
        );
    }
}

// ── Sample ingestion ──────────────────────────────────────────────────────────

/// Ingest a labelled static image into the quality training set.
/// Ingest a quality sample into the database.
///
/// # Errors
/// Returns an error if the sample cannot be ingested.
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
    let spatial_bpp = crate::numeric_cast::u64_to_f64(analysis.file_size)
        / crate::numeric_cast::i64_to_f64(total_pixels).max(1.0);
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
            &(i32::try_from(analysis.width).unwrap_or(0)),
            &(i32::try_from(analysis.height).unwrap_or(0)),
            &(i64::try_from(analysis.file_size).unwrap_or(0)),
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
