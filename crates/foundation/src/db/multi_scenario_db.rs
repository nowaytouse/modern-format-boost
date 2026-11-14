// Multi-Scenario Database Operations
//
// Provides unified interface for managing embeddings across different
// scenarios. Strictly aligned with the 001_multi_scenario_embedding.sql schema.

use crate::scenario::ScenarioType;
use anyhow::{Context, Result};
use postgres::Client;
use serde::{Deserialize, Serialize};

pub(crate) const MULTI_SCENARIO_SCHEMA_ADVISORY_LOCK_KEY: i64 = 0x4D46_425F_5343_4845_i64;
pub(crate) const MULTI_SCENARIO_SCHEMA_DDL_ADVISORY_LOCK_KEY: i64 = 0x4D46_425F_4444_4C30_i64;
pub(crate) const PG_ADVISORY_LOCK_SQL: &str = "SELECT pg_advisory_lock($1)";
pub(crate) const PG_ADVISORY_UNLOCK_SQL: &str = "SELECT pg_advisory_unlock($1)";

/// Extracted `KNN` regression features intended for `LightGBM` consumption.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnnRegressionFeatures {
    pub knn_score_mean_k5: f64,
    pub knn_score_std_k5: f64,
    pub knn_score_min_k5: f64,
    pub dist_to_nearest: f64,
    pub dist_weighted_score: f64,
    pub confidence: f64,
    pub neighbor_count: usize,
}

impl KnnRegressionFeatures {
    /// Returns `true` when every scalar is finite and at least one neighbor was
    /// aggregated.
    #[must_use]
    pub const fn is_usable_for_regression(&self) -> bool {
        self.neighbor_count > 0
            && self.knn_score_mean_k5.is_finite()
            && self.knn_score_std_k5.is_finite()
            && self.knn_score_min_k5.is_finite()
            && self.dist_to_nearest.is_finite()
            && self.dist_weighted_score.is_finite()
            && self.confidence.is_finite()
    }

    /// Seal aggregated KNN scalars; reject the whole row when any field is
    /// non-contract.
    #[must_use]
    pub fn seal_aggregates(self) -> Option<Self> {
        Some(Self {
            knn_score_mean_k5: crate::algorithm_seal::quality_unit_probability(
                self.knn_score_mean_k5,
            )?,
            knn_score_std_k5: crate::algorithm_seal::quality_finite_scalar(self.knn_score_std_k5)?,
            knn_score_min_k5: crate::algorithm_seal::quality_unit_probability(
                self.knn_score_min_k5,
            )?,
            dist_to_nearest: crate::algorithm_seal::quality_finite_scalar(self.dist_to_nearest)?,
            dist_weighted_score: crate::algorithm_seal::quality_unit_probability(
                self.dist_weighted_score,
            )?,
            confidence: crate::algorithm_seal::quality_unit_probability(self.confidence)?,
            neighbor_count: self.neighbor_count,
        })
    }
}

/// Unified scenario query configuration
#[derive(Debug, Clone)]
#[must_use]
pub struct ScenarioQuery {
    pub scenario: ScenarioType,
    pub k_neighbors: usize,
    pub threshold_distance: f64,
}

impl ScenarioQuery {
    pub const fn new(scenario: ScenarioType) -> Self {
        Self {
            scenario,
            k_neighbors: 5,
            threshold_distance: 2.0,
        }
    }

    pub const fn with_k(mut self, k: usize) -> Self {
        self.k_neighbors = k;
        self
    }

    pub const fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold_distance = threshold;
        self
    }
}

/// Sample record wrapper for scenario-agnostic operations
#[derive(Debug, Clone)]
#[must_use]
pub struct ScenarioSample {
    pub blake3: Vec<u8>,
    pub source_path: Option<String>,
    pub file_name: Option<String>,
    pub scenario: ScenarioType,
    pub label: Option<String>,
    pub embedding: Option<pgvector::Vector>,
    pub metadata: serde_json::Value,

    // Physical features
    pub width: i32,
    pub height: i32,
    pub file_size_bytes: i64,
    pub format: String,
    pub duration_secs: f64,
    pub frame_count: i64,
    pub fps: Option<f64>,

    // Physical Signal Integrity
    pub is_lossless: Option<bool>,
    pub is_meme: Option<bool>,
    pub entropy: Option<f64>,
    pub compression_ratio: Option<f64>,
    pub quality_score: Option<f32>,
    pub palette_size: Option<i32>,
    pub palette_depth: Option<f64>,
    pub animation_smoothness: Option<f64>,
    pub frame_delay_variation: Option<f64>,
    pub frame_payload_variation: Option<f64>,
    pub bitrate_mbps: Option<f64>,
    pub bit_depth: Option<i16>,
    pub has_audio: Option<bool>,
    pub is_variable_frame_rate: Option<bool>,
    pub is_hdr: Option<bool>,
    pub motion_intensity: Option<f64>,
    pub temporal_stability: Option<f64>,
    pub motion_gini: Option<f64>,
    pub loop_closure_score: Option<f64>,
    pub motion_periodicity: Option<f64>,
    pub temporal_jitter: Option<f64>,
    pub cadence_score: Option<f64>,
    pub labeled_by: Option<String>,
}

impl ScenarioSample {
    pub fn new(blake3: Vec<u8>, scenario: ScenarioType) -> Self {
        Self {
            blake3,
            scenario,
            source_path: None,
            file_name: None,
            label: None,
            embedding: None,
            metadata: serde_json::json!({}),
            width: 0,
            height: 0,
            file_size_bytes: 0,
            format: "unknown".to_string(),
            duration_secs: 0.0,
            frame_count: 0,
            fps: None,
            is_lossless: None,
            is_meme: None,
            entropy: None,
            compression_ratio: None,
            quality_score: None,
            palette_size: None,
            palette_depth: None,
            animation_smoothness: None,
            frame_delay_variation: None,
            frame_payload_variation: None,
            bitrate_mbps: None,
            bit_depth: None,
            has_audio: None,
            is_variable_frame_rate: None,
            is_hdr: None,
            motion_intensity: None,
            temporal_stability: None,
            motion_gini: None,
            loop_closure_score: None,
            motion_periodicity: None,
            temporal_jitter: None,
            cadence_score: None,
            labeled_by: None,
        }
    }

    pub fn with_path(mut self, path: String) -> Self {
        self.source_path = Some(path);
        self
    }

    pub fn with_file_name(mut self, file_name: String) -> Self {
        self.file_name = Some(file_name);
        self
    }

    pub fn with_label(mut self, label: String) -> Self {
        self.label = Some(label);
        self
    }

    pub fn with_embedding(mut self, embedding: pgvector::Vector) -> Self {
        self.embedding = Some(embedding);
        self
    }

    pub const fn with_dimensions(mut self, w: i32, h: i32) -> Self {
        self.width = w;
        self.height = h;
        self
    }

    pub const fn with_size(mut self, size: i64) -> Self {
        self.file_size_bytes = size;
        self
    }

    pub fn with_format(mut self, fmt: String) -> Self {
        self.format = fmt;
        self
    }

    pub const fn with_entropy(mut self, entropy: Option<f64>) -> Self {
        self.entropy = entropy;
        self
    }

    pub const fn with_compression_ratio(mut self, ratio: Option<f64>) -> Self {
        self.compression_ratio = ratio;
        self
    }

    pub const fn with_lossless(mut self, is_lossless: bool) -> Self {
        self.is_lossless = Some(is_lossless);
        self
    }

    pub const fn with_quality_score(mut self, score: f32) -> Self {
        self.quality_score = Some(score);
        self
    }

    pub const fn with_duration_secs(mut self, duration: f64) -> Self {
        self.duration_secs = duration;
        self
    }

    pub const fn with_frame_count(mut self, count: i64) -> Self {
        self.frame_count = count;
        self
    }

    pub const fn with_fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }

    pub const fn with_is_meme(mut self, is_meme: bool) -> Self {
        self.is_meme = Some(is_meme);
        self
    }

    pub const fn with_palette_size(mut self, palette_size: i32) -> Self {
        self.palette_size = Some(palette_size);
        self
    }

    pub const fn with_palette_depth(mut self, palette_depth: f64) -> Self {
        self.palette_depth = Some(palette_depth);
        self
    }

    pub const fn with_animation_smoothness(mut self, animation_smoothness: f64) -> Self {
        self.animation_smoothness = Some(animation_smoothness);
        self
    }

    pub const fn with_frame_delay_variation(mut self, frame_delay_variation: f64) -> Self {
        self.frame_delay_variation = Some(frame_delay_variation);
        self
    }

    pub const fn with_frame_delay_variation_opt(
        mut self,
        frame_delay_variation: Option<f64>,
    ) -> Self {
        self.frame_delay_variation = frame_delay_variation;
        self
    }

    pub const fn with_frame_payload_variation_opt(
        mut self,
        frame_payload_variation: Option<f64>,
    ) -> Self {
        self.frame_payload_variation = frame_payload_variation;
        self
    }

    pub const fn with_bitrate_mbps(mut self, bitrate_mbps: f64) -> Self {
        self.bitrate_mbps = Some(bitrate_mbps);
        self
    }

    pub const fn with_bit_depth_opt(mut self, bit_depth: Option<u8>) -> Self {
        self.bit_depth = match bit_depth {
            Some(value) => Some(i16::from_le_bytes([value, 0])),
            None => None,
        };
        self
    }

    pub const fn with_has_audio(mut self, has_audio: bool) -> Self {
        self.has_audio = Some(has_audio);
        self
    }

    pub const fn with_is_variable_frame_rate(mut self, is_variable_frame_rate: bool) -> Self {
        self.is_variable_frame_rate = Some(is_variable_frame_rate);
        self
    }

    pub const fn with_is_hdr(mut self, is_hdr: bool) -> Self {
        self.is_hdr = Some(is_hdr);
        self
    }

    pub const fn with_motion_intensity(mut self, motion_intensity: f64) -> Self {
        self.motion_intensity = Some(motion_intensity);
        self
    }

    pub const fn with_temporal_stability(mut self, temporal_stability: f64) -> Self {
        self.temporal_stability = Some(temporal_stability);
        self
    }

    pub const fn with_motion_gini_opt(mut self, motion_gini: Option<f64>) -> Self {
        self.motion_gini = motion_gini;
        self
    }

    pub const fn with_loop_closure_score_opt(mut self, loop_closure_score: Option<f64>) -> Self {
        self.loop_closure_score = loop_closure_score;
        self
    }

    pub const fn with_motion_periodicity_opt(mut self, motion_periodicity: Option<f64>) -> Self {
        self.motion_periodicity = motion_periodicity;
        self
    }

    pub const fn with_temporal_jitter_opt(mut self, temporal_jitter: Option<f64>) -> Self {
        self.temporal_jitter = temporal_jitter;
        self
    }

    pub const fn with_cadence_score(mut self, cadence_score: f64) -> Self {
        self.cadence_score = Some(cadence_score);
        self
    }

    pub fn with_labeled_by(mut self, labeled_by: String) -> Self {
        self.labeled_by = Some(labeled_by);
        self
    }
}

fn embedding_slot_allows_non_finite(scenario: ScenarioType, index: usize) -> bool {
    scenario == ScenarioType::ImageQuality
        && crate::image_quality_db::quality_embedding_slot_allows_non_finite(index)
}

fn validate_embedding(sample: &ScenarioSample) -> Result<()> {
    let embedding = sample
        .embedding
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Embedding required for {}", sample.scenario))?;
    let actual_dim = embedding.as_slice().len();
    let expected_dim = sample.scenario.embedding_dimension();
    if actual_dim != expected_dim {
        anyhow::bail!(
            "Embedding dimension mismatch for {}: expected {}, got {}",
            sample.scenario,
            expected_dim,
            actual_dim
        );
    }
    if sample.scenario == ScenarioType::ImageQuality {
        crate::image_quality_db::assert_quality_embedding_finite_policy(embedding.as_slice())?;
    } else if embedding
        .as_slice()
        .iter()
        .enumerate()
        .any(|(index, value)| {
            !value.is_finite() && !embedding_slot_allows_non_finite(sample.scenario, index)
        })
    {
        anyhow::bail!(
            "Embedding for {} contains non-finite values",
            sample.scenario
        );
    }
    let physics_range = match sample.scenario {
        ScenarioType::LoopIntent => 36..261,
        ScenarioType::ImageQuality => 31..256,
        ScenarioType::AnimatedImageQuality | ScenarioType::VideoQuality => 0..225,
    };
    if embedding.as_slice()[physics_range]
        .iter()
        .all(|value| value.abs() <= f32::EPSILON)
    {
        anyhow::bail!("{} embedding is missing its physics block", sample.scenario);
    }
    Ok(())
}

fn require_quality_score(sample: &ScenarioSample) -> Result<f32> {
    let quality_score = sample
        .quality_score
        .ok_or_else(|| anyhow::anyhow!("quality_score required for {}", sample.scenario))?;
    if !quality_score.is_finite() {
        anyhow::bail!("quality_score for {} must be finite", sample.scenario);
    }
    if !(0.0..=1.0).contains(&quality_score) {
        anyhow::bail!(
            "quality_score for {} must be within [0.0, 1.0], got {}",
            sample.scenario,
            quality_score
        );
    }
    Ok(quality_score)
}

fn ensure_quality_regression_scenario(scenario: ScenarioType) -> Result<()> {
    anyhow::ensure!(
        scenario.is_quality_regression(),
        "knn_regression_lookup only supports quality regression scenarios; use loop_intent \
         clustering via knn_lookup and the loop pipeline"
    );
    Ok(())
}

fn require_finite_real(value: f64, field: &str, sample: &ScenarioSample) -> Result<f64> {
    if !value.is_finite() {
        anyhow::bail!("{field} for {} must be finite", sample.scenario);
    }
    Ok(value)
}

fn require_finite_metric(value: Option<f64>, field: &str, sample: &ScenarioSample) -> Result<f64> {
    let value = value.ok_or_else(|| anyhow::anyhow!("{field} required for {}", sample.scenario))?;
    require_finite_real(value, field, sample)
}

fn normalize_optional_finite_real(
    value: Option<f64>,
    field: &str,
    sample: &ScenarioSample,
) -> Result<Option<f64>> {
    value
        .map(|value| require_finite_real(value, field, sample))
        .transpose()
}

fn normalize_optional_positive_real(
    value: Option<f64>,
    field: &str,
    sample: &ScenarioSample,
) -> Result<Option<f64>> {
    let value = normalize_optional_finite_real(value, field, sample)?;
    if let Some(value) = value
        && value <= 0.0
    {
        anyhow::bail!(
            "{field} for {} must be > 0 when present, got {value}",
            sample.scenario
        );
    }
    Ok(value)
}

fn normalize_optional_non_negative_i32(
    value: Option<i32>,
    field: &str,
    sample: &ScenarioSample,
) -> Result<Option<i32>> {
    if let Some(value) = value
        && value < 0
    {
        anyhow::bail!(
            "{field} for {} must be >= 0 when present, got {value}",
            sample.scenario
        );
    }
    Ok(value)
}

fn normalize_optional_positive_i16(
    value: Option<i16>,
    field: &str,
    sample: &ScenarioSample,
) -> Result<Option<i16>> {
    if let Some(value) = value
        && value <= 0
    {
        anyhow::bail!(
            "{field} for {} must be > 0 when present, got {value}",
            sample.scenario
        );
    }
    Ok(value)
}

fn resolve_image_quality_label(
    sample: &ScenarioSample,
    quality_score: f32,
) -> Result<crate::scenario::ImageQualityLabel> {
    let raw_label = sample
        .label
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("quality_label required for {}", sample.scenario))?;
    let canonical_label =
        crate::scenario::ImageQualityLabel::resolve_for_format(raw_label, &sample.format)?;
    let expected_score = canonical_label.to_score();
    if (quality_score - expected_score).abs() > f32::EPSILON {
        anyhow::bail!(
            "Image quality label '{}' implies score {}, but sample carries {}",
            canonical_label.as_str(),
            expected_score,
            quality_score
        );
    }
    Ok(canonical_label)
}

fn embedding_column_type(conn: &mut Client, table_name: &str) -> Result<Option<String>> {
    let row = conn.query_opt(
        "
        SELECT format_type(a.atttypid, a.atttypmod)
        FROM pg_attribute a
        JOIN pg_class c ON c.oid = a.attrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname = $1
          AND a.attname = 'embedding'
          AND NOT a.attisdropped
        ",
        &[&table_name],
    )?;
    Ok(row.map(|row| row.get(0)))
}

fn parse_vector_dimension(column_type: &str) -> Option<i32> {
    let inner = column_type.strip_prefix("vector(")?.strip_suffix(')')?;
    match inner.parse::<i32>() {
        Ok(value) => Some(value),
        Err(e) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "multi_scenario_vector_dimension",
                format!("failed to parse vector dimension from '{column_type}': {e}"),
            );
            None
        }
    }
}

fn table_row_count(conn: &mut Client, table_name: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table_name}");
    let row = conn
        .query_one(&sql, &[])
        .with_context(|| format!("Failed to count rows in {table_name}"))?;
    Ok(row.get(0))
}

fn ensure_embedding_column_dimension(
    conn: &mut Client,
    table_name: &str,
    expected_dim: i32,
) -> Result<()> {
    let Some(column_type) = embedding_column_type(conn, table_name)? else {
        return Ok(());
    };
    let Some(actual_dim) = parse_vector_dimension(&column_type) else {
        anyhow::bail!(
            "Unsupported embedding column type for {table_name}: {column_type} (expected \
             vector({expected_dim}))"
        );
    };

    if actual_dim == expected_dim {
        return Ok(());
    }
    if actual_dim > expected_dim {
        if table_row_count(conn, table_name)? == 0 {
            let sql = format!(
                "ALTER TABLE {table_name}
                 ALTER COLUMN embedding TYPE VECTOR({expected_dim})"
            );
            conn.execute(&sql, &[]).with_context(|| {
                format!(
                    "Failed to tighten empty {table_name}.embedding from vector({actual_dim}) to \
                     vector({expected_dim})"
                )
            })?;
            return Ok(());
        }
        anyhow::bail!(
            "Embedding column for {table_name} has dimension {actual_dim}, cannot tighten to \
             {expected_dim} without lossy truncation"
        );
    }

    let zero_pad_dims = expected_dim - actual_dim;
    let sql = format!(
        "ALTER TABLE {table_name}
         ALTER COLUMN embedding TYPE VECTOR({expected_dim})
         USING CASE
             WHEN embedding IS NULL THEN NULL
             ELSE regexp_replace(
                 embedding::text,
                 '\\]$',
                 repeat(',0', {zero_pad_dims}) || ']'
             )::vector({expected_dim})
         END"
    );
    conn.execute(&sql, &[]).with_context(|| {
        format!(
            "Failed to migrate {table_name}.embedding from vector({actual_dim}) to \
             vector({expected_dim})"
        )
    })?;
    Ok(())
}

/// Legacy loop-intent feedback table (`inference_log`) plus first-class audit
/// columns.
///
/// # Errors
///
/// Returns an error when `CREATE TABLE` / `ALTER TABLE` DDL fails.
fn ensure_loop_inference_log_schema(conn: &mut Client) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS inference_log (
            id BIGSERIAL PRIMARY KEY,
            file_hash TEXT,
            source_path TEXT,
            duration_secs DOUBLE PRECISION,
            webp_compression_ratio DOUBLE PRECISION,
            tree_probability DOUBLE PRECISION,
            knn_keep_probability DOUBLE PRECISION,
            knn_confidence DOUBLE PRECISION,
            knn_neighbor_count INTEGER,
            final_probability DOUBLE PRECISION,
            final_verdict TEXT,
            decision_reason TEXT,
            layer_exit TEXT,
            signal_snapshot JSONB,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE inference_log ADD COLUMN IF NOT EXISTS resolution_path TEXT",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE inference_log ADD COLUMN IF NOT EXISTS hnsw_lookup_branch TEXT",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE inference_log ADD COLUMN IF NOT EXISTS tree_log_odds DOUBLE PRECISION",
        &[],
    )?;
    Ok(())
}

/// Create all scenario-specific tables used by the multi-scenario embedding
/// layer.
///
/// This mirrors `migrations/001_multi_scenario_embedding.sql` and is
/// intentionally idempotent so training binaries and the C API can run without
/// a separate manual migration step.
///
/// # Errors
///
/// Returns an error if schema creation, index creation, or metadata refresh
/// fails.
pub fn init_multi_scenario_schema(conn: &mut Client) -> Result<()> {
    init_multi_scenario_schema_locked(conn)
}

pub(crate) fn with_multi_scenario_metadata_lock<T>(
    conn: &mut Client,
    op: impl FnOnce(&mut Client) -> Result<T>,
) -> Result<T> {
    with_multi_scenario_advisory_lock(conn, MULTI_SCENARIO_SCHEMA_ADVISORY_LOCK_KEY, op)
}

fn with_multi_scenario_schema_ddl_lock<T>(
    conn: &mut Client,
    op: impl FnOnce(&mut Client) -> Result<T>,
) -> Result<T> {
    with_multi_scenario_advisory_lock(conn, MULTI_SCENARIO_SCHEMA_DDL_ADVISORY_LOCK_KEY, op)
}

fn with_multi_scenario_advisory_lock<T>(
    conn: &mut Client,
    lock_key: i64,
    op: impl FnOnce(&mut Client) -> Result<T>,
) -> Result<T> {
    conn.query_one(PG_ADVISORY_LOCK_SQL, &[&lock_key])
        .map(|_| ())
        .context("multi-scenario schema advisory lock acquire failed")?;
    let result = op(conn);
    let unlock = conn
        .query_one(PG_ADVISORY_UNLOCK_SQL, &[&lock_key])
        .map(|row| row.get::<_, bool>(0))
        .context("multi-scenario schema advisory lock release failed");

    match (result, unlock) {
        (Ok(value), Ok(true)) => Ok(value),
        (Ok(_), Ok(false)) => {
            anyhow::bail!("multi-scenario schema advisory lock release returned false")
        }
        (Ok(_), Err(unlock_err)) => Err(unlock_err),
        (Err(op_err), Ok(true)) => Err(op_err),
        (Err(op_err), Ok(false)) => Err(op_err.context(
            "multi-scenario schema advisory lock release returned false after operation failure",
        )),
        (Err(op_err), Err(unlock_err)) => Err(op_err.context(format!(
            "multi-scenario schema advisory lock release also failed: {unlock_err:#}"
        ))),
    }
}

fn init_multi_scenario_schema_locked(conn: &mut Client) -> Result<()> {
    with_multi_scenario_schema_ddl_lock(conn, init_multi_scenario_schema_inner)
}

fn init_multi_scenario_schema_inner(conn: &mut Client) -> Result<()> {
    conn.execute("CREATE EXTENSION IF NOT EXISTS vector", &[])
        .context("Failed to enable pgvector extension")?;
    conn.batch_execute(
        "
        DO $$
        BEGIN
            IF EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'gif_quality_samples'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: gif_quality_samples. Remove or rename legacy \
         animated-image schema objects before initializing the strict animated_image_quality \
         schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'gif_quality_inference_log'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: gif_quality_inference_log. Remove or rename \
         legacy animated-image schema objects before initializing the strict \
         animated_image_quality schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM pg_class
                WHERE relkind = 'S' AND relname = 'gif_quality_samples_id_seq'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: gif_quality_samples_id_seq. Remove or rename \
         legacy animated-image schema objects before initializing the strict \
         animated_image_quality schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM pg_class
                WHERE relkind = 'S' AND relname = 'gif_quality_inference_log_id_seq'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: gif_quality_inference_log_id_seq. Remove or \
         rename legacy animated-image schema objects before initializing the strict \
         animated_image_quality schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM pg_class
                WHERE relkind = 'i' AND relname = 'idx_gif_quality_blake3'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: idx_gif_quality_blake3. Remove or rename \
         legacy animated-image schema objects before initializing the strict \
         animated_image_quality schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM pg_class
                WHERE relkind = 'i' AND relname = 'idx_gif_quality_hnsw'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: idx_gif_quality_hnsw. Remove or rename legacy \
         animated-image schema objects before initializing the strict animated_image_quality \
         schema.';
            END IF;

            IF EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'gif_quality_samples_quality_score_check'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: gif_quality_samples_quality_score_check. \
         Remove or rename legacy animated-image schema objects before initializing the strict \
         animated_image_quality schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM pg_trigger
                WHERE tgname = 'trg_sync_gif_quality_samples_metadata'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: trg_sync_gif_quality_samples_metadata. Remove \
         or rename legacy animated-image schema objects before initializing the strict \
         animated_image_quality schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM pg_trigger
                WHERE tgname = 'trg_sync_gif_quality_samples_metadata_truncate'
            ) THEN
                RAISE EXCEPTION
                    'Legacy schema object detected: \
         trg_sync_gif_quality_samples_metadata_truncate. Remove or rename legacy animated-image \
         schema objects before initializing the strict animated_image_quality schema.';
            END IF;

            IF EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'multi_scenario_metadata'
            ) THEN
                IF EXISTS (
                    SELECT 1 FROM multi_scenario_metadata
                    WHERE scenario = 'gif_quality'
                ) THEN
                    RAISE EXCEPTION
                        'Legacy metadata row detected: scenario=gif_quality. Remove legacy \
         animated-image metadata before initializing the strict animated_image_quality schema.';
                END IF;
            END IF;
        END;
        $$;
        ",
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS loop_samples (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA UNIQUE NOT NULL,
            source_path TEXT,
            file_name TEXT,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            duration_secs DOUBLE PRECISION NOT NULL,
            frame_count BIGINT NOT NULL,
            fps DOUBLE PRECISION,
            file_size_bytes BIGINT NOT NULL DEFAULT 0,
            motion_periodicity DOUBLE PRECISION,
            temporal_jitter DOUBLE PRECISION,
            motion_gini DOUBLE PRECISION,
            loop_closure_score DOUBLE PRECISION,
            cadence_score DOUBLE PRECISION,
            embedding VECTOR(261),
            label SMALLINT DEFAULT 0,
            labeled_by TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            metadata JSONB DEFAULT '{}'::jsonb
        )",
        &[],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS image_quality_samples (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA UNIQUE NOT NULL,
            source_path TEXT,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            file_size_bytes BIGINT NOT NULL,
            format TEXT NOT NULL,
            total_pixels BIGINT,
            entropy DOUBLE PRECISION NOT NULL,
            compression_ratio DOUBLE PRECISION NOT NULL,
            spatial_bpp DOUBLE PRECISION NOT NULL,
            is_lossless BOOLEAN NOT NULL,
            embedding VECTOR(256),
            quality_label TEXT,
            quality_score REAL,
            labeled_by TEXT DEFAULT 'manual_training',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            metadata JSONB DEFAULT '{}'::jsonb
        )",
        &[],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS animated_image_quality_samples (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA UNIQUE NOT NULL,
            source_path TEXT,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            frame_count BIGINT NOT NULL,
            duration_secs DOUBLE PRECISION NOT NULL,
            fps DOUBLE PRECISION,
            palette_size INTEGER,
            palette_depth DOUBLE PRECISION,
            animation_smoothness DOUBLE PRECISION,
            frame_delay_variation DOUBLE PRECISION,
            embedding VECTOR(256),
            quality_score REAL,
            is_meme BOOLEAN DEFAULT FALSE,
            labeled_by TEXT DEFAULT 'manual_training',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            metadata JSONB DEFAULT '{}'::jsonb
        )",
        &[],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS video_quality_samples (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA UNIQUE NOT NULL,
            source_path TEXT,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            duration_secs DOUBLE PRECISION NOT NULL,
            frame_count BIGINT NOT NULL,
            fps DOUBLE PRECISION,
            file_size_bytes BIGINT NOT NULL,
            codec TEXT NOT NULL,
            bitrate_mbps REAL,
            bit_depth SMALLINT,
            has_audio BOOLEAN NOT NULL DEFAULT FALSE,
            is_variable_frame_rate BOOLEAN NOT NULL DEFAULT FALSE,
            is_hdr BOOLEAN NOT NULL DEFAULT FALSE,
            motion_intensity REAL,
            temporal_stability REAL,
            embedding VECTOR(256),
            quality_score REAL,
            labeled_by TEXT DEFAULT 'manual_training',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            metadata JSONB DEFAULT '{}'::jsonb
        )",
        &[],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS loop_intent_inference_log (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA,
            source_path TEXT,
            knn_score DOUBLE PRECISION,
            knn_confidence DOUBLE PRECISION,
            knn_neighbor_count INTEGER,
            final_verdict TEXT NOT NULL DEFAULT 'unknown',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS image_quality_inference_log (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA,
            source_path TEXT,
            knn_score DOUBLE PRECISION,
            knn_confidence DOUBLE PRECISION,
            knn_neighbor_count INTEGER,
            bpp_fallback_score DOUBLE PRECISION,
            heuristic_score DOUBLE PRECISION,
            regression_score DOUBLE PRECISION,
            predictor_family TEXT NOT NULL DEFAULT 'unknown',
            final_verdict TEXT NOT NULL DEFAULT 'low',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS animated_image_quality_inference_log (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA,
            source_path TEXT,
            knn_score DOUBLE PRECISION,
            knn_confidence DOUBLE PRECISION,
            knn_neighbor_count INTEGER,
            final_verdict TEXT NOT NULL DEFAULT 'unknown',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS video_quality_inference_log (
            id BIGSERIAL PRIMARY KEY,
            blake3 BYTEA,
            source_path TEXT,
            knn_score DOUBLE PRECISION,
            knn_confidence DOUBLE PRECISION,
            knn_neighbor_count INTEGER,
            final_verdict TEXT NOT NULL DEFAULT 'unknown',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS multi_scenario_metadata (
            scenario TEXT PRIMARY KEY,
            table_name TEXT NOT NULL,
            embedding_dimension INTEGER NOT NULL,
            sample_count BIGINT DEFAULT 0,
            last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            feature_stats JSONB DEFAULT '{}'::jsonb,
            collection_stats JSONB DEFAULT '{}'::jsonb
        )",
        &[],
    )?;

    // Column-level migrations keep databases created by earlier snapshots usable.
    conn.execute(
        "ALTER TABLE image_quality_samples ADD COLUMN IF NOT EXISTS quality_score REAL",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS file_size_bytes BIGINT NOT NULL \
         DEFAULT 0",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS file_name TEXT",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS fps DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS motion_periodicity DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS temporal_jitter DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS motion_gini DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS loop_closure_score DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS cadence_score DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS labeled_by TEXT",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE loop_samples ADD COLUMN IF NOT EXISTS metadata JSONB DEFAULT '{}'::jsonb",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE multi_scenario_metadata ADD COLUMN IF NOT EXISTS feature_stats JSONB DEFAULT \
         '{}'::jsonb",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE multi_scenario_metadata ADD COLUMN IF NOT EXISTS collection_stats JSONB \
         DEFAULT '{}'::jsonb",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE image_quality_samples ADD COLUMN IF NOT EXISTS metadata JSONB DEFAULT \
         '{}'::jsonb",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE image_quality_inference_log ADD COLUMN IF NOT EXISTS bpp_fallback_score \
         DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE image_quality_inference_log ADD COLUMN IF NOT EXISTS heuristic_score DOUBLE \
         PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE image_quality_inference_log ADD COLUMN IF NOT EXISTS regression_score DOUBLE \
         PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE image_quality_inference_log ADD COLUMN IF NOT EXISTS predictor_family TEXT \
         NOT NULL DEFAULT 'unknown'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE animated_image_quality_inference_log ADD COLUMN IF NOT EXISTS \
         predictor_family TEXT NOT NULL DEFAULT 'unknown'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_inference_log ADD COLUMN IF NOT EXISTS predictor_family TEXT \
         NOT NULL DEFAULT 'unknown'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE image_quality_inference_log ADD COLUMN IF NOT EXISTS resolution_branch TEXT \
         NOT NULL DEFAULT 'unknown'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE animated_image_quality_inference_log ADD COLUMN IF NOT EXISTS \
         resolution_branch TEXT NOT NULL DEFAULT 'unknown'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_inference_log ADD COLUMN IF NOT EXISTS resolution_branch TEXT \
         NOT NULL DEFAULT 'unknown'",
        &[],
    )?;
    for table in [
        "image_quality_inference_log",
        "animated_image_quality_inference_log",
        "video_quality_inference_log",
    ] {
        conn.execute(
            &format!(
                "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS inference_snapshot JSONB DEFAULT \
                 '{{}}'::jsonb"
            ),
            &[],
        )?;
    }
    ensure_loop_inference_log_schema(conn)?;
    conn.execute(
        "ALTER TABLE animated_image_quality_samples ADD COLUMN IF NOT EXISTS quality_score REAL",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE animated_image_quality_samples ADD COLUMN IF NOT EXISTS metadata JSONB \
         DEFAULT '{}'::jsonb",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS quality_score REAL",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS frame_count BIGINT NOT NULL \
         DEFAULT 0",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS file_size_bytes BIGINT NOT \
         NULL DEFAULT 0",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS codec TEXT NOT NULL DEFAULT \
         'unknown'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS bit_depth SMALLINT",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS has_audio BOOLEAN NOT NULL \
         DEFAULT FALSE",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS is_variable_frame_rate \
         BOOLEAN NOT NULL DEFAULT FALSE",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS is_hdr BOOLEAN NOT NULL \
         DEFAULT FALSE",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE video_quality_samples ADD COLUMN IF NOT EXISTS metadata JSONB DEFAULT \
         '{}'::jsonb",
        &[],
    )?;

    ensure_embedding_column_dimension(
        conn,
        "loop_samples",
        i32::try_from(ScenarioType::LoopIntent.embedding_dimension())
            .context("LoopIntent embedding dimension does not fit i32")?,
    )?;
    ensure_embedding_column_dimension(
        conn,
        "image_quality_samples",
        i32::try_from(ScenarioType::ImageQuality.embedding_dimension())
            .context("ImageQuality embedding dimension does not fit i32")?,
    )?;
    ensure_embedding_column_dimension(
        conn,
        "animated_image_quality_samples",
        i32::try_from(ScenarioType::AnimatedImageQuality.embedding_dimension())
            .context("AnimatedImageQuality embedding dimension does not fit i32")?,
    )?;
    ensure_embedding_column_dimension(
        conn,
        "video_quality_samples",
        i32::try_from(ScenarioType::VideoQuality.embedding_dimension())
            .context("VideoQuality embedding dimension does not fit i32")?,
    )?;
    conn.execute(
        "UPDATE image_quality_samples
         SET quality_label = CASE
            WHEN quality_label = 'high' AND LOWER(COALESCE(format, '')) = 'png' THEN 'png-high'
            WHEN quality_label = 'high' THEN 'modern-high'
            WHEN quality_label = 'low' AND LOWER(COALESCE(format, '')) = 'png' THEN 'png-low'
            WHEN quality_label = 'low' THEN 'modern-low'
            ELSE quality_label
         END
         WHERE quality_label IN ('high', 'low')",
        &[],
    )?;
    conn.execute(
        "UPDATE image_quality_samples
         SET quality_score = CASE
            WHEN quality_label IN ('png-high', 'modern-high') THEN 1.0
            WHEN quality_label IN ('png-low', 'modern-low') THEN 0.0
         END
         WHERE quality_label IN ('png-high', 'modern-high', 'png-low', 'modern-low')
           AND quality_score IS DISTINCT FROM CASE
               WHEN quality_label IN ('png-high', 'modern-high') THEN 1.0
               WHEN quality_label IN ('png-low', 'modern-low') THEN 0.0
               ELSE quality_score
           END",
        &[],
    )?;
    conn.batch_execute(
        "
        CREATE OR REPLACE FUNCTION normalize_image_quality_score()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            normalized_label TEXT;
            expected_score REAL;
            is_png_family BOOLEAN;
        BEGIN
            IF TG_OP = 'UPDATE' AND OLD.quality_label IS NOT NULL AND NEW.quality_label IS NOT \
         NULL THEN
                IF LOWER(BTRIM(OLD.quality_label)) <> LOWER(BTRIM(NEW.quality_label)) THEN
                    RAISE EXCEPTION
                        'image_quality_samples.quality_label is immutable once set (old=%, new=%)',
                        OLD.quality_label, NEW.quality_label;
                END IF;
            END IF;

            IF NEW.quality_label IS NULL THEN
                RAISE EXCEPTION 'image_quality_samples.quality_label must not be NULL';
            END IF;

            normalized_label := LOWER(BTRIM(NEW.quality_label));
            is_png_family := LOWER(COALESCE(NEW.format, '')) = 'png';

            IF normalized_label = 'high' THEN
                normalized_label := CASE WHEN is_png_family THEN 'png-high' ELSE 'modern-high' END;
            ELSIF normalized_label = 'low' THEN
                normalized_label := CASE WHEN is_png_family THEN 'png-low' ELSE 'modern-low' END;
            ELSIF normalized_label IN ('png-high', 'png-low') THEN
                IF NOT is_png_family THEN
                    RAISE EXCEPTION
                        'PNG quality labels require PNG sources (label=%, format=%)',
                        NEW.quality_label, NEW.format;
                END IF;
            ELSIF normalized_label IN ('modern-high', 'modern-low') THEN
                IF is_png_family THEN
                    RAISE EXCEPTION
                        'Modern quality labels are incompatible with PNG sources (label=%, \
         format=%)',
                        NEW.quality_label, NEW.format;
                END IF;
            ELSE
                RAISE EXCEPTION
                    'Unsupported image quality label: %',
                    NEW.quality_label;
            END IF;

            expected_score := CASE
                WHEN normalized_label IN ('png-high', 'modern-high') THEN 1.0
                WHEN normalized_label IN ('png-low', 'modern-low') THEN 0.0
                ELSE NULL
            END;

            NEW.quality_label := normalized_label;
            IF NEW.quality_score IS NULL THEN
                NEW.quality_score := expected_score;
            ELSIF NEW.quality_score <> expected_score THEN
                RAISE EXCEPTION
                    'quality_score % does not match quality_label % (expected %)',
                    NEW.quality_score, NEW.quality_label, expected_score;
            END IF;

            RETURN NEW;
        END;
        $$;

        DROP TRIGGER IF EXISTS trg_normalize_image_quality_score ON image_quality_samples;
        CREATE TRIGGER trg_normalize_image_quality_score
        BEFORE INSERT OR UPDATE ON image_quality_samples
        FOR EACH ROW
        EXECUTE FUNCTION normalize_image_quality_score();

        CREATE OR REPLACE FUNCTION enforce_loop_label_immutable()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF TG_OP = 'UPDATE' THEN
                IF OLD.label IS DISTINCT FROM NEW.label THEN
                    RAISE EXCEPTION
                        'loop_samples.label is immutable once set (old=%, new=%)',
                        OLD.label, NEW.label;
                END IF;
            END IF;
            RETURN NEW;
        END;
        $$;

        DROP TRIGGER IF EXISTS trg_enforce_loop_label_immutable ON loop_samples;
        CREATE TRIGGER trg_enforce_loop_label_immutable
        BEFORE UPDATE ON loop_samples
        FOR EACH ROW
        EXECUTE FUNCTION enforce_loop_label_immutable();

        CREATE OR REPLACE FUNCTION enforce_animated_image_quality_score_immutable()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF TG_OP = 'UPDATE' AND OLD.quality_score IS NOT NULL AND NEW.quality_score IS NOT \
         NULL THEN
                IF OLD.quality_score IS DISTINCT FROM NEW.quality_score THEN
                    RAISE EXCEPTION
                        'animated_image_quality_samples.quality_score is immutable once set \
         (old=%, new=%)',
                        OLD.quality_score, NEW.quality_score;
                END IF;
            END IF;
            RETURN NEW;
        END;
        $$;

        DROP TRIGGER IF EXISTS trg_enforce_animated_image_quality_score_immutable ON \
         animated_image_quality_samples;
        CREATE TRIGGER trg_enforce_animated_image_quality_score_immutable
        BEFORE UPDATE ON animated_image_quality_samples
        FOR EACH ROW
        EXECUTE FUNCTION enforce_animated_image_quality_score_immutable();

        CREATE OR REPLACE FUNCTION enforce_video_quality_score_immutable()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF TG_OP = 'UPDATE' AND OLD.quality_score IS NOT NULL AND NEW.quality_score IS NOT \
         NULL THEN
                IF OLD.quality_score IS DISTINCT FROM NEW.quality_score THEN
                    RAISE EXCEPTION
                        'video_quality_samples.quality_score is immutable once set (old=%, new=%)',
                        OLD.quality_score, NEW.quality_score;
                END IF;
            END IF;
            RETURN NEW;
        END;
        $$;

        DROP TRIGGER IF EXISTS trg_enforce_video_quality_score_immutable ON video_quality_samples;
        CREATE TRIGGER trg_enforce_video_quality_score_immutable
        BEFORE UPDATE ON video_quality_samples
        FOR EACH ROW
        EXECUTE FUNCTION enforce_video_quality_score_immutable();

        CREATE OR REPLACE FUNCTION sync_multi_scenario_metadata_sample_count()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            PERFORM pg_advisory_xact_lock(5568210966476441669);

            IF TG_OP = 'INSERT' THEN
                UPDATE multi_scenario_metadata
                SET sample_count = sample_count + 1,
                    last_updated = CURRENT_TIMESTAMP
                WHERE table_name = TG_TABLE_NAME;
                RETURN NEW;
            ELSIF TG_OP = 'DELETE' THEN
                UPDATE multi_scenario_metadata
                SET sample_count = GREATEST(sample_count - 1, 0),
                    last_updated = CURRENT_TIMESTAMP
                WHERE table_name = TG_TABLE_NAME;
                RETURN OLD;
            END IF;

            UPDATE multi_scenario_metadata
            SET last_updated = CURRENT_TIMESTAMP
            WHERE table_name = TG_TABLE_NAME;
            RETURN NEW;
        END;
        $$;

        CREATE OR REPLACE FUNCTION sync_multi_scenario_metadata_on_truncate()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            PERFORM pg_advisory_xact_lock(5568210966476441669);

            UPDATE multi_scenario_metadata
            SET sample_count = 0,
                last_updated = CURRENT_TIMESTAMP
            WHERE table_name = TG_TABLE_NAME;
            RETURN NULL;
        END;
        $$;

        DROP TRIGGER IF EXISTS trg_sync_loop_samples_metadata ON loop_samples;
        CREATE TRIGGER trg_sync_loop_samples_metadata
        AFTER INSERT OR UPDATE OR DELETE ON loop_samples
        FOR EACH ROW
        EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count();

        DROP TRIGGER IF EXISTS trg_sync_image_quality_samples_metadata ON image_quality_samples;
        CREATE TRIGGER trg_sync_image_quality_samples_metadata
        AFTER INSERT OR UPDATE OR DELETE ON image_quality_samples
        FOR EACH ROW
        EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count();

        DROP TRIGGER IF EXISTS trg_sync_animated_image_quality_samples_metadata ON \
         animated_image_quality_samples;
        CREATE TRIGGER trg_sync_animated_image_quality_samples_metadata
        AFTER INSERT OR UPDATE OR DELETE ON animated_image_quality_samples
        FOR EACH ROW
        EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count();

        DROP TRIGGER IF EXISTS trg_sync_video_quality_samples_metadata ON video_quality_samples;
        CREATE TRIGGER trg_sync_video_quality_samples_metadata
        AFTER INSERT OR UPDATE OR DELETE ON video_quality_samples
        FOR EACH ROW
        EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count();

        DROP TRIGGER IF EXISTS trg_sync_loop_samples_metadata_truncate ON loop_samples;
        CREATE TRIGGER trg_sync_loop_samples_metadata_truncate
        AFTER TRUNCATE ON loop_samples
        FOR EACH STATEMENT
        EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate();

        DROP TRIGGER IF EXISTS trg_sync_image_quality_samples_metadata_truncate ON \
         image_quality_samples;
        CREATE TRIGGER trg_sync_image_quality_samples_metadata_truncate
        AFTER TRUNCATE ON image_quality_samples
        FOR EACH STATEMENT
        EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate();

        DROP TRIGGER IF EXISTS trg_sync_animated_image_quality_samples_metadata_truncate ON \
         animated_image_quality_samples;
        CREATE TRIGGER trg_sync_animated_image_quality_samples_metadata_truncate
        AFTER TRUNCATE ON animated_image_quality_samples
        FOR EACH STATEMENT
        EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate();

        DROP TRIGGER IF EXISTS trg_sync_video_quality_samples_metadata_truncate ON \
         video_quality_samples;
        CREATE TRIGGER trg_sync_video_quality_samples_metadata_truncate
        AFTER TRUNCATE ON video_quality_samples
        FOR EACH STATEMENT
        EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate();
        ",
    )?;
    conn.batch_execute(
        "
        DO $$
        BEGIN
            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'image_quality_samples_quality_score_check'
                  AND conrelid = 'image_quality_samples'::regclass
            ) THEN
                ALTER TABLE image_quality_samples
                ADD CONSTRAINT image_quality_samples_quality_score_check
                CHECK (
                    quality_score IS NOT NULL
                    AND quality_score = quality_score
                    AND quality_score >= 0.0
                    AND quality_score <= 1.0
                ) NOT VALID;
            END IF;

            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'animated_image_quality_samples_quality_score_check'
                  AND conrelid = 'animated_image_quality_samples'::regclass
            ) THEN
                ALTER TABLE animated_image_quality_samples
                ADD CONSTRAINT animated_image_quality_samples_quality_score_check
                CHECK (
                    quality_score IS NOT NULL
                    AND quality_score = quality_score
                    AND quality_score >= 0.0
                    AND quality_score <= 1.0
                ) NOT VALID;
            END IF;

            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'video_quality_samples_quality_score_check'
                  AND conrelid = 'video_quality_samples'::regclass
            ) THEN
                ALTER TABLE video_quality_samples
                ADD CONSTRAINT video_quality_samples_quality_score_check
                CHECK (
                    quality_score IS NOT NULL
                    AND quality_score = quality_score
                    AND quality_score >= 0.0
                    AND quality_score <= 1.0
                ) NOT VALID;
            END IF;

            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'loop_samples_media_metadata_check'
                  AND conrelid = 'loop_samples'::regclass
            ) THEN
                ALTER TABLE loop_samples
                ADD CONSTRAINT loop_samples_media_metadata_check
                CHECK (
                    width > 0
                    AND height > 0
                    AND duration_secs = duration_secs
                    AND duration_secs > 0.0
                    AND duration_secs < 'Infinity'::double precision
                    AND frame_count > 0
                    AND file_size_bytes > 0
                    AND (fps IS NULL OR (
                        fps = fps
                        AND fps > 0.0
                        AND fps < 'Infinity'::double precision
                    ))
                ) NOT VALID;
            END IF;

            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'image_quality_samples_media_metadata_check'
                  AND conrelid = 'image_quality_samples'::regclass
            ) THEN
                ALTER TABLE image_quality_samples
                ADD CONSTRAINT image_quality_samples_media_metadata_check
                CHECK (
                    width > 0
                    AND height > 0
                    AND file_size_bytes > 0
                    AND LENGTH(BTRIM(format)) > 0
                    AND (total_pixels IS NULL OR total_pixels > 0)
                    AND entropy = entropy
                    AND entropy >= 0.0
                    AND entropy < 'Infinity'::double precision
                    AND compression_ratio = compression_ratio
                    AND compression_ratio > 0.0
                    AND compression_ratio < 'Infinity'::double precision
                    AND spatial_bpp = spatial_bpp
                    AND spatial_bpp > 0.0
                    AND spatial_bpp < 'Infinity'::double precision
                ) NOT VALID;
            END IF;

            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'animated_image_quality_samples_media_metadata_check'
                  AND conrelid = 'animated_image_quality_samples'::regclass
            ) THEN
                ALTER TABLE animated_image_quality_samples
                ADD CONSTRAINT animated_image_quality_samples_media_metadata_check
                CHECK (
                    width > 0
                    AND height > 0
                    AND frame_count > 1
                    AND duration_secs = duration_secs
                    AND duration_secs > 0.0
                    AND duration_secs < 'Infinity'::double precision
                    AND (fps IS NULL OR (
                        fps = fps
                        AND fps > 0.0
                        AND fps < 'Infinity'::double precision
                    ))
                ) NOT VALID;
            END IF;

            IF NOT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = 'video_quality_samples_media_metadata_check'
                  AND conrelid = 'video_quality_samples'::regclass
            ) THEN
                ALTER TABLE video_quality_samples
                ADD CONSTRAINT video_quality_samples_media_metadata_check
                CHECK (
                    width > 0
                    AND height > 0
                    AND duration_secs = duration_secs
                    AND duration_secs > 0.0
                    AND duration_secs < 'Infinity'::double precision
                    AND frame_count > 0
                    AND file_size_bytes > 0
                    AND LENGTH(BTRIM(codec)) > 0
                    AND LOWER(BTRIM(codec)) <> 'unknown'
                    AND (fps IS NULL OR (
                        fps = fps
                        AND fps > 0.0
                        AND fps < 'Infinity'::double precision
                    ))
                ) NOT VALID;
            END IF;
        END;
        $$;

        ALTER TABLE image_quality_samples
        VALIDATE CONSTRAINT image_quality_samples_quality_score_check;
        ALTER TABLE animated_image_quality_samples
        VALIDATE CONSTRAINT animated_image_quality_samples_quality_score_check;
        ALTER TABLE video_quality_samples
        VALIDATE CONSTRAINT video_quality_samples_quality_score_check;
        ALTER TABLE loop_samples
        VALIDATE CONSTRAINT loop_samples_media_metadata_check;
        ALTER TABLE image_quality_samples
        VALIDATE CONSTRAINT image_quality_samples_media_metadata_check;
        ALTER TABLE animated_image_quality_samples
        VALIDATE CONSTRAINT animated_image_quality_samples_media_metadata_check;
        ALTER TABLE video_quality_samples
        VALIDATE CONSTRAINT video_quality_samples_media_metadata_check;
        ",
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_loop_samples_blake3 ON loop_samples(blake3)",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_loop_samples_hnsw ON loop_samples USING hnsw (embedding \
         vector_l2_ops)",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_image_quality_blake3 ON image_quality_samples(blake3)",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_image_quality_hnsw ON image_quality_samples USING hnsw \
         (embedding vector_l2_ops)",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_animated_image_quality_blake3 ON \
         animated_image_quality_samples(blake3)",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_animated_image_quality_hnsw ON \
         animated_image_quality_samples USING hnsw (embedding vector_l2_ops)",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_video_quality_blake3 ON video_quality_samples(blake3)",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_video_quality_hnsw ON video_quality_samples USING hnsw \
         (embedding vector_l2_ops)",
        &[],
    )?;

    for scenario in ScenarioType::all() {
        let scenario_name = scenario.to_string();
        let table_name = scenario.table_name();
        let row = conn.query_one(&format!("SELECT COUNT(*) FROM {table_name}"), &[])?;
        let count: i64 = row.get(0);
        let dimension = i32::try_from(scenario.embedding_dimension())
            .context("Embedding dimension does not fit into PostgreSQL INT")?;

        conn.execute(
            "INSERT INTO multi_scenario_metadata (
                scenario, table_name, embedding_dimension, sample_count, last_updated
            ) VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
            ON CONFLICT (scenario) DO UPDATE SET
                table_name = EXCLUDED.table_name,
                embedding_dimension = EXCLUDED.embedding_dimension,
                sample_count = EXCLUDED.sample_count,
                last_updated = CURRENT_TIMESTAMP",
            &[&scenario_name, &table_name, &dimension, &count],
        )?;
    }

    ensure_inference_runtime_verdict_views(conn)?;
    init_all_scenarios(conn)?;

    Ok(())
}

/// Analytics views: expose `runtime_final_verdict` when audit-only columns
/// store `TelemetryOnly`.
fn ensure_inference_runtime_verdict_views(conn: &mut Client) -> Result<()> {
    conn.batch_execute(
        "
        CREATE OR REPLACE VIEW loop_inference_log_effective AS
        SELECT
            id,
            CASE
                WHEN file_hash ~ '^[0-9A-Fa-f]{64}$' THEN decode(file_hash, 'hex')
                ELSE NULL
            END AS blake3,
            file_hash AS legacy_file_hash,
            source_path,
            COALESCE(signal_snapshot->>'runtime_final_verdict', final_verdict) AS \
         effective_final_verdict,
            COALESCE(signal_snapshot->>'runtime_decision_reason', decision_reason) AS \
         effective_decision_reason,
            COALESCE(
                NULLIF(signal_snapshot->>'runtime_final_probability', '')::double precision,
                final_probability
            ) AS effective_final_probability,
            final_verdict AS stored_final_verdict,
            (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
            (
                resolution_path = 'layer7_fallback'
                OR (signal_snapshot->>'layer7_upstream') IS NOT NULL
            ) AS is_layer7_policy_exit,
            (
                tree_probability IS NOT NULL
                AND NOT (
                    resolution_path = 'layer7_fallback'
                    OR (signal_snapshot->>'layer7_upstream') IS NOT NULL
                )
            ) AS tree_probability_is_authoritative,
            resolution_path,
            layer_exit,
            tree_probability,
            final_probability,
            created_at
        FROM inference_log;

        CREATE OR REPLACE VIEW image_quality_inference_log_effective AS
        SELECT
            id,
            source_path,
            COALESCE(inference_snapshot->>'runtime_final_verdict', final_verdict) AS \
         effective_final_verdict,
            COALESCE(inference_snapshot->>'runtime_resolution_branch', resolution_branch) AS \
         effective_resolution_branch,
            final_verdict AS stored_final_verdict,
            (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
            resolution_branch,
            predictor_family,
            created_at
        FROM image_quality_inference_log;

        CREATE OR REPLACE VIEW animated_image_quality_inference_log_effective AS
        SELECT
            id,
            source_path,
            COALESCE(inference_snapshot->>'runtime_final_verdict', final_verdict) AS \
         effective_final_verdict,
            COALESCE(inference_snapshot->>'runtime_resolution_branch', resolution_branch) AS \
         effective_resolution_branch,
            final_verdict AS stored_final_verdict,
            (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
            resolution_branch,
            predictor_family,
            created_at
        FROM animated_image_quality_inference_log;

        CREATE OR REPLACE VIEW video_quality_inference_log_effective AS
        SELECT
            id,
            source_path,
            COALESCE(inference_snapshot->>'runtime_final_verdict', final_verdict) AS \
         effective_final_verdict,
            COALESCE(inference_snapshot->>'runtime_resolution_branch', resolution_branch) AS \
         effective_resolution_branch,
            final_verdict AS stored_final_verdict,
            (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
            resolution_branch,
            predictor_family,
            created_at
        FROM video_quality_inference_log;
        ",
    )?;
    Ok(())
}

fn blake3_hex(b: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(b.len().saturating_mul(2));
    for byte in b {
        let _ = write!(&mut s, "{byte:02x}");
    }
    s
}

/// Explicit pre-check so CLI/C-API callers surface `LABEL_CONFLICT:` without
/// relying only on DB triggers.
fn ensure_no_image_quality_label_conflict(
    conn: &mut Client,
    blake3: &[u8],
    incoming_score: f32,
) -> Result<()> {
    let row = conn.query_opt(
        "SELECT quality_score FROM image_quality_samples WHERE blake3 = $1",
        &[&blake3],
    )?;
    if let Some(row) = row {
        let existing: f32 = row.get(0);
        if (existing - incoming_score).abs() > 1e-4 {
            anyhow::bail!(
                "LABEL_CONFLICT: image_quality blake3={} existing_score={} incoming_score={}",
                blake3_hex(blake3),
                existing,
                incoming_score
            );
        }
    }
    Ok(())
}

fn ensure_no_animated_quality_score_conflict(
    conn: &mut Client,
    blake3: &[u8],
    incoming: f32,
) -> Result<()> {
    let row = conn.query_opt(
        "SELECT quality_score FROM animated_image_quality_samples WHERE blake3 = $1",
        &[&blake3],
    )?;
    if let Some(row) = row {
        let existing: f32 = row.get(0);
        if (existing - incoming).abs() > 1e-4 {
            anyhow::bail!(
                "LABEL_CONFLICT: animated_image_quality blake3={} existing_quality_score={} \
                 incoming_quality_score={}",
                blake3_hex(blake3),
                existing,
                incoming
            );
        }
    }
    Ok(())
}

fn ensure_no_video_quality_score_conflict(
    conn: &mut Client,
    blake3: &[u8],
    incoming: f32,
) -> Result<()> {
    let row = conn.query_opt(
        "SELECT quality_score FROM video_quality_samples WHERE blake3 = $1",
        &[&blake3],
    )?;
    if let Some(row) = row {
        let existing: f32 = row.get(0);
        if (existing - incoming).abs() > 1e-4 {
            anyhow::bail!(
                "LABEL_CONFLICT: video_quality blake3={} existing_quality_score={} \
                 incoming_quality_score={}",
                blake3_hex(blake3),
                existing,
                incoming
            );
        }
    }
    Ok(())
}

fn ensure_no_loop_label_conflict(conn: &mut Client, blake3: &[u8], incoming: i16) -> Result<()> {
    let row = conn.query_opt(
        "SELECT label FROM loop_samples WHERE blake3 = $1",
        &[&blake3],
    )?;
    if let Some(row) = row {
        let existing: i16 = row.get(0);
        if existing != incoming {
            anyhow::bail!(
                "LABEL_CONFLICT: loop_intent blake3={} existing_label={} incoming_label={}",
                blake3_hex(blake3),
                existing,
                incoming
            );
        }
    }
    Ok(())
}

/// Ingest into `image_quality_samples` (`256D`).
///
/// # Errors
/// Returns an error if the sample is missing required embedding, entropy, or
/// compression metadata, or if the insert fails.
pub fn ingest_image_quality_sample(conn: &mut Client, sample: &ScenarioSample) -> Result<()> {
    validate_embedding(sample)?;
    let quality_score = require_quality_score(sample)?;
    let canonical_label = resolve_image_quality_label(sample, quality_score)?;
    let canonical_label_str = canonical_label.as_str();
    ensure_no_image_quality_label_conflict(conn, &sample.blake3, canonical_label.to_score())?;

    let exists: bool = conn
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM image_quality_samples WHERE blake3 = $1)",
            &[&sample.blake3],
        )?
        .get(0);
    if !exists {
        let (high, low) = crate::image_quality_db::get_class_counts(conn);
        let is_high = canonical_label_str.contains("high");
        if is_high && high >= crate::constants::STATIC_QUALITY_DB_CAP_PER_CLASS {
            anyhow::bail!(
                "INGESTION REJECTED: Database class 'high' has reached the maximum cap of {}.",
                crate::constants::STATIC_QUALITY_DB_CAP_PER_CLASS
            );
        }
        if !is_high && low >= crate::constants::STATIC_QUALITY_DB_CAP_PER_CLASS {
            anyhow::bail!(
                "INGESTION REJECTED: Database class 'low' has reached the maximum cap of {}.",
                crate::constants::STATIC_QUALITY_DB_CAP_PER_CLASS
            );
        }
    }

    let entropy = require_finite_metric(sample.entropy, "entropy", sample)?;
    let comp_ratio = require_finite_metric(sample.compression_ratio, "compression_ratio", sample)?;
    let is_lossless = sample
        .is_lossless
        .ok_or_else(|| anyhow::anyhow!("Lossless status required"))?;

    if sample.width <= 0 || sample.height <= 0 || sample.file_size_bytes <= 0 {
        anyhow::bail!("Invalid dimensions/size for image quality ingestion");
    }

    let total_pixels = i64::from(sample.width) * i64::from(sample.height);
    let spatial_bpp = crate::numeric_cast::i64_to_f64(sample.file_size_bytes)
        / crate::numeric_cast::i64_to_f64(total_pixels);
    if !spatial_bpp.is_finite() || spatial_bpp < 0.0 {
        anyhow::bail!("spatial_bpp is not finite for image quality ingestion");
    }

    conn.execute(
        "INSERT INTO image_quality_samples (
            blake3, source_path, width, height, file_size_bytes, format, total_pixels,
            entropy, compression_ratio, spatial_bpp, is_lossless, embedding, quality_label, \
         quality_score, metadata
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        ON CONFLICT (blake3) DO UPDATE SET
            source_path = EXCLUDED.source_path,
            width = EXCLUDED.width,
            height = EXCLUDED.height,
            file_size_bytes = EXCLUDED.file_size_bytes,
            format = EXCLUDED.format,
            total_pixels = EXCLUDED.total_pixels,
            entropy = EXCLUDED.entropy,
            compression_ratio = EXCLUDED.compression_ratio,
            spatial_bpp = EXCLUDED.spatial_bpp,
            is_lossless = EXCLUDED.is_lossless,
            embedding = EXCLUDED.embedding,
            quality_label = EXCLUDED.quality_label,
            quality_score = EXCLUDED.quality_score,
            metadata = EXCLUDED.metadata",
        &[
            &sample.blake3,
            &sample.source_path,
            &sample.width,
            &sample.height,
            &sample.file_size_bytes,
            &sample.format,
            &total_pixels,
            &entropy,
            &comp_ratio,
            &spatial_bpp,
            &is_lossless,
            &sample.embedding,
            &canonical_label_str,
            &canonical_label.to_score(),
            &sample.metadata,
        ],
    )
    .context("Failed to ingest image quality sample")?;
    Ok(())
}

/// Ingest into `animated_image_quality_samples` for `animated_image_quality`
/// (`256D`).
///
/// # Errors
/// Returns an error if the sample is missing required embedding, geometry, or
/// temporal fields, or if the insert fails.
pub fn ingest_animated_image_quality_sample(
    conn: &mut Client,
    sample: &ScenarioSample,
) -> Result<()> {
    validate_embedding(sample)?;
    let quality_score = require_quality_score(sample)?;
    let duration_secs = require_finite_real(sample.duration_secs, "duration_secs", sample)?;
    let fps = normalize_optional_positive_real(sample.fps, "fps", sample)?;
    let palette_size =
        normalize_optional_non_negative_i32(sample.palette_size, "palette_size", sample)?;
    let palette_depth =
        normalize_optional_positive_real(sample.palette_depth, "palette_depth", sample)?;
    let animation_smoothness = normalize_optional_finite_real(
        sample.animation_smoothness,
        "animation_smoothness",
        sample,
    )?;
    let frame_delay_variation = normalize_optional_finite_real(
        sample.frame_delay_variation,
        "frame_delay_variation",
        sample,
    )?;

    // 🛡️ DIMENSION & TEMPORAL GUARD: Prevent zero-value pollution
    if sample.width <= 0 || sample.height <= 0 || sample.frame_count <= 0 || duration_secs <= 0.0 {
        anyhow::bail!("Invalid dimensions or temporal metadata for animated-image ingestion");
    }

    ensure_no_animated_quality_score_conflict(conn, &sample.blake3, quality_score)?;

    conn.execute(
        "INSERT INTO animated_image_quality_samples (
            blake3, source_path, width, height, frame_count, duration_secs, fps,
            palette_size, palette_depth, animation_smoothness, frame_delay_variation,
            embedding, quality_score, is_meme, metadata
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (blake3) DO UPDATE SET
            source_path = EXCLUDED.source_path,
            width = EXCLUDED.width,
            height = EXCLUDED.height,
            frame_count = EXCLUDED.frame_count,
            duration_secs = EXCLUDED.duration_secs,
            fps = EXCLUDED.fps,
            palette_size = EXCLUDED.palette_size,
            palette_depth = EXCLUDED.palette_depth,
            animation_smoothness = EXCLUDED.animation_smoothness,
            frame_delay_variation = EXCLUDED.frame_delay_variation,
            embedding = EXCLUDED.embedding,
            quality_score = EXCLUDED.quality_score,
            is_meme = EXCLUDED.is_meme,
            metadata = EXCLUDED.metadata",
        &[
            &sample.blake3,
            &sample.source_path,
            &sample.width,
            &sample.height,
            &sample.frame_count,
            &duration_secs,
            &fps,
            &palette_size,
            &palette_depth,
            &animation_smoothness,
            &frame_delay_variation,
            &sample.embedding,
            &quality_score,
            &sample.is_meme,
            &sample.metadata,
        ],
    )
    .context("Failed to ingest animated-image quality sample")?;
    Ok(())
}

/// Ingest into `video_quality_samples` (`256D`).
///
/// # Errors
/// Returns an error if the sample is missing required embedding, geometry, or
/// bitrate fields, or if the insert fails.
pub fn ingest_video_quality_sample(conn: &mut Client, sample: &ScenarioSample) -> Result<()> {
    validate_embedding(sample)?;
    let quality_score = require_quality_score(sample)?;
    let duration_secs = require_finite_real(sample.duration_secs, "duration_secs", sample)?;
    let fps = normalize_optional_positive_real(sample.fps, "fps", sample)?;
    let bitrate_mbps =
        normalize_optional_positive_real(sample.bitrate_mbps, "bitrate_mbps", sample)?;
    let bit_depth = normalize_optional_positive_i16(sample.bit_depth, "bit_depth", sample)?;
    let motion_intensity =
        normalize_optional_finite_real(sample.motion_intensity, "motion_intensity", sample)?;
    let temporal_stability =
        normalize_optional_finite_real(sample.temporal_stability, "temporal_stability", sample)?;

    // 🛡️ DIMENSION & TEMPORAL GUARD: Prevent zero-value pollution
    if sample.width <= 0
        || sample.height <= 0
        || sample.frame_count <= 0
        || sample.file_size_bytes <= 0
        || duration_secs <= 0.0
        || sample.format.trim().is_empty()
    {
        anyhow::bail!("Invalid dimensions or media metadata for video ingestion");
    }

    ensure_no_video_quality_score_conflict(conn, &sample.blake3, quality_score)?;

    conn.execute(
        "INSERT INTO video_quality_samples (
            blake3, source_path, width, height, duration_secs, frame_count, fps,
            file_size_bytes, codec, bitrate_mbps, bit_depth, has_audio,
            is_variable_frame_rate, is_hdr, motion_intensity, temporal_stability,
            embedding, quality_score, metadata
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18, $19
        )
         ON CONFLICT (blake3) DO UPDATE SET
            source_path = EXCLUDED.source_path,
            width = EXCLUDED.width,
            height = EXCLUDED.height,
            duration_secs = EXCLUDED.duration_secs,
            frame_count = EXCLUDED.frame_count,
            fps = EXCLUDED.fps,
            file_size_bytes = EXCLUDED.file_size_bytes,
            codec = EXCLUDED.codec,
            bitrate_mbps = EXCLUDED.bitrate_mbps,
            bit_depth = EXCLUDED.bit_depth,
            has_audio = EXCLUDED.has_audio,
            is_variable_frame_rate = EXCLUDED.is_variable_frame_rate,
            is_hdr = EXCLUDED.is_hdr,
            motion_intensity = EXCLUDED.motion_intensity,
            temporal_stability = EXCLUDED.temporal_stability,
            embedding = EXCLUDED.embedding,
            quality_score = EXCLUDED.quality_score,
            metadata = EXCLUDED.metadata",
        &[
            &sample.blake3,
            &sample.source_path,
            &sample.width,
            &sample.height,
            &duration_secs,
            &sample.frame_count,
            &fps,
            &sample.file_size_bytes,
            &sample.format,
            &bitrate_mbps,
            &bit_depth,
            &crate::media_conversion_gate::db_optional_bool_or_false(
                sample.has_audio,
                "has_audio",
                "multi_scenario video_quality ingest",
            ),
            &crate::media_conversion_gate::db_optional_bool_or_false(
                sample.is_variable_frame_rate,
                "is_variable_frame_rate",
                "multi_scenario video_quality ingest",
            ),
            &crate::media_conversion_gate::db_optional_bool_or_false(
                sample.is_hdr,
                "is_hdr",
                "multi_scenario video_quality ingest",
            ),
            &motion_intensity,
            &temporal_stability,
            &sample.embedding,
            &quality_score,
            &sample.metadata,
        ],
    )
    .context("Failed to ingest video quality sample")?;
    Ok(())
}

/// Ingest into `loop_samples` (`261D`).
///
/// # Errors
/// Returns an error if the sample is missing required embedding, label, or
/// frame geometry, or if the insert fails.
pub fn ingest_loop_intent_sample(conn: &mut Client, sample: &ScenarioSample) -> Result<()> {
    validate_embedding(sample)?;

    // 🛡️ NO SILENT LOSS: Explicitly bail if label is missing or unmapped
    let label_str = sample
        .label
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Label required for LoopIntent training data"))?;
    let numeric_label = match label_str.as_str() {
        "high" | "low" => 1i16,
        "video" => 0i16,
        _ => anyhow::bail!("Unsupported LoopIntent label: '{label_str}'"),
    };

    let duration_secs = require_finite_real(sample.duration_secs, "duration_secs", sample)?;
    let fps = normalize_optional_finite_real(sample.fps, "fps", sample)?;
    let motion_periodicity =
        normalize_optional_finite_real(sample.motion_periodicity, "motion_periodicity", sample)?;
    let temporal_jitter =
        normalize_optional_finite_real(sample.temporal_jitter, "temporal_jitter", sample)?;
    let motion_gini = normalize_optional_finite_real(sample.motion_gini, "motion_gini", sample)?;
    let loop_closure_score =
        normalize_optional_finite_real(sample.loop_closure_score, "loop_closure_score", sample)?;
    let cadence_score =
        normalize_optional_finite_real(sample.cadence_score, "cadence_score", sample)?;

    if sample.width <= 0 || sample.height <= 0 || sample.frame_count <= 1 || duration_secs <= 0.0 {
        anyhow::bail!("Invalid dimensions or temporal metadata for LoopIntent ingestion");
    }

    ensure_no_loop_label_conflict(conn, &sample.blake3, numeric_label)
        .context("LoopIntent conflict precheck failed")?;

    let exists: bool = conn
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM loop_samples WHERE blake3 = $1)",
            &[&sample.blake3],
        )?
        .get(0);
    if !exists {
        let row = conn.query_one(
            "SELECT COUNT(*) FROM loop_samples WHERE label = $1",
            &[&numeric_label],
        )?;
        let count: i64 = row.get(0);
        if count >= crate::constants::LOOP_INTENT_DB_CAP_PER_CLASS {
            anyhow::bail!(
                "INGESTION REJECTED: LoopIntent label '{}' has reached the maximum cap of {}.",
                label_str,
                crate::constants::LOOP_INTENT_DB_CAP_PER_CLASS
            );
        }
    }

    conn.execute(
        "INSERT INTO loop_samples (
            blake3, source_path, file_name, width, height, duration_secs, frame_count,
            fps, file_size_bytes, motion_periodicity, temporal_jitter, motion_gini,
            loop_closure_score, cadence_score, embedding, label, labeled_by, metadata
        )
         VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17, $18
         )
         ON CONFLICT (blake3) DO UPDATE SET
            source_path = EXCLUDED.source_path,
            file_name = EXCLUDED.file_name,
            width = EXCLUDED.width,
            height = EXCLUDED.height,
            duration_secs = EXCLUDED.duration_secs,
            frame_count = EXCLUDED.frame_count,
            fps = EXCLUDED.fps,
            motion_periodicity = EXCLUDED.motion_periodicity,
            temporal_jitter = EXCLUDED.temporal_jitter,
            motion_gini = EXCLUDED.motion_gini,
            loop_closure_score = EXCLUDED.loop_closure_score,
            cadence_score = EXCLUDED.cadence_score,
            embedding = EXCLUDED.embedding,
            metadata = EXCLUDED.metadata,
            file_size_bytes = EXCLUDED.file_size_bytes,
            label = EXCLUDED.label,
            labeled_by = EXCLUDED.labeled_by",
        &[
            &sample.blake3,
            &sample.source_path,
            &sample.file_name,
            &sample.width,
            &sample.height,
            &duration_secs,
            &sample.frame_count,
            &fps,
            &sample.file_size_bytes,
            &motion_periodicity,
            &temporal_jitter,
            &motion_gini,
            &loop_closure_score,
            &cadence_score,
            &sample.embedding,
            &numeric_label,
            &sample.labeled_by,
            &sample.metadata,
        ],
    )
    .context("LoopIntent upsert into loop_samples failed")?;
    Ok(())
}

/// Query nearest neighbors in a scenario table.
///
/// # Errors
/// Returns an error if the query cannot be executed against the backing
/// scenario table.
pub fn knn_lookup(
    conn: &mut Client,
    query: &ScenarioQuery,
    query_embedding: &pgvector::Vector,
) -> Result<Vec<(String, f64)>> {
    let table = query.scenario.table_name();
    let k = query.k_neighbors;
    let threshold = query.threshold_distance;

    let sql = format!(
        "SELECT blake3::text, (embedding <-> $1::vector) as dist
         FROM {table}
         WHERE (embedding <-> $1::vector) <= $2
         ORDER BY embedding <-> $1::vector
         LIMIT $3"
    );

    let k = i64::try_from(k).context("Neighbor count does not fit into BIGINT")?;
    let rows = conn.query(&sql, &[&query_embedding, &threshold, &k])?;
    Ok(rows
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect())
}

/// Query nearest neighbors and extract regression features for `LightGBM`.
///
/// # Errors
/// Returns an error if the query cannot be executed or the neighbor count does
/// not fit into the SQL parameter type.
pub fn knn_regression_lookup(
    conn: &mut Client,
    query: &ScenarioQuery,
    query_embedding: &pgvector::Vector,
) -> Result<Option<KnnRegressionFeatures>> {
    ensure_quality_regression_scenario(query.scenario)?;
    let table = query.scenario.table_name();
    let k = query.k_neighbors;
    let threshold = query.threshold_distance;

    let sql = format!(
        "SELECT quality_score, (embedding <-> $1::vector) as dist
         FROM {table}
         WHERE (embedding <-> $1::vector) <= $2
         AND quality_score IS NOT NULL
         ORDER BY embedding <-> $1::vector
         LIMIT $3"
    );

    let k = i64::try_from(k).context("Neighbor count does not fit into BIGINT")?;
    let rows = conn.query(&sql, &[&query_embedding, &threshold, &k])?;
    if rows.is_empty() {
        return Ok(None);
    }

    let mut scores = Vec::with_capacity(rows.len());
    let mut dists = Vec::with_capacity(rows.len());

    for row in rows {
        let score: f32 = row.get(0);
        let dist: f64 = row.get(1);
        let score_f = f64::from(score);
        if !score_f.is_finite() || !dist.is_finite() {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "quality_knn_regression",
                branch = "skip_corrupt_neighbor_row",
                "dropping neighbor row with non-finite score or distance"
            );
            continue;
        }
        scores.push(score_f.clamp(0.0, 1.0));
        dists.push(dist.max(0.0));
    }

    if scores.is_empty() {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "quality_knn_regression",
            branch = "all_neighbors_corrupt",
            "knn_regression_lookup: no finite neighbors after sanitation"
        );
        return Ok(None);
    }

    let n = crate::numeric_cast::usize_to_f64(scores.len());
    let mean = scores.iter().sum::<f64>() / n;
    let variance = scores.iter().map(|&s| (s - mean).powi(2)).sum::<f64>() / n;
    let std = variance.max(0.0).sqrt();
    let min = scores.iter().copied().fold(f64::INFINITY, f64::min);

    // 🛡️ ORDER INDEPENDENT: Find true nearest distance in case HNSW result set
    // order is fuzzy
    let dist_to_nearest = dists.iter().copied().fold(f64::INFINITY, f64::min);

    let mut weight_sum = 0.0;
    let mut weighted_score_sum = 0.0;
    for (score, dist) in scores.iter().zip(dists.iter()) {
        let w = 1.0 / (dist + 0.01);
        weight_sum += w;
        weighted_score_sum += score * w;
    }

    let dist_weighted_score = if weight_sum > 0.0 {
        weighted_score_sum / weight_sum
    } else {
        mean
    };
    let confidence = 1.0 / std.mul_add(dist_to_nearest, 1.0);

    let features = KnnRegressionFeatures {
        knn_score_mean_k5: mean,
        knn_score_std_k5: std,
        knn_score_min_k5: min,
        dist_to_nearest,
        dist_weighted_score,
        confidence,
        neighbor_count: scores.len(),
    };
    if !features.is_usable_for_regression() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "quality_knn_regression",
            branch = "aggregated_features_non_finite",
            ?features,
            "knn_regression_lookup produced unusable features; refusing to forward to model stack"
        );
        return Ok(None);
    }

    Ok(features.seal_aggregates())
}

/// Get sample count for a scenario.
///
/// # Errors
/// Returns an error if the scenario table cannot be queried.
pub fn sample_count(conn: &mut Client, scenario: ScenarioType) -> Result<i64> {
    let table = scenario.table_name();
    let row = conn.query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])?;
    Ok(row.get(0))
}

/// Metadata record for a single scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMetadata {
    pub scenario: String,
    pub table_name: String,
    pub embedding_dimension: usize,
    pub sample_count: i64,
    pub last_updated: String,
}

/// Fetch scenario-specific metadata from the registry
///
/// # Errors
/// Returns an error if the metadata row is missing or cannot be decoded.
pub fn get_scenario_metadata(
    conn: &mut Client,
    scenario: ScenarioType,
) -> Result<ScenarioMetadata> {
    let scenario_str = scenario.to_string();
    let row = conn.query_opt(
        "SELECT scenario, table_name, embedding_dimension, sample_count, last_updated::text
         FROM multi_scenario_metadata
         WHERE scenario = $1",
        &[&scenario_str],
    )?;

    if let Some(row) = row {
        let live_sample_count = sample_count(conn, scenario)?;
        Ok(ScenarioMetadata {
            scenario: row.get(0),
            table_name: row.get(1),
            embedding_dimension: usize::try_from(row.get::<_, i32>(2))
                .context("Embedding dimension from metadata was negative")?,
            sample_count: live_sample_count,
            last_updated: crate::media_conversion_gate::db_optional_string_or_empty(
                row.get(4),
                "last_updated",
                "multi_scenario metadata",
            ),
        })
    } else {
        anyhow::bail!("Metadata not found for scenario: {scenario_str}")
    }
}

/// Unified table verification with dimension checks
///
/// # Errors
/// Returns an error if any scenario table is missing or has the wrong embedding
/// dimension.
pub fn init_all_scenarios(conn: &mut Client) -> Result<()> {
    for scenario in ScenarioType::all() {
        verify_scenario_table(conn, *scenario)?;
    }
    Ok(())
}

fn verify_scenario_table(conn: &mut Client, scenario: ScenarioType) -> Result<()> {
    let table_name = scenario.table_name();
    let expected_dim = i32::try_from(scenario.embedding_dimension())
        .context("Embedding dimension does not fit into PostgreSQL INT")?;

    let exists: bool = conn
        .query_opt(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = $1)",
            &[&table_name],
        )?
        .is_some_and(|row| row.get(0));

    if !exists {
        anyhow::bail!(crate::media_conversion_gate::ui_user_facing_error(format!(
            "Missing table: {table_name}"
        )));
    }

    let actual_dim = embedding_column_type(conn, table_name)?
        .as_deref()
        .and_then(parse_vector_dimension);

    if let Some(dim) = actual_dim
        && dim != expected_dim
    {
        anyhow::bail!(crate::media_conversion_gate::ui_user_facing_error(format!(
            "Dimension mismatch for {table_name}: expected {expected_dim}, found {dim}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn knn_regression_seal_aggregates_rejects_non_contract_row() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_ALGORITHM_SEAL,
            "0",
        );
        let raw = KnnRegressionFeatures {
            knn_score_mean_k5: f64::NAN,
            knn_score_std_k5: -1.0,
            knn_score_min_k5: 2.0,
            dist_to_nearest: f64::INFINITY,
            dist_weighted_score: f64::NAN,
            confidence: 1.5,
            neighbor_count: 3,
        };
        assert!(raw.seal_aggregates().is_none());
    }

    #[test]
    fn test_knn_regression_features_is_usable_for_regression() {
        let ok = KnnRegressionFeatures {
            knn_score_mean_k5: 0.5,
            knn_score_std_k5: 0.1,
            knn_score_min_k5: 0.3,
            dist_to_nearest: 0.2,
            dist_weighted_score: 0.55,
            confidence: 0.8,
            neighbor_count: 3,
        };
        assert!(ok.is_usable_for_regression());
        let bad = KnnRegressionFeatures {
            neighbor_count: 0,
            ..ok
        };
        assert!(!bad.is_usable_for_regression());
    }

    #[test]
    fn test_scenario_sample_builder_complete() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality)
            .with_path("/test/path.png".into())
            .with_label("png-high".into())
            .with_dimensions(1920, 1080)
            .with_size(5000)
            .with_format("PNG".into())
            .with_entropy(Some(7.5))
            .with_lossless(true)
            .with_quality_score(1.0);

        assert_eq!(sample.width, 1920);
        assert_eq!(sample.file_size_bytes, 5000);
        assert_eq!(sample.format, "PNG");
        assert_eq!(sample.entropy, Some(7.5));
        assert_eq!(sample.quality_score, Some(1.0));
        assert_eq!(sample.is_lossless, Some(true));
    }

    #[test]
    fn test_resolve_image_quality_label_canonicalizes_generic_png_label() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality)
            .with_label("high".into())
            .with_format("PNG".into())
            .with_quality_score(1.0);

        assert_eq!(
            resolve_image_quality_label(&sample, 1.0).unwrap(), /* audited: db module unit-test
                                                                 * fixture assertion; not
                                                                 * production DB runtime path */
            crate::scenario::ImageQualityLabel::PngHigh
        );
    }

    #[test]
    fn test_resolve_image_quality_label_rejects_format_mismatch() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality)
            .with_label("png-high".into())
            .with_format("WEBP".into())
            .with_quality_score(1.0);

        assert!(resolve_image_quality_label(&sample, 1.0).is_err());
    }

    #[test]
    fn test_resolve_image_quality_label_rejects_score_mismatch() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality)
            .with_label("modern-low".into())
            .with_format("AVIF".into())
            .with_quality_score(1.0);

        assert!(resolve_image_quality_label(&sample, 1.0).is_err());
    }

    #[test]
    fn test_video_scenario_sample_builder_metadata() {
        let sample = ScenarioSample::new(vec![9, 9, 9], ScenarioType::VideoQuality)
            .with_size(42_000)
            .with_frame_count(144)
            .with_format("h264".into())
            .with_bitrate_mbps(3.5)
            .with_bit_depth_opt(Some(10))
            .with_has_audio(true)
            .with_is_variable_frame_rate(true)
            .with_is_hdr(true);

        assert_eq!(sample.file_size_bytes, 42_000);
        assert_eq!(sample.frame_count, 144);
        assert_eq!(sample.format, "h264");
        assert_eq!(sample.bitrate_mbps, Some(3.5));
        assert_eq!(sample.bit_depth, Some(10));
        assert_eq!(sample.has_audio, Some(true));
        assert_eq!(sample.is_variable_frame_rate, Some(true));
        assert_eq!(sample.is_hdr, Some(true));
    }

    #[test]
    fn test_require_finite_metric_rejects_nan() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality);
        assert!(require_finite_metric(Some(f64::NAN), "entropy", &sample).is_err());
    }

    #[test]
    fn test_validate_image_quality_embedding_rejects_optional_nan_measurement_slots() {
        let mut emb = vec![0.1_f32; 256];
        emb[19] = f32::NAN;
        for slot in emb.iter_mut().skip(31) {
            *slot = 0.5;
        }
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality)
            .with_embedding(pgvector::Vector::from(emb));
        assert!(validate_embedding(&sample).is_err());
    }

    #[test]
    fn test_validate_image_quality_embedding_allows_pgvector_safe_missing_sentinel() {
        let mut emb = vec![0.1_f32; 256];
        emb[crate::image_quality_db::QUALITY_EMBED_COLOR_DEPTH_SLOT] =
            crate::image_quality_db::QUALITY_EMBED_MISSING_MEASUREMENT;
        emb[crate::image_quality_db::QUALITY_EMBED_PSNR_SLOT] =
            crate::image_quality_db::QUALITY_EMBED_MISSING_MEASUREMENT;
        emb[crate::image_quality_db::QUALITY_EMBED_SSIM_SLOT] =
            crate::image_quality_db::QUALITY_EMBED_MISSING_MEASUREMENT;
        emb[crate::image_quality_db::QUALITY_EMBED_JPEG_QUALITY_SLOT] =
            crate::image_quality_db::QUALITY_EMBED_MISSING_MEASUREMENT;
        emb[crate::image_quality_db::QUALITY_EMBED_JPEG_CONFIDENCE_SLOT] =
            crate::image_quality_db::QUALITY_EMBED_MISSING_MEASUREMENT;
        for slot in emb.iter_mut().skip(31) {
            *slot = 0.5;
        }
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality)
            .with_embedding(pgvector::Vector::from(emb));
        assert!(validate_embedding(&sample).is_ok());
    }

    #[test]
    fn test_validate_image_quality_embedding_rejects_nan_outside_optional_slots() {
        let mut emb = vec![0.1_f32; 256];
        emb[0] = f32::NAN;
        for slot in emb.iter_mut().skip(31) {
            *slot = 0.5;
        }
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::ImageQuality)
            .with_embedding(pgvector::Vector::from(emb));
        assert!(validate_embedding(&sample).is_err());
    }

    #[test]
    fn test_normalize_optional_positive_real_rejects_zero_and_infinity() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::AnimatedImageQuality);
        assert!(normalize_optional_positive_real(Some(0.0), "fps", &sample).is_err());
        assert!(normalize_optional_positive_real(Some(f64::INFINITY), "fps", &sample).is_err());
    }

    #[test]
    fn test_normalize_optional_non_negative_i32_rejects_negative_values() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::AnimatedImageQuality);
        assert!(normalize_optional_non_negative_i32(Some(-1), "palette_size", &sample).is_err());
    }

    #[test]
    fn test_normalize_optional_positive_i16_rejects_zero() {
        let sample = ScenarioSample::new(vec![1, 2, 3], ScenarioType::VideoQuality);
        assert!(normalize_optional_positive_i16(Some(0), "bit_depth", &sample).is_err());
    }

    #[test]
    fn metadata_sync_triggers_share_advisory_lock_key() {
        let source = include_str!("multi_scenario_db.rs");
        let expected_lock = MULTI_SCENARIO_SCHEMA_ADVISORY_LOCK_KEY.to_string();

        assert_eq!(
            source
                .matches(&format!("PERFORM pg_advisory_xact_lock({expected_lock});"))
                .count(),
            2,
            "sample-count and truncate metadata triggers must both serialize metadata row updates \
             with the Rust advisory key"
        );
    }

    #[test]
    fn schema_ddl_and_metadata_updates_use_distinct_advisory_locks() {
        assert_ne!(
            MULTI_SCENARIO_SCHEMA_DDL_ADVISORY_LOCK_KEY, MULTI_SCENARIO_SCHEMA_ADVISORY_LOCK_KEY,
            "schema DDL must not hold the metadata advisory lock while waiting for table locks"
        );
    }

    #[test]
    fn test_ensure_quality_regression_scenario_rejects_loop_intent() {
        assert!(ensure_quality_regression_scenario(ScenarioType::LoopIntent).is_err());
        assert!(ensure_quality_regression_scenario(ScenarioType::ImageQuality).is_ok());
    }
}
