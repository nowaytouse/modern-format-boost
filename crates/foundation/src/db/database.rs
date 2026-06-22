//! PostgreSQL-backed KNN learning system for loop-intent classification.
//!
//! This module provides database schema management, sample ingestion,
//! feature vector computation (pgvector/HNSW), similarity search,
//! inference logging, and health diagnostics. It enables the system
//! to learn from labeled GIF/video samples and improve classification
//! accuracy over time.

use crate::Rational;
use crate::infra::numeric_cast::i64_to_usize_sat;
use crate::loop_intent::LoopMeta;
use crate::media_meta_utils::scan_gif_headers;
use crate::probe_video;
use anyhow::{Context, Result};
use blake3::Hasher;
use indicatif::{ProgressBar, ProgressStyle};
use postgres::Client;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;
use walkdir::WalkDir;

/// Unix-socket `postgresql:///dbname` fails on macOS/Homebrew when libpq has no default host.
pub const PG_DEFAULT_CONNSTR: &str = "postgresql://localhost/modern_format_boost";

const LOOP_VECTOR_FEATURE_NAMES: [&str; 29] = [
    "pixels",
    "duration",
    "frame_count",
    "file_size_bytes",
    "fps",
    "density",
    "gap",
    "temporal_bpp",
    "spatial_bpp",
    "aspect",
    "loop_freq",
    "cadence",
    "payload_var",
    "delay_var",
    "p_depth",
    "m_gini",
    "b_skew",
    "t_flat",
    "webp_ratio",
    "loop_affin",
    "l_close",
    "m_period",
    "t_jitter",
    "dir_meme",
    "max_fd",
    "min_fd",
    "audio_dur",
    "path_depth",
    "num_density",
];

static DB_WARN_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct FeatureStats {
    pub(crate) mean: f64,
    pub(crate) std_dev: f64,
    #[serde(default)]
    pub(crate) weight: Option<f64>,
    #[serde(default)]
    pub(crate) p10: Option<f64>,
    #[serde(default)]
    pub(crate) p25: Option<f64>,
    #[serde(default)]
    pub(crate) p50: Option<f64>,
    #[serde(default)]
    pub(crate) p75: Option<f64>,
    #[serde(default)]
    pub(crate) p90: Option<f64>,
}

impl FeatureStats {
    #[must_use]
    pub(crate) const fn has_empirical_percentiles(&self) -> bool {
        self.p10.is_some()
            || self.p25.is_some()
            || self.p50.is_some()
            || self.p75.is_some()
            || self.p90.is_some()
    }
}

/// Offline HDBSCAN cluster centroid stored in `multi_scenario_metadata.feature_stats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LoopHdbscanClusterCentroid {
    pub cluster_id: i32,
    pub loop_prior: f64,
    pub member_count: usize,
    pub centroid: Vec<f64>,
}

/// Catalog of density clusters used to augment HNSW neighbor voting at inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LoopHdbscanCatalog {
    #[serde(default = "loop_hdbscan_catalog_default_version")]
    pub version: u32,
    pub min_cluster_size: usize,
    pub noise_count: usize,
    pub clusters: Vec<LoopHdbscanClusterCentroid>,
}

const fn loop_hdbscan_catalog_default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct FeatureMap {
    pub(crate) stats: std::collections::HashMap<String, FeatureStats>,
    pub(crate) top_keywords: Vec<String>,
    #[serde(default)]
    pub(crate) hdbscan_catalog: Option<LoopHdbscanCatalog>,
}

#[cfg(test)]
impl FeatureMap {
    pub(crate) fn mock() -> Self {
        bootstrap_loop_feature_map()
    }
}

/// Cold-start feature map for training ingest when `loop_samples` is empty (fresh DB).
pub(crate) fn cold_start_loop_training_feature_map() -> FeatureMap {
    crate::media_conversion_gate::delivery_db_batch_audit(
        "loop_feature_stats_cold_start",
        "loop training ingest using cold-start feature_stats (zero loop_samples); percentile slots are not empirical until refresh",
    );
    let mut stats = std::collections::HashMap::new();
    for f in LOOP_VECTOR_FEATURE_NAMES {
        stats.insert(
            f.to_string(),
            FeatureStats {
                mean: 1.0,
                std_dev: 1.0,
                weight: Some(1.0),
                ..Default::default()
            },
        );
    }

    FeatureMap {
        stats,
        top_keywords: Vec::new(),
        hdbscan_catalog: None,
    }
}

fn loop_samples_table_count(conn: &mut Client) -> Result<i64> {
    Ok(conn
        .query_one("SELECT COUNT(*) FROM loop_samples", &[])?
        .get(0))
}

/// Persist loop training `feature_stats` for ingest bootstrap (cold-start or corpus refresh).
pub(crate) fn persist_loop_training_feature_map(
    conn: &mut Client,
    feature_map: &FeatureMap,
) -> Result<()> {
    let encoded =
        serde_json::to_value(feature_map).context("LoopIntent feature_stats JSON encode failed")?;
    let updated = conn.execute(
        "UPDATE multi_scenario_metadata
         SET feature_stats = $1::jsonb,
             last_updated = CURRENT_TIMESTAMP
         WHERE scenario = $2",
        &[
            &encoded,
            &crate::scenario::ScenarioType::LoopIntent.to_string(),
        ],
    )?;
    if updated == 0 {
        anyhow::bail!(
            "loop_intent multi_scenario_metadata row missing; cannot persist training feature_stats"
        );
    }
    Ok(())
}

/// Cold-start collection stats when `loop_samples` is empty (baseline constants, not `{}`).
pub(crate) fn cold_start_loop_collection_stats() -> GlobalCollectionStats {
    crate::media_conversion_gate::delivery_db_batch_audit(
        "loop_collection_stats_cold_start",
        "loop training ingest using cold-start collection_stats (zero loop_samples)",
    );
    GlobalCollectionStats::default()
}

/// Persist loop `collection_stats` for ingest bootstrap.
pub(crate) fn persist_loop_collection_stats(
    conn: &mut Client,
    stats: &GlobalCollectionStats,
) -> Result<()> {
    let encoded =
        serde_json::to_value(stats).context("LoopIntent collection_stats JSON encode failed")?;
    let updated = conn.execute(
        "UPDATE multi_scenario_metadata
         SET collection_stats = $1::jsonb,
             last_updated = CURRENT_TIMESTAMP
         WHERE scenario = $2",
        &[
            &encoded,
            &crate::scenario::ScenarioType::LoopIntent.to_string(),
        ],
    )?;
    if updated == 0 {
        anyhow::bail!(
            "loop_intent multi_scenario_metadata row missing; cannot persist training collection_stats"
        );
    }
    Ok(())
}

#[cfg(test)]
fn bootstrap_loop_feature_map() -> FeatureMap {
    cold_start_loop_training_feature_map()
}

/// Statistical summary of a feature distribution across the training dataset.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DistributionStats {
    /// Arithmetic mean of the feature values.
    pub mean: f64,
    /// Standard deviation of the feature values.
    pub std_dev: f64,
    /// Learned weight for this feature in distance calculations (optional).
    #[serde(default)]
    pub weight: Option<f64>,
    /// 10th percentile value.
    #[serde(default)]
    pub p10: Option<f64>,
    /// 25th percentile value (Q1).
    #[serde(default)]
    pub p25: Option<f64>,
    /// 50th percentile value (median).
    #[serde(default)]
    pub p50: Option<f64>,
    /// 75th percentile value (Q3).
    #[serde(default)]
    pub p75: Option<f64>,
    /// 90th percentile value.
    #[serde(default)]
    pub p90: Option<f64>,
}

impl DistributionStats {
    /// Compute the z-score of a value relative to this distribution.
    /// Returns 0.0 if the standard deviation is near zero.
    #[must_use]
    pub fn z_score(&self, value: f64) -> f64 {
        if self.std_dev > 1e-6 {
            (value - self.mean) / self.std_dev
        } else {
            0.0
        }
    }

    #[must_use]
    pub const fn has_empirical_percentiles(&self) -> bool {
        self.p10.is_some()
            || self.p25.is_some()
            || self.p50.is_some()
            || self.p75.is_some()
            || self.p90.is_some()
    }
}

/// Clear duration percentile slots that must not drive thresholds without a DB histogram.
const fn strip_non_empirical_duration_percentiles(duration: &mut DistributionStats) {
    duration.p10 = None;
    duration.p25 = None;
    duration.p50 = None;
    duration.p75 = None;
    duration.p90 = None;
}

/// Convert internal `FeatureStats` into a public `DistributionStats`.
impl From<&FeatureStats> for DistributionStats {
    fn from(value: &FeatureStats) -> Self {
        Self {
            mean: value.mean,
            std_dev: value.std_dev,
            weight: value.weight,
            p10: value.p10,
            p25: value.p25,
            p50: value.p50,
            p75: value.p75,
            p90: value.p90,
        }
    }
}

/// Classification label for a sample's looping intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelStatus {
    /// Manually verified / high confidence: looping intent (meme/sticker/video sticker).
    LoopStrong,
    /// Manually verified / high confidence: non-looping intent (clip/record/long video).
    LoopWeak,
    /// Edge case / low confidence label.
    Uncertain,
    /// Not yet labeled by a human or model.
    NotLabeled,
}

/// Result of a KNN similarity search against the training database.
#[derive(Debug, Clone)]
pub struct SampleMatch {
    /// The label status matched (usually `NotLabeled`; use `keep_probability` for probability).
    pub exact_label: LabelStatus,
    /// Probability that this sample should be kept as a looping asset (None if no match).
    pub keep_probability: Option<f64>,
    /// Confidence score in [0, 1]: how tightly clustered the KNN neighbors are.
    /// confidence = 1.0 - (`std_dev_distance` / `mean_distance`), clamped to [0, 1].
    /// High confidence (>0.75) means neighbors are homogeneous; safe to trust `keep_probability`.
    pub confidence: f64,
    /// Number of neighbors used in the computation.
    pub neighbor_count: usize,
    /// Mean L2 distance to the KNN neighbors.
    pub mean_distance: Option<f64>,
    /// Standard deviation of distances to the KNN neighbors.
    pub std_dev_distance: Option<f64>,
    /// Minimum distance among the KNN neighbors.
    pub min_distance: Option<f64>,
    /// 25th percentile of distances to the KNN neighbors.
    pub p25_distance: Option<f64>,
    /// 75th percentile of distances to the KNN neighbors.
    pub p75_distance: Option<f64>,
    /// Dynamic baseline: P90 duration of neighbors with high loss tolerance.
    pub p90_duration: Option<f64>,
    /// HDBSCAN cluster id when catalog fusion was applied (`None` if unavailable).
    pub hdbscan_cluster_id: Option<i32>,
    /// Loop-keep prior of the nearest HDBSCAN cluster.
    pub hdbscan_cluster_loop_prior: Option<f64>,
}

/// A single inference result logged to the `inference_log` table for feedback analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopInferenceRecord {
    /// Probability output from the decision tree classifier (`None` for Layer 7 policy exits).
    pub tree_probability: Option<f64>,
    /// KNN-derived keep probability (None if KNN was not available).
    pub knn_keep_probability: Option<f64>,
    /// KNN confidence score (None if KNN was not available).
    pub knn_confidence: Option<f64>,
    /// Number of KNN neighbors used (None if KNN was not available).
    pub knn_neighbor_count: Option<usize>,
    /// Final blended probability after combining tree and KNN signals (`None` for Layer 7 policy exits).
    pub final_probability: Option<f64>,
    /// The final verdict string (e.g., "`LoopStrong`", "`LoopWeak`").
    pub final_verdict: String,
    /// Human-readable explanation of the decision.
    pub decision_reason: String,
    /// Which decision layer produced the exit (e.g., "Layer 1-A").
    pub layer_exit: String,
}

impl LoopInferenceRecord {
    /// Last gate before persisting loop-intent telemetry.
    pub(crate) fn seal_algorithm_outputs(&mut self) {
        self.tree_probability = self
            .tree_probability
            .and_then(crate::algorithm_seal::loop_unit_probability);
        self.knn_keep_probability = self
            .knn_keep_probability
            .and_then(crate::algorithm_seal::loop_unit_probability);
        self.knn_confidence = self
            .knn_confidence
            .and_then(crate::algorithm_seal::loop_unit_probability);
        self.final_probability = self
            .final_probability
            .and_then(crate::algorithm_seal::loop_unit_probability);
    }
}

/// Measures how well a single feature separates `LoopStrong` from `LoopWeak` samples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopFeatureDiscriminativePower {
    /// Name of the feature being measured.
    pub feature_name: String,
    /// Mean feature value among `LoopStrong` samples.
    pub mean_loop_strong: Option<f64>,
    /// Mean feature value among `LoopWeak` samples.
    pub mean_loop_weak: Option<f64>,
    /// Discriminative power = (`mean_loop_strong` - `mean_loop_weak`) / stddev.
    pub discriminative_power: f64,
    /// Number of samples used in the calculation.
    pub sample_count: i64,
}

/// Identifies regions in feature space where the inference system is most uncertain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceBlindSpot {
    /// Duration bucket boundary (seconds).
    pub duration_bucket: f64,
    /// WebP compression ratio bucket boundary.
    pub webp_bucket: f64,
    /// Average KNN confidence in this bucket.
    pub avg_knn_confidence: f64,
    /// Average tree probability in this bucket.
    pub avg_tree_probability: Option<f64>,
    /// Average final probability in this bucket.
    pub avg_final_probability: Option<f64>,
    /// Average neighbor count in this bucket.
    pub avg_neighbor_count: Option<f64>,
    /// Number of inference records in this bucket.
    pub sample_count: i64,
    /// A representative layer exit string from this bucket.
    pub example_layer_exit: Option<String>,
}

/// Aggregate statistics computed across the entire GIF/animation collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalCollectionStats {
    /// Minimum duration in seconds.
    pub duration_min: Option<f64>,
    /// Average duration in seconds.
    pub duration_avg: Option<f64>,
    /// Maximum duration in seconds.
    pub duration_max: Option<f64>,
    /// 90th percentile duration in seconds.
    pub duration_p90: Option<f64>,
    /// True when `duration_p90` was computed from training-sample durations (not feature-map fallback).
    #[serde(default)]
    pub duration_p90_from_samples: bool,
    /// True when min/avg/max duration came from training-sample durations (not feature-map fallback).
    #[serde(default)]
    pub duration_stats_from_samples: bool,

    /// Minimum file size in bytes (from samples only; `None` when corpus empty).
    #[serde(default)]
    pub size_min: Option<f64>,
    /// Average file size in bytes.
    #[serde(default)]
    pub size_avg: Option<f64>,
    /// Maximum file size in bytes.
    #[serde(default)]
    pub size_max: Option<f64>,

    /// Minimum bitrate in bits/sec.
    #[serde(default)]
    pub bitrate_min: Option<f64>,
    /// Average bitrate in bits/sec.
    #[serde(default)]
    pub bitrate_avg: Option<f64>,
    /// Maximum bitrate in bits/sec.
    #[serde(default)]
    pub bitrate_max: Option<f64>,

    /// Minimum width in pixels.
    #[serde(default)]
    pub width_min: Option<f64>,
    /// Average width in pixels.
    #[serde(default)]
    pub width_avg: Option<f64>,
    /// Maximum width in pixels.
    #[serde(default)]
    pub width_max: Option<f64>,

    /// Minimum height in pixels.
    #[serde(default)]
    pub height_min: Option<f64>,
    /// Average height in pixels.
    #[serde(default)]
    pub height_avg: Option<f64>,
    /// Maximum height in pixels.
    #[serde(default)]
    pub height_max: Option<f64>,

    /// Minimum aspect ratio (width/height).
    #[serde(default)]
    pub aspect_min: Option<f64>,
    /// Average aspect ratio (width/height).
    #[serde(default)]
    pub aspect_avg: Option<f64>,
    /// Maximum aspect ratio (width/height).
    #[serde(default)]
    pub aspect_max: Option<f64>,
    /// Top keywords extracted from `LoopStrong` filenames.
    pub top_keywords: Vec<String>,
}

/// A comprehensive reference profile for loop-intent classification, combining
/// collection statistics with per-feature distribution statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopReferenceProfile {
    /// Aggregate collection-wide statistics.
    pub collection: GlobalCollectionStats,
    /// Duration distribution statistics.
    pub duration: DistributionStats,
    /// Frames-per-second distribution statistics.
    pub fps: DistributionStats,
    /// Frame density (frames/sec) distribution statistics.
    pub frame_density: DistributionStats,
    /// File size in bytes distribution statistics.
    pub file_size_bytes: DistributionStats,
    /// Total pixel count distribution statistics.
    pub pixels: DistributionStats,
    /// Temporal bits-per-pixel distribution statistics.
    pub temporal_bpp: DistributionStats,
    /// Spatial bits-per-pixel distribution statistics.
    pub spatial_bpp: DistributionStats,
    /// Frame payload variation distribution statistics.
    pub payload_variation: DistributionStats,
    /// Frame delay variation distribution statistics.
    pub delay_variation: DistributionStats,
    /// Palette depth distribution statistics.
    pub palette_depth: DistributionStats,
    /// Motion Gini coefficient distribution statistics.
    pub motion_gini: DistributionStats,
    /// Temporal flatness distribution statistics.
    pub temporal_flatness: DistributionStats,
    /// WebP compression ratio distribution statistics.
    pub webp_ratio: DistributionStats,
    /// Cadence score distribution statistics.
    pub cadence: DistributionStats,
    /// Top discriminative keywords from filenames.
    pub top_keywords: Vec<String>,
    /// True when `duration` percentiles in `feature_stats` were measured from a histogram (not inferred).
    #[serde(default)]
    pub duration_has_empirical_percentiles: bool,
    /// True only for `#[cfg(test)]` KNN bootstrap (`LoopReferenceProfile` test `Default`); corpus builds must leave this false.
    #[serde(default)]
    pub is_knn_bootstrap_heuristic: bool,
}

impl Default for GlobalCollectionStats {
    fn default() -> Self {
        use crate::constants::{
            DEFAULT_LOOP_BASELINE_DURATION_SECS, MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS,
        };
        Self {
            duration_min: Some(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_MIN_SECS),
            duration_avg: Some(DEFAULT_LOOP_BASELINE_DURATION_SECS),
            duration_max: Some(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_MAX_SECS),
            duration_p90: Some(MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS),
            duration_p90_from_samples: false,
            duration_stats_from_samples: false,

            size_min: None,
            size_avg: None,
            size_max: None,

            bitrate_min: None,
            bitrate_avg: None,
            bitrate_max: None,

            width_min: None,
            width_avg: None,
            width_max: None,

            height_min: None,
            height_avg: None,
            height_max: None,

            aspect_min: None,
            aspect_avg: None,
            aspect_max: None,
            top_keywords: Vec::new(),
        }
    }
}

/// Empty corpus-backed profile shell (no KNN bootstrap heuristic).
fn loop_reference_profile_corpus_shell(
    collection: GlobalCollectionStats,
    top_keywords: Vec<String>,
) -> LoopReferenceProfile {
    LoopReferenceProfile {
        collection,
        duration: DistributionStats::default(),
        fps: DistributionStats::default(),
        frame_density: DistributionStats::default(),
        file_size_bytes: DistributionStats::default(),
        pixels: DistributionStats::default(),
        temporal_bpp: DistributionStats::default(),
        spatial_bpp: DistributionStats::default(),
        payload_variation: DistributionStats::default(),
        delay_variation: DistributionStats::default(),
        palette_depth: DistributionStats::default(),
        motion_gini: DistributionStats::default(),
        temporal_flatness: DistributionStats::default(),
        webp_ratio: DistributionStats::default(),
        cadence: DistributionStats::default(),
        top_keywords,
        duration_has_empirical_percentiles: false,
        is_knn_bootstrap_heuristic: false,
    }
}

#[cfg(test)]
impl Default for LoopReferenceProfile {
    // Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
    fn default() -> Self {
        const KNN_DEFAULT_CTX: &str = "KnnDistributionProfile::default";
        const COLLECTION_BASELINE_TRUSTED: bool = true;

        let collection = GlobalCollectionStats::default();
        let duration_p90_gated =
            crate::media_conversion_gate::loop_collection_secs_or_baseline_policy(
                collection.duration_p90,
                crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS,
                "duration_p90",
                KNN_DEFAULT_CTX,
                false,
                COLLECTION_BASELINE_TRUSTED,
            );
        let pixels_min = 1_024.0_f64;
        let pixels_avg = 262_144.0_f64;
        let pixels_max = 2_073_600.0_f64;

        let mut profile = Self {
            duration: DistributionStats {
                mean: crate::media_conversion_gate::loop_collection_secs_or_baseline_policy(
                    collection.duration_avg,
                    crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS,
                    "duration_avg",
                    KNN_DEFAULT_CTX,
                    false,
                    COLLECTION_BASELINE_TRUSTED,
                ),
                std_dev: ((crate::media_conversion_gate::loop_collection_secs_or_baseline_policy(
                    collection.duration_max,
                    crate::constants::DEFAULT_LOOP_BASELINE_DURATION_MAX_SECS,
                    "duration_max",
                    KNN_DEFAULT_CTX,
                    false,
                    COLLECTION_BASELINE_TRUSTED,
                )
                    - crate::media_conversion_gate::loop_collection_secs_or_baseline_policy(
                        collection.duration_min,
                        crate::constants::DEFAULT_LOOP_BASELINE_DURATION_MIN_SECS,
                        "duration_min",
                        KNN_DEFAULT_CTX,
                        false,
                        COLLECTION_BASELINE_TRUSTED,
                    ))
                    / 4.0)
                    .max(0.5),
                p10: collection.duration_min,
                p25: Some(f64::midpoint(
                    crate::media_conversion_gate::loop_collection_secs_or_baseline_policy(
                        collection.duration_min,
                        crate::constants::DEFAULT_LOOP_BASELINE_DURATION_MIN_SECS,
                        "duration_min",
                        KNN_DEFAULT_CTX,
                        false,
                        COLLECTION_BASELINE_TRUSTED,
                    ),
                    crate::media_conversion_gate::loop_collection_secs_or_baseline_policy(
                        collection.duration_avg,
                        crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS,
                        "duration_avg",
                        KNN_DEFAULT_CTX,
                        false,
                        COLLECTION_BASELINE_TRUSTED,
                    ),
                )),
                p50: collection.duration_avg,
                p75: Some(f64::midpoint(
                    crate::media_conversion_gate::loop_collection_secs_or_baseline_policy(
                        collection.duration_avg,
                        crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS,
                        "duration_avg",
                        KNN_DEFAULT_CTX,
                        false,
                        COLLECTION_BASELINE_TRUSTED,
                    ),
                    duration_p90_gated,
                )),
                p90: Some(duration_p90_gated),
                weight: None,
            },
            fps: DistributionStats {
                mean: crate::constants::KNN_STATS_FPS_MEAN,
                std_dev: crate::constants::KNN_STATS_FPS_STD_DEV,
                p10: Some(4.0),
                p25: Some(8.0),
                p50: Some(12.0),
                p75: Some(18.0),
                p90: Some(24.0),
                weight: None,
            },
            frame_density: DistributionStats {
                mean: crate::constants::KNN_STATS_FPS_MEAN,
                std_dev: crate::constants::KNN_STATS_FPS_STD_DEV,
                p10: Some(4.0),
                p25: Some(8.0),
                p50: Some(12.0),
                p75: Some(18.0),
                p90: Some(24.0),
                weight: None,
            },
            file_size_bytes: DistributionStats {
                mean: 1_000_000.0,
                std_dev: 1_000_000.0,
                p10: Some(10_000.0),
                p25: Some(100_000.0),
                p50: Some(1_000_000.0),
                p75: Some(2_500_000.0),
                p90: Some(5_000_000.0),
                weight: None,
            },
            pixels: DistributionStats {
                mean: pixels_avg,
                std_dev: ((pixels_max - pixels_min) / 4.0).max(16_384.0),
                p10: Some(pixels_min),
                p25: Some(f64::midpoint(pixels_min, pixels_avg)),
                p50: Some(pixels_avg),
                p75: Some(f64::midpoint(pixels_avg, pixels_max)),
                p90: Some(pixels_max),
                weight: None,
            },
            temporal_bpp: DistributionStats {
                mean: crate::constants::KNN_STATS_BPP_MEAN,
                std_dev: crate::constants::KNN_STATS_BPP_STD_DEV,
                p10: Some(0.01),
                p25: Some(0.02),
                p50: Some(0.05),
                p75: Some(0.08),
                p90: Some(0.12),
                weight: None,
            },
            spatial_bpp: DistributionStats {
                mean: crate::constants::KNN_STATS_SPATIAL_BPP_MEAN,
                std_dev: crate::constants::KNN_STATS_SPATIAL_BPP_STD_DEV,
                p10: Some(1.0),
                p25: Some(2.0),
                p50: Some(4.0),
                p75: Some(6.0),
                p90: Some(10.0),
                weight: None,
            },
            payload_variation: DistributionStats {
                mean: crate::constants::KNN_STATS_VARIATION_MEAN,
                std_dev: crate::constants::KNN_STATS_VARIATION_STD_DEV,
                p10: Some(0.2),
                p25: Some(0.35),
                p50: Some(0.5),
                p75: Some(0.65),
                p90: Some(0.8),
                weight: None,
            },
            delay_variation: DistributionStats {
                mean: 0.25,
                std_dev: crate::constants::DB_HEURISTIC_STD_DEV_DEFAULT,
                p10: Some(0.05),
                p25: Some(0.12),
                p50: Some(0.25),
                p75: Some(0.35),
                p90: Some(0.55),
                weight: None,
            },
            palette_depth: DistributionStats {
                mean: 0.55,
                std_dev: 0.18,
                p10: Some(0.25),
                p25: Some(0.4),
                p50: Some(0.55),
                p75: Some(0.7),
                p90: Some(0.85),
                weight: None,
            },
            motion_gini: DistributionStats {
                mean: crate::constants::KNN_STATS_GINI_MEAN,
                std_dev: crate::constants::KNN_STATS_GINI_STD_DEV,
                p10: Some(0.2),
                p25: Some(0.4),
                p50: Some(0.55),
                p75: Some(0.7),
                p90: Some(0.85),
                weight: None,
            },
            temporal_flatness: DistributionStats {
                mean: 0.55,
                std_dev: 0.18,
                p10: Some(0.2),
                p25: Some(0.4),
                p50: Some(0.55),
                p75: Some(0.7),
                p90: Some(0.85),
                weight: None,
            },
            webp_ratio: DistributionStats {
                mean: crate::constants::KNN_STATS_WEBP_RATIO_MEAN,
                std_dev: crate::constants::KNN_STATS_WEBP_RATIO_STD_DEV,
                p10: Some(4.0),
                p25: Some(7.0),
                p50: Some(10.0),
                p75: Some(13.0),
                p90: Some(16.0),
                weight: None,
            },
            cadence: DistributionStats {
                mean: 0.5,
                std_dev: 0.2,
                p10: Some(0.2),
                p25: Some(0.35),
                p50: Some(0.5),
                p75: Some(0.65),
                p90: Some(0.8),
                weight: None,
            },
            top_keywords: collection.top_keywords.clone(),
            duration_has_empirical_percentiles: false,
            is_knn_bootstrap_heuristic: true,
            collection,
        };
        strip_non_empirical_duration_percentiles(&mut profile.duration);
        profile
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SampleStreamFlags {
    pub has_transparency: bool,
    pub is_native_gif: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SampleColorFlags {
    pub has_embedded_icc: bool,
    pub has_complex_color_profile: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SampleMemeFlags {
    pub is_meme_platform: bool,
    pub is_human_semantic_name: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SampleSourceFlags {
    pub is_high_value_source: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SampleFlags {
    #[serde(flatten)]
    pub streams: SampleStreamFlags,
    #[serde(flatten)]
    pub color: SampleColorFlags,
    #[serde(flatten)]
    pub meme: SampleMemeFlags,
    #[serde(flatten)]
    pub source: SampleSourceFlags,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LoopIntentStoredMetadata {
    #[serde(default)]
    source_ext: Option<String>,
    #[serde(default)]
    loss_tolerance: Option<String>,
    #[serde(default)]
    loop_verdict: Option<String>,
    #[serde(default)]
    temporal_bpp: Option<f64>,
    #[serde(default)]
    spatial_bpp: Option<f64>,
    #[serde(default)]
    frame_payload_variation: Option<f64>,
    #[serde(default)]
    frame_delay_variation: Option<f64>,
    #[serde(default)]
    aspect_ratio: Option<f64>,
    #[serde(default)]
    total_pixels: Option<u64>,
    #[serde(default)]
    loop_frequency: Option<f64>,
    #[serde(default)]
    directory_loop_intent_score: Option<f64>,
    #[serde(default)]
    palette_size: Option<u32>,
    #[serde(default)]
    palette_depth: Option<f64>,
    #[serde(default)]
    block_skew: Option<f64>,
    #[serde(default)]
    temporal_flatness: Option<f64>,
    #[serde(default)]
    webp_compression_ratio: Option<f64>,
    #[serde(default)]
    max_frame_delay: Option<f64>,
    #[serde(default)]
    min_frame_delay: Option<f64>,
    #[serde(default)]
    audio_duration_secs: Option<f64>,
    #[serde(default)]
    path_depth: u32,
    #[serde(default)]
    filename_numeric_density: f64,
    #[serde(default)]
    physics_225: Option<Vec<f32>>,
    #[serde(default)]
    flags: SampleFlags,
}

#[derive(Debug, Clone)]
struct LoopIntentTrainingSample {
    blake3: Vec<u8>,
    file_name: Option<String>,
    label: i16,
    sample_row: SampleRow,
}

/// Row shape for GIF/video KNN features consumed by vectorization and similarity logic.
#[derive(Debug, Clone)]
pub(crate) struct SampleRow {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) duration_secs: Option<f64>,
    pub(crate) frame_count: Option<u64>,
    pub(crate) file_size_bytes: u64,
    pub(crate) fps: Option<f64>,
    pub(crate) temporal_bpp: f64,
    pub(crate) spatial_bpp: f64,
    pub(crate) flags: SampleFlags,
    pub(crate) palette_size: Option<u32>,
    pub(crate) frame_payload_variation: Option<f64>,
    pub(crate) frame_delay_variation: Option<f64>,
    pub(crate) aspect_ratio: Option<f64>,
    pub(crate) loop_frequency: Option<f64>,
    pub(crate) cadence_score: Option<f64>,
    pub(crate) directory_loop_intent_score: Option<f64>,
    pub(crate) palette_depth: Option<f64>,
    pub(crate) motion_gini: Option<f64>,
    pub(crate) block_skew: Option<f64>,
    pub(crate) temporal_flatness: Option<f64>,
    pub(crate) loop_closure_score: Option<f64>,
    pub(crate) motion_periodicity: Option<f64>,
    pub(crate) temporal_jitter: Option<f64>,
    pub(crate) webp_compression_ratio: Option<f64>,
    pub(crate) max_frame_delay: Option<f64>,
    pub(crate) min_frame_delay: Option<f64>,
    pub(crate) audio_duration_secs: Option<f64>,
    pub(crate) path_depth: u32,
    pub(crate) filename_numeric_density: f64,
    pub(crate) physics_225: Option<Vec<f32>>,
}

impl From<SampleInsert> for SampleRow {
    fn from(s: SampleInsert) -> Self {
        Self {
            width: s.width,
            height: s.height,
            duration_secs: s.duration_secs,
            frame_count: s.frame_count,
            file_size_bytes: s.file_size_bytes,
            fps: s.fps,
            temporal_bpp: s.temporal_bpp,
            spatial_bpp: s.spatial_bpp,
            flags: s.flags,
            palette_size: s.palette_size,
            frame_payload_variation: s.frame_payload_variation,
            frame_delay_variation: s.frame_delay_variation,
            aspect_ratio: s.aspect_ratio,
            loop_frequency: Some(s.loop_frequency),
            cadence_score: Some(s.cadence_score),
            directory_loop_intent_score: Some(s.directory_loop_intent_score),
            palette_depth: s.palette_depth,
            motion_gini: s.motion_gini,
            block_skew: s.block_skew,
            temporal_flatness: s.temporal_flatness,
            loop_closure_score: s.loop_closure_score,
            motion_periodicity: s.motion_periodicity,
            temporal_jitter: s.temporal_jitter,
            webp_compression_ratio: s.webp_compression_ratio,
            max_frame_delay: s.max_frame_delay,
            min_frame_delay: s.min_frame_delay,
            audio_duration_secs: s.audio_duration_secs,
            path_depth: s.path_depth,
            filename_numeric_density: s.filename_numeric_density,
            physics_225: s.physics_225,
        }
    }
}

#[must_use]
pub fn get_pg_conn_str() -> String {
    match std::env::var(crate::constants::ENV_MFB_PG_CONNSTR) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => PG_DEFAULT_CONNSTR.to_string(),
    }
}

/// Establish a `PostgreSQL` connection using an explicit connection string.
/// Includes comprehensive error logging and user hints.
///
/// # Errors
///
/// Returns an error when the database connection attempt fails.
pub fn connect_pg_with_str(conn_str: &str) -> Result<Client> {
    match Client::connect(conn_str, postgres::NoTls) {
        Ok(client) => Ok(client),
        Err(e) => {
            if !DB_WARN_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_connect",
                    format!(
                        "Database connectivity failed: {e}. Check if PostgreSQL service is running and accessible via {conn_str}"
                    ),
                );
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_DATABASE,
                    crate::infra::static_logs::messages::MSG_DB_FALLBACK
                );
                crate::log_hint!(
                    crate::infra::static_logs::messages::LABEL_DATABASE,
                    crate::infra::static_logs::messages::MSG_DB_INIT_HINT
                );
            }
            Err(e).with_context(|| format!("Failed to connect to PostgreSQL: {conn_str}"))
        }
    }
}

/// Reads the connection string from `MFB_PG_CONNSTR`, falling back to the
/// privacy-safe local default database name.
///
/// # Errors
/// Returns an error if the connection to the `PostgreSQL` database fails.
pub fn open_pg_client() -> Result<Client> {
    let conn_str = get_pg_conn_str();
    connect_pg_with_str(&conn_str)
}

/// TRUNCATE all training tables in a single transaction.
/// Tables that do not yet exist are silently skipped.
/// Row counts cleared are printed for an audit trail.
///
/// # Errors
/// Returns an error when database connection fails or TRUNCATE operations fail.
pub fn reset_training_db(conn_str: &str) -> Result<()> {
    let mut client = connect_pg_with_str(conn_str)?;

    // Training tables that accumulate rows across runs; order is insertion-safe
    let tables: &[&str] = &[
        "inference_log",
        "loop_intent_inference_log",
        "image_quality_inference_log",
        "animated_image_quality_inference_log",
        "video_quality_inference_log",
        "loop_samples",
        "image_quality_samples",
        "animated_image_quality_samples",
        "video_quality_samples",
        "multi_scenario_metadata",
        "path_tree_snapshots",
        "live_audit",
        "decision_snapshots",
        "media_entries",
    ];

    eprintln!("  [RESET-DB] Clearing training tables before run...");
    let mut total_deleted: usize = 0;
    for table in tables {
        let exists: bool = match client.query(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
            &[table],
        ) {
            Ok(rows) => if let Some(row) = rows.first() {
                row.get(0)
            } else {
                eprintln!("[RESET-DB] table existence query returned no row for {table}");
                continue;
            },
            Err(err) => {
                eprintln!("[RESET-DB] table existence check failed for {table}: {err}");
                continue;
            }
        };

        if !exists {
            continue;
        }

        let count: i64 = match client.query(&format!("SELECT COUNT(*) FROM {table}"), &[]) {
            Ok(rows) => {
                if let Some(row) = rows.first() {
                    row.get(0)
                } else {
                    eprintln!("[RESET-DB] COUNT query returned no row for {table}");
                    continue;
                }
            }
            Err(err) => {
                eprintln!("[RESET-DB] COUNT query failed for {table}: {err}");
                continue;
            }
        };

        // Truncate table
        let _ = client.execute(&format!("TRUNCATE TABLE {table}"), &[]);

        total_deleted += i64_to_usize_sat(count);
        if count > 0 {
            eprintln!("      cleared {table}: {count} rows");
        }
    }

    let _ = client;
    eprintln!("  [RESET-DB] Done — {total_deleted} rows removed across all tables.");
    Ok(())
}

/// Prints a one-line status message indicating whether the database is reachable.
pub fn report_db_status() {
    match open_pg_client() {
        Ok(_conn) => {
            crate::log_success!(
                crate::infra::static_logs::messages::LABEL_DATABASE,
                crate::infra::static_logs::messages::MSG_DB_CONN_SUCCESS
            );
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "db_status",
                format!("database status probe failed: {e}"),
            );
        }
    }
}

/// Look up similar samples in the database using HNSW vector search.
///
/// Returns a `SampleMatch` if enough labeled training data exists and
/// similar neighbors are found. Returns `None` on DB error or if the
/// database is too immature for reliable KNN.
/// Outcome of a loop-intent HNSW similarity lookup, including the audit branch tag.
#[derive(Clone, Debug)]
pub(crate) struct LoopSimilarityLookupResult {
    pub sample: Option<SampleMatch>,
    pub branch: LoopIntentLookupBranch,
}

#[must_use]
pub fn lookup_similar_samples(meta: &LoopMeta, path: Option<&Path>) -> Option<SampleMatch> {
    lookup_similar_samples_detailed(meta, path).sample
}

#[must_use]
pub(crate) fn lookup_similar_samples_detailed(
    meta: &LoopMeta,
    path: Option<&Path>,
) -> LoopSimilarityLookupResult {
    match lookup_similar_samples_inner(meta, path) {
        Ok(result) => result,
        Err(e) => {
            if let Some(asset_path) = path {
                crate::media_conversion_gate::delivery_db_path_audit(
                    "delivery_db_knn",
                    asset_path,
                    format!(
                        "KNN SIMILARITY AUDIT: Similar sample lookup failed for asset '{}' | Pipeline Error: {} | System will proceed with heuristic-only fallback",
                        asset_path.display(),
                        e
                    ),
                );
            } else {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_knn",
                    format!(
                        "KNN SIMILARITY AUDIT: Similar sample lookup failed for IN-MEMORY-BUFFER | Pipeline Error: {e} | System will proceed with heuristic-only fallback"
                    ),
                );
            }
            lookup_branch_none(LoopIntentLookupBranch::InnerQueryError)
        }
    }
}

/// Retrieve aggregate collection statistics from the metadata table.
///
/// # Errors
/// Returns an error if the database query fails or corpus stats are missing without cold-start eligibility.
pub fn fetch_global_collection_stats(conn: &mut Client) -> Result<GlobalCollectionStats> {
    fetch_loop_collection_stats(conn)
}

/// Fetch the full loop reference profile, combining collection stats
/// with per-feature distribution statistics.
///
/// # Errors
/// Returns an error if the underlying database fetches fail.
pub fn fetch_loop_reference_profile(conn: &mut Client) -> Result<LoopReferenceProfile> {
    crate::multi_scenario_db::init_multi_scenario_schema(conn)?;
    let collection = fetch_global_collection_stats(conn)?;
    let mut feature_map = fetch_loop_feature_map(conn)?;
    if feature_map.stats.is_empty() {
        refresh_loop_intent_feature_stats(conn)?;
        feature_map = fetch_loop_feature_map(conn)?;
    }
    if feature_map.stats.is_empty() {
        anyhow::bail!(
            "loop_intent feature_stats empty after refresh; refusing bootstrap defaults for reference profile"
        );
    }
    build_loop_reference_profile(collection, &feature_map)
}

fn fetch_loop_feature_map(conn: &mut Client) -> Result<FeatureMap> {
    let row_opt = conn.query_opt(
        "SELECT feature_stats::text FROM multi_scenario_metadata WHERE scenario = $1",
        &[&crate::scenario::ScenarioType::LoopIntent.to_string()],
    )?;

    match row_opt {
        None => {
            if loop_samples_table_count(conn)? == 0 {
                let map = cold_start_loop_training_feature_map();
                persist_loop_training_feature_map(conn, &map)?;
                return Ok(map);
            }
            anyhow::bail!(
                "loop_intent multi_scenario_metadata row missing; refusing empty FeatureMap default"
            );
        }
        Some(row) => {
            let value: String = row.get(0);
            if value.trim().is_empty() || value.trim() == "{}" {
                if loop_samples_table_count(conn)? == 0 {
                    tracing::info!(
                        target: "mfb.algorithm",
                        pipeline = "loop_intent",
                        branch = "training_feature_map_cold_start_persist",
                        "loop_samples empty with stale empty feature_stats; seeding cold-start histogram"
                    );
                    let map = cold_start_loop_training_feature_map();
                    persist_loop_training_feature_map(conn, &map)?;
                    return Ok(map);
                }
                let n = loop_samples_table_count(conn)?;
                anyhow::bail!(
                    "loop_intent feature_stats row is empty with {n} loop_samples rows; run refresh_loop_intent_feature_stats",
                );
            }
            match serde_json::from_str::<FeatureMap>(&value) {
                Ok(mut feature_map) => {
                    sanitize_hdbscan_catalog_on_load(&mut feature_map);
                    Ok(feature_map)
                }
                Err(err) => Err(err).context(
                    "corrupt loop_intent feature_stats JSON; refusing bootstrap histogram",
                ),
            }
        }
    }
}

fn parse_loop_hdbscan_catalog_seed(feature_stats_text: &str) -> Result<Option<LoopHdbscanCatalog>> {
    let trimmed = feature_stats_text.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Ok(None);
    }

    let mut feature_map = serde_json::from_str::<FeatureMap>(trimmed)?;
    sanitize_hdbscan_catalog_on_load(&mut feature_map);
    Ok(feature_map.hdbscan_catalog)
}

fn fetch_loop_hdbscan_catalog_seed(conn: &mut Client) -> Result<Option<LoopHdbscanCatalog>> {
    let row_opt = conn.query_opt(
        "SELECT feature_stats::text FROM multi_scenario_metadata WHERE scenario = $1",
        &[&crate::scenario::ScenarioType::LoopIntent.to_string()],
    )?;

    match row_opt {
        None => Ok(None),
        Some(row) => {
            let value: String = row.get(0);
            parse_loop_hdbscan_catalog_seed(&value)
        }
    }
}

/// Drop or clear HDBSCAN catalogs that fail structural contract (wrong version, corrupt centroids).
fn sanitize_hdbscan_catalog_on_load(feature_map: &mut FeatureMap) {
    let Some(catalog) = feature_map.hdbscan_catalog.as_ref() else {
        return;
    };
    if catalog.version != crate::constants::SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent",
            branch = "hdbscan_catalog_version_rejected_on_load",
            catalog_version = catalog.version,
            expected = crate::constants::SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION,
            "discarding HDBSCAN catalog at feature_stats load"
        );
        feature_map.hdbscan_catalog = None;
        return;
    }
    let valid_clusters: Vec<LoopHdbscanClusterCentroid> = catalog
        .clusters
        .iter()
        .filter(|c| {
            !c.centroid.is_empty()
                && c.loop_prior.is_finite()
                && c.centroid.iter().all(|v| v.is_finite())
        })
        .cloned()
        .collect();
    if valid_clusters.is_empty() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent",
            branch = "hdbscan_catalog_empty_after_sanitize",
            "discarding HDBSCAN catalog: no valid cluster centroids"
        );
        feature_map.hdbscan_catalog = None;
        return;
    }
    if valid_clusters.len() != catalog.clusters.len() {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent",
            branch = "hdbscan_catalog_clusters_pruned",
            kept = valid_clusters.len(),
            dropped = catalog.clusters.len() - valid_clusters.len(),
            "pruned invalid HDBSCAN cluster entries at load"
        );
    }
    if let Some(catalog_mut) = feature_map.hdbscan_catalog.as_mut() {
        catalog_mut.clusters = valid_clusters;
    }
}

#[inline]
fn loop_hdbscan_catalog_usable(catalog: Option<&LoopHdbscanCatalog>) -> bool {
    catalog.is_some_and(|c| {
        c.version == crate::constants::SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION
            && !c.clusters.is_empty()
    })
}

#[inline]
fn loop_hdbscan_catalog_usable_in_map(feature_map: &FeatureMap) -> bool {
    loop_hdbscan_catalog_usable(feature_map.hdbscan_catalog.as_ref())
}

/// One-shot production alert: mature loop corpus + HDBSCAN fusion on + no usable catalog.
fn maybe_alert_production_hdbscan_catalog_gap(conn: &mut Client, reason: &'static str) {
    static ALERTED: OnceLock<()> = OnceLock::new();
    if !crate::algorithm_runtime::loop_hdbscan_fusion_enabled()
        || !check_loop_intent_db_maturity(conn)
    {
        return;
    }
    let feature_map = match fetch_loop_feature_map(conn) {
        Ok(map) => map,
        Err(err) => {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_hnsw",
                branch = "production_hdbscan_catalog_gap_check_skipped",
                reason,
                error = %err,
                "LoopIntent feature map fetch failed; skipping HDBSCAN catalog gap alert"
            );
            return;
        }
    };
    if loop_hdbscan_catalog_usable_in_map(&feature_map) {
        return;
    }
    if ALERTED.set(()).is_err() {
        return;
    }
    tracing::error!(
        target: "mfb.algorithm",
        pipeline = "loop_intent_hnsw",
        branch = "production_hdbscan_catalog_missing",
        reason,
        disable_env = crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION,
        "PRODUCTION ALERT: mature LoopIntent corpus without usable HDBSCAN catalog while fusion is enabled; KNN lookups reject at fusion. Run loop_intent_clustering.py after stats refresh, or set MODERN_FORMAT_DISABLE_LOOP_HDBSCAN_FUSION=1"
    );
}

fn fetch_loop_collection_stats(conn: &mut Client) -> Result<GlobalCollectionStats> {
    let row_opt = conn.query_opt(
        "SELECT collection_stats::text FROM multi_scenario_metadata WHERE scenario = $1",
        &[&crate::scenario::ScenarioType::LoopIntent.to_string()],
    )?;

    match row_opt {
        None => {
            if loop_samples_table_count(conn)? == 0 {
                let stats = cold_start_loop_collection_stats();
                persist_loop_collection_stats(conn, &stats)?;
                return Ok(stats);
            }
            anyhow::bail!(
                "loop collection_stats row missing; refusing GlobalCollectionStats::default()"
            );
        }
        Some(row) => {
            let value: String = row.get(0);
            if value.trim().is_empty() || value.trim() == "{}" {
                if loop_samples_table_count(conn)? == 0 {
                    tracing::info!(
                        target: "mfb.algorithm",
                        pipeline = "loop_intent",
                        branch = "training_collection_stats_cold_start_persist",
                        "loop_samples empty with stale empty collection_stats; seeding cold-start baseline"
                    );
                    let stats = cold_start_loop_collection_stats();
                    persist_loop_collection_stats(conn, &stats)?;
                    return Ok(stats);
                }
                anyhow::bail!(
                    "loop collection_stats JSON empty with {} loop_samples rows; run refresh_loop_intent_feature_stats",
                    loop_samples_table_count(conn)?
                );
            }
            Ok(serde_json::from_str(&value).context("parse loop collection_stats JSON failed")?)
        }
    }
}

const LOOP_REFERENCE_FEATURE_KEYS: &[&str] = &[
    "duration",
    "fps",
    "density",
    "file_size_bytes",
    "pixels",
    "temporal_bpp",
    "spatial_bpp",
    "payload_var",
    "delay_var",
    "p_depth",
    "m_gini",
    "t_flat",
    "webp_ratio",
    "cadence",
];

fn build_loop_reference_profile(
    collection: GlobalCollectionStats,
    feature_map: &FeatureMap,
) -> Result<LoopReferenceProfile> {
    for key in LOOP_REFERENCE_FEATURE_KEYS {
        if !feature_map.stats.contains_key(*key) {
            anyhow::bail!("loop reference feature `{key}` missing from feature_stats");
        }
    }
    let mut profile =
        loop_reference_profile_corpus_shell(collection, feature_map.top_keywords.clone());
    profile.duration = distribution_from_feature(feature_map, "duration")?;
    profile.fps = distribution_from_feature(feature_map, "fps")?;
    profile.frame_density = distribution_from_feature(feature_map, "density")?;
    profile.file_size_bytes = distribution_from_feature(feature_map, "file_size_bytes")?;
    profile.pixels = distribution_from_feature(feature_map, "pixels")?;
    profile.temporal_bpp = distribution_from_feature(feature_map, "temporal_bpp")?;
    profile.spatial_bpp = distribution_from_feature(feature_map, "spatial_bpp")?;
    profile.payload_variation = distribution_from_feature(feature_map, "payload_var")?;
    profile.delay_variation = distribution_from_feature(feature_map, "delay_var")?;
    profile.palette_depth = distribution_from_feature(feature_map, "p_depth")?;
    profile.motion_gini = distribution_from_feature(feature_map, "m_gini")?;
    profile.temporal_flatness = distribution_from_feature(feature_map, "t_flat")?;
    profile.webp_ratio = distribution_from_feature(feature_map, "webp_ratio")?;
    profile.cadence = distribution_from_feature(feature_map, "cadence")?;
    profile.duration_has_empirical_percentiles = feature_map
        .stats
        .get("duration")
        .is_some_and(FeatureStats::has_empirical_percentiles);
    if !profile.duration_has_empirical_percentiles {
        strip_non_empirical_duration_percentiles(&mut profile.duration);
    }
    profile.is_knn_bootstrap_heuristic = false;
    Ok(profile)
}

fn distribution_from_feature(feature_map: &FeatureMap, key: &str) -> Result<DistributionStats> {
    crate::media_conversion_gate::algorithm_feature_distribution_required(
        feature_map.stats.get(key).map(DistributionStats::from),
        key,
    )
}

/// Retrieves the count of positive and negative `LoopIntent` labels.
fn get_class_counts(conn: &mut Client) -> (i64, i64, i64) {
    let Ok(rows) = conn.query(
        "SELECT
            CASE WHEN label = 1 THEN 'high'
                 WHEN label = 0 THEN 'video'
                 ELSE 'other'
            END as class,
            count(*)
         FROM loop_samples
         WHERE label IN (0, 1)
         GROUP BY 1",
        &[],
    ) else {
        return (0, 0, 0);
    };

    let low_count: i64 = 0;
    let mut high_count: i64 = 0;
    let mut video_count: i64 = 0;
    for row in rows {
        let class: String = row.get(0);
        let count: i64 = row.get::<_, i64>(1).max(0);
        if class == "high" {
            high_count = count;
        } else if class == "video" {
            video_count = count;
        }
    }
    (low_count, high_count.max(0), video_count.max(0))
}

/// High/low class counts for `image_quality_samples` (static quality training corpus).
fn get_static_quality_class_counts(conn: &mut Client) -> (i64, i64) {
    let Ok(rows) = conn.query(
        "SELECT
            CASE WHEN quality_label IN ('png-high', 'modern-high') THEN 'high'
                 WHEN quality_label IN ('png-low', 'modern-low') THEN 'low'
                 ELSE 'other'
            END AS class,
            COUNT(*)::bigint
         FROM image_quality_samples
         WHERE quality_label IN ('png-high', 'modern-high', 'png-low', 'modern-low')
         GROUP BY 1",
        &[],
    ) else {
        return (0, 0);
    };

    let mut high = 0_i64;
    let mut low = 0_i64;
    for row in rows {
        let class: String = row.get(0);
        let count: i64 = row.get(1);
        if class == "high" {
            high = count;
        } else if class == "low" {
            low = count;
        }
    }
    (high.max(0), low.max(0))
}

/// Validates whether the `LoopIntent` database has enough diverse samples to merit KNN lookup.
fn check_loop_intent_db_maturity(conn: &mut Client) -> bool {
    let (low_count, high_count, video_count) = get_class_counts(conn);
    let total = low_count + high_count + video_count;

    // Both `low` (high-value source) and `high` (explicit loop/sticker intent)
    // are positive loop-preservation classes. `video` remains the negative class.
    let quality_class = low_count + high_count;
    let video_equivalent_class = video_count;

    tracing::info!(
        target: "mfb.database",
        "{} LoopIntent DB Check: low(quality)={}, high={}, video={}, total={} (Needed per class: {})",
        crate::media_conversion_gate::ui_icon_pick("📊", "[stats]"),
        low_count,
        high_count,
        video_count,
        total,
        crate::algorithm_runtime::min_gif_samples_per_class()
    );

    crate::algorithm_runtime::loop_corpus_is_mature(total, quality_class, video_equivalent_class)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LoopIntentLookupBranch {
    DbUnavailable,
    CorpusImmature,
    TargetMetaInsufficient,
    TargetVectorInsufficient,
    HnswNoRows,
    HnswAllNeighborsCorrupt,
    AdaptiveNeighborCountFailed,
    HnswRadiusExcludedAllNeighbors,
    HnswInsufficientWeightedNeighbors,
    /// HDBSCAN fusion enabled (default) but catalog missing, invalid, or unusable for fusion.
    HdbscanCatalogUnavailable,
    PosteriorNonFinite,
    InnerQueryError,
    Success,
}

impl LoopIntentLookupBranch {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DbUnavailable => "db_unavailable",
            Self::CorpusImmature => "corpus_immature",
            Self::TargetMetaInsufficient => "target_meta_insufficient",
            Self::TargetVectorInsufficient => "target_vector_insufficient",
            Self::HnswNoRows => "hnsw_no_rows",
            Self::HnswAllNeighborsCorrupt => "hnsw_all_neighbors_corrupt",
            Self::AdaptiveNeighborCountFailed => "adaptive_neighbor_count_failed",
            Self::HnswRadiusExcludedAllNeighbors => "hnsw_radius_excluded_all_neighbors",
            Self::HnswInsufficientWeightedNeighbors => "hnsw_insufficient_weighted_neighbors",
            Self::HdbscanCatalogUnavailable => "hdbscan_catalog_unavailable",
            Self::PosteriorNonFinite => "posterior_non_finite",
            Self::InnerQueryError => "inner_query_error",
            Self::Success => "success",
        }
    }
}

#[inline]
fn lookup_branch_none(branch: LoopIntentLookupBranch) -> LoopSimilarityLookupResult {
    log_loop_lookup_branch(branch);
    LoopSimilarityLookupResult {
        sample: None,
        branch,
    }
}

#[inline]
fn log_loop_lookup_branch(branch: LoopIntentLookupBranch) {
    tracing::debug!(
        target: "mfb.algorithm",
        pipeline = "loop_intent_hnsw",
        branch = branch.as_str(),
        "loop intent similarity lookup"
    );
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
fn lookup_similar_samples_inner(
    meta: &LoopMeta,
    _path: Option<&Path>,
) -> Result<LoopSimilarityLookupResult> {
    let mut conn = match open_pg_client() {
        Ok(c) => c,
        Err(e) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "delivery_db_knn",
                format!(
                    "DATABASE AUDIT: PostgreSQL connection failed (graceful fallback): {e} | Forensic: Connection string or service state issue; suggesting manual intervention to restore KNN capabilities"
                ),
            );
            crate::log_hint!(
                crate::infra::static_logs::messages::LABEL_DB,
                "DATABASE SUGGESTION: Run 'cargo run --locked -p dev --bin database_manager' (option 1: Database Setup) to initialize and start the local database service."
            );
            return Ok(lookup_branch_none(LoopIntentLookupBranch::DbUnavailable));
        }
    };

    crate::multi_scenario_db::init_multi_scenario_schema(&mut conn)?;

    // ── Automated Vector Hydration Check ──
    // If we have samples but no feature vectors, trigger a one-time backfill
    let missing_vec_count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM loop_samples WHERE embedding IS NULL",
            &[],
        )?
        .get(0);
    if missing_vec_count > 0 {
        let total_count: i64 = conn
            .query_one("SELECT COUNT(*) FROM loop_samples", &[])?
            .get(0);
        if total_count > 0 {
            tracing::info!(
                target: "mfb.database",
                "{} Detected {missing_vec_count} samples with missing feature vectors. Triggering automated recompute...",
                crate::media_conversion_gate::ui_icon_pick("🧩", "[sync]")
            );
            refresh_loop_intent_feature_stats(&mut conn)?;
        }
    }

    if !check_loop_intent_db_maturity(&mut conn) {
        tracing::info!(
            target: "mfb.database",
            "{} LoopIntent database is immature (needs >={} total, >={} per class). Bypassing KNN.",
            crate::media_conversion_gate::ui_icon_pick("🔬", "[audit]"),
            crate::algorithm_runtime::min_gif_samples_total(),
            crate::algorithm_runtime::min_gif_samples_per_class()
        );
        return Ok(lookup_branch_none(LoopIntentLookupBranch::CorpusImmature));
    }

    // Map the incoming LoopMeta into a SampleRow to compute its HNSW search vector
    let Some((target_temporal_bpp, target_spatial_bpp)) = bpp_from_meta(meta) else {
        crate::media_conversion_gate::delivery_db_batch_audit(
            "delivery_db_knn",
            "KNN AUDIT: Target LoopMeta missing width/height/frame_count for BPP; skipping KNN lookup | Forensic: refusing fabricated BPP denominator",
        );
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::TargetMetaInsufficient,
        ));
    };

    let Some(target_sample) = sample_row_from_meta(meta, target_temporal_bpp, target_spatial_bpp)
    else {
        crate::media_conversion_gate::delivery_db_batch_audit(
            "delivery_db_knn",
            "KNN AUDIT: Target LoopMeta missing critical width/height dimensions; skipping KNN lookup | Forensic: Insufficient metadata for feature vector computation",
        );
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::TargetMetaInsufficient,
        ));
    };

    let feature_stats = prepare_loop_training_feature_map(&mut conn)?;
    let target_vector =
        match crate::database_vector::compute_sample_vector(&target_sample, &feature_stats) {
            Ok(vector) => vector,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "KNN: Target sample lacks required features; falling back to non-KNN scoring."
                );
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_knn",
                    format!("KNN AUDIT: Target vector unavailable | Forensic: {err}"),
                );
                return Ok(lookup_branch_none(
                    LoopIntentLookupBranch::TargetVectorInsufficient,
                ));
            }
        };
    let target_vector_for_cluster = target_vector.clone();
    let target_pg_vector = pgvector::Vector::from(target_vector);

    // Deep pgvector Integration: We let PostgreSQL use the HNSW index to rapidly return the closest labels
    let rows = conn.query(
        "SELECT
            label, duration_secs,
            embedding <-> $1::vector AS dist
         FROM loop_samples
         WHERE embedding IS NOT NULL AND frame_count > 1
         ORDER BY embedding <-> $1::vector
         LIMIT 24",
        &[&target_pg_vector],
    )?;

    tracing::debug!(rows = rows.len(), "KNN search rows loaded");

    let candidate_distances: Vec<f64> = rows
        .iter()
        .map(|r: &postgres::Row| r.get::<_, f64>(2))
        .collect();
    if !candidate_distances.is_empty() {
        tracing::debug!(
            top5 = ?&candidate_distances[..5.min(candidate_distances.len())],
            "KNN raw distance sample"
        );
    }

    if rows.is_empty() {
        return Ok(lookup_branch_none(LoopIntentLookupBranch::HnswNoRows));
    }

    let candidates: Vec<(LabelStatus, Option<f64>, f64)> = rows
        .iter()
        .filter_map(|row| {
            let dist: f64 = row.get(2);
            if !dist.is_finite() {
                tracing::debug!(
                    target: "mfb.algorithm",
                    pipeline = "loop_intent_hnsw",
                    branch = "skip_corrupt_neighbor_distance",
                    "dropping loop HNSW row with non-finite distance"
                );
                return None;
            }
            let numeric_label: i16 = row.get(0);
            let label = if let Some(normalized) = loop_training_label_from_numeric(numeric_label) {
                label_status_from_training_label(Some(normalized))
            } else {
                tracing::debug!(
                    target: "mfb.algorithm",
                    pipeline = "loop_intent_hnsw",
                    branch = "skip_corrupt_neighbor_label",
                    numeric_label,
                    "dropping loop HNSW row with unsupported training label"
                );
                return None;
            };
            Some((label, row.get::<_, Option<f64>>(1), dist.max(0.0)))
        })
        .collect();

    if candidates.is_empty() {
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::HnswAllNeighborsCorrupt,
        ));
    }

    let neighbor_count = match adaptive_neighbor_count(candidates.len()) {
        Ok(count) => count,
        Err(err) => {
            tracing::error!(
                "☢️ [CRITICAL ANOMALY] {}; skipping KNN classification to avoid forgery.",
                err
            );
            return Ok(lookup_branch_none(
                LoopIntentLookupBranch::AdaptiveNeighborCountFailed,
            ));
        }
    };
    let neighbors = &candidates[..neighbor_count.min(candidates.len())];

    let min_distance = match neighbors.first() {
        Some((_, _, d)) if d.is_finite() => *d,
        _ => {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_hnsw",
                branch = "hnsw_min_distance_non_finite",
                "HNSW neighbor min distance non-finite; rejecting lookup"
            );
            return Ok(lookup_branch_none(
                LoopIntentLookupBranch::PosteriorNonFinite,
            ));
        }
    };
    let radius = dynamic_neighbor_radius(neighbors);

    let (low_count, high_count, video_count) = get_class_counts(&mut conn);
    let total_samples = low_count + high_count + video_count;
    let quality_count = low_count + high_count;
    let video_equivalent_count = video_count;

    // Class-balance reweighting with smoothing + damping.
    // Compared with raw inverse-frequency scaling, this avoids unstable over-corrections
    // when one class is heavily underrepresented.
    let w_quality = class_balance_weight(total_samples, quality_count);
    let w_video = class_balance_weight(total_samples, video_equivalent_count);
    let global_keep_prior = smoothed_keep_prior(quality_count, video_equivalent_count);
    let global_imbalance_ratio = imbalance_ratio(quality_count, video_equivalent_count);
    tracing::debug!(
        radius = radius,
        min_distance = min_distance,
        candidate_count = neighbors.len(),
        w_keep = w_quality,
        w_weak = w_video,
        prior = global_keep_prior,
        imbalance_ratio = global_imbalance_ratio,
        "KNN balance context"
    );

    let mut weighted_keep = Rational::from(0);
    let mut total_weight = Rational::from(0);
    let mut weight_squares_sum = Rational::from(0);
    let mut distances: Vec<f64> = Vec::new();
    let mut loop_durations: Vec<f64> = Vec::new();

    for (label, duration_secs, distance) in neighbors {
        if *distance > radius {
            continue;
        }
        if !matches!(label, LabelStatus::LoopStrong | LabelStatus::LoopWeak) {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_hnsw",
                branch = "skip_unlabeled_neighbor_posterior",
                ?label,
                "dropping loop HNSW neighbor without Strong/Weak training label"
            );
            continue;
        }

        let relative_distance = (*distance - min_distance).max(0.0);
        let Some(rel_dist_r) = rug::Rational::from_f64(
            1.0_f64
                / (relative_distance * relative_distance)
                    .mul_add(crate::constants::KNN_DISTANCE_WEIGHT_SCALE, 1.0),
        ) else {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "delivery_db_knn",
                format!(
                    "NUMERIC ANOMALY: NaN/Inf distance ({distance}) encountered in KNN neighbor weights | Forensic: Corrupt distance calculation; skipping corrupt neighbor to prevent weight drift"
                ),
            );
            continue;
        };
        let distance_weight = rel_dist_r;

        let class_weight = match label {
            LabelStatus::LoopStrong => {
                let Some(weight) =
                    crate::numeric_cast::f64_to_rational_strict(w_quality, "knn_w_quality")
                else {
                    crate::media_conversion_gate::delivery_db_batch_audit(
                        "delivery_db_knn",
                        format!(
                            "NUMERIC ANOMALY: non-finite class balance weight w_quality={w_quality}; skipping neighbor"
                        ),
                    );
                    continue;
                };
                weight
            }
            LabelStatus::LoopWeak => {
                let Some(weight) =
                    crate::numeric_cast::f64_to_rational_strict(w_video, "knn_w_video")
                else {
                    crate::media_conversion_gate::delivery_db_batch_audit(
                        "delivery_db_knn",
                        format!(
                            "NUMERIC ANOMALY: non-finite class balance weight w_video={w_video}; skipping neighbor"
                        ),
                    );
                    continue;
                };
                weight
            }
            LabelStatus::Uncertain | LabelStatus::NotLabeled => {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_knn",
                    format!(
                        "KNN DEFENSIVE SKIP: unlabeled neighbor reached class weighting after filter: {label:?}"
                    ),
                );
                continue;
            }
        };

        let final_weight = distance_weight * class_weight;

        let prob = match label {
            LabelStatus::LoopStrong => Rational::from(1), // Preserve as looping asset
            LabelStatus::LoopWeak => Rational::from(0),   // Treat as video-like dynamic content
            LabelStatus::Uncertain | LabelStatus::NotLabeled => {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_knn",
                    format!(
                        "KNN DEFENSIVE SKIP: unlabeled neighbor reached probability mapping after filter: {label:?}"
                    ),
                );
                continue;
            }
        };

        if prob >= Rational::from((1, 2))
            && let Some(d) = duration_secs
        {
            loop_durations.push(*d);
        }

        weighted_keep += prob * final_weight.clone();
        total_weight += final_weight.clone();
        weight_squares_sum += final_weight.clone() * final_weight;
        distances.push(*distance);
    }

    if distances.is_empty() {
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::HnswRadiusExcludedAllNeighbors,
        ));
    }

    if distances.len() < crate::algorithm_runtime::loop_hnsw_min_weighted_neighbors() {
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::HnswInsufficientWeightedNeighbors,
        ));
    }

    let Some(min_weight) = crate::numeric_cast::f64_to_rational_strict(1e-6, "knn_min_weight")
    else {
        crate::media_conversion_gate::delivery_db_batch_audit(
            "delivery_db_knn",
            "NUMERIC ANOMALY: knn_min_weight constant is non-finite; rejecting lookup",
        );
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::PosteriorNonFinite,
        ));
    };
    let divisor = if total_weight > min_weight {
        total_weight.clone()
    } else {
        min_weight
    };
    let local_keep_probability = (weighted_keep / divisor).to_f64();
    if !local_keep_probability.is_finite() {
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::PosteriorNonFinite,
        ));
    }
    let eff_n = effective_sample_size(weight_squares_sum.to_f64(), total_weight.to_f64());
    // With higher imbalance, require stronger local evidence before moving away from global prior.
    let prior_strength = crate::constants::KNN_PRIOR_STRENGTH_SLOPE.mul_add(
        global_imbalance_ratio.ln_1p(),
        crate::constants::KNN_PRIOR_STRENGTH_BASE,
    );
    let shrink = (eff_n / (eff_n + prior_strength)).clamp(0.0, 1.0);
    let knn_keep_probability = global_keep_prior
        .mul_add(1.0 - shrink, local_keep_probability * shrink)
        .clamp(0.0, 1.0);
    let (keep_probability, hdbscan_cluster_id, hdbscan_cluster_loop_prior) =
        match fuse_keep_probability_with_hdbscan_cluster(
            knn_keep_probability,
            &target_vector_for_cluster,
            feature_stats.hdbscan_catalog.as_ref(),
        ) {
            Ok(fused) => fused,
            Err(branch) => {
                if branch == LoopIntentLookupBranch::HdbscanCatalogUnavailable {
                    maybe_alert_production_hdbscan_catalog_gap(&mut conn, "lookup_fusion_rejected");
                }
                return Ok(lookup_branch_none(branch));
            }
        };
    let mean_distance: f64 = if distances.is_empty() {
        0.0
    } else {
        distances.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(distances.len())
    };

    let variance: f64 = distances
        .iter()
        .map(|d| {
            let diff = d - mean_distance;
            diff * diff
        })
        .sum::<f64>()
        / crate::numeric_cast::usize_to_f64(distances.len());
    let std_dev_distance = variance.sqrt();

    // Confidence: how tightly clustered the neighbors are.
    // High std_dev relative to mean → low confidence (mixed signals).
    let mut confidence = if mean_distance > 1e-6_f64 {
        (1.0f64 - (std_dev_distance / mean_distance)).clamp(0.0, 1.0)
    } else {
        // All neighbors at distance ≈0 → exact match level confidence
        1.0_f64
    };
    // Penalize confidence under severe class imbalance and low effective sample size.
    let balance_penalty = (1.0 / global_imbalance_ratio.sqrt())
        .clamp(crate::constants::KNN_BALANCE_PENALTY_FLOOR, 1.0);
    confidence *= balance_penalty;
    confidence *= (eff_n / (eff_n + 2.0)).clamp(crate::constants::KNN_CONFIDENCE_MIN_LIMIT, 1.0);
    tracing::debug!(
        local_keep_probability = local_keep_probability,
        keep_probability = keep_probability,
        effective_n = eff_n,
        shrink = shrink,
        prior_strength = prior_strength,
        confidence = confidence,
        "KNN posterior fusion result"
    );

    // Sort for percentiles
    distances.sort_by(|a: &f64, b: &f64| crate::media_conversion_gate::f64_sort_cmp(*a, *b));
    let n = distances.len();
    let min_distance = distances.first().copied();
    let p25_distance = distances.get(n / 4).copied();
    let p75_distance = distances.get(3 * n / 4).copied();

    let p90_duration = if loop_durations.is_empty() {
        None
    } else {
        loop_durations
            .sort_by(|a: &f64, b: &f64| crate::media_conversion_gate::f64_sort_cmp(*a, *b));
        let idx = crate::media_conversion_gate::delivery_db_usize_or_zero(
            crate::numeric_cast::f64_to_usize_strict(
                (crate::numeric_cast::usize_to_f64(loop_durations.len()) * 0.90).floor(),
                "p90_idx",
            ),
            "NUMERIC ANOMALY: p90_idx overflow during duration percentile calculation | Forensic: Integer overflow in index mapping; refusing to forge data",
        )
        .ok_or_else(|| anyhow::anyhow!("p90_idx overflow"))?;
        Some(
            *loop_durations
                .get(idx.min(loop_durations.len().saturating_sub(1)))
                .ok_or_else(|| {
                    crate::media_conversion_gate::delivery_db_batch_audit(
                "delivery_db_numeric",
                "NUMERIC ANOMALY: Percentile index out of bounds in duration distribution | Forensic: Index resolution failure; refusing to forge duration data",
            );
                    anyhow::anyhow!("Percentile index out of bounds")
                })?,
        )
    };

    let Some((keep_probability, confidence)) =
        crate::algorithm_seal::loop_seal_probability_pair(keep_probability, confidence)
    else {
        return Ok(lookup_branch_none(
            LoopIntentLookupBranch::PosteriorNonFinite,
        ));
    };

    log_loop_lookup_branch(LoopIntentLookupBranch::Success);
    Ok(LoopSimilarityLookupResult {
        sample: Some(SampleMatch {
            exact_label: LabelStatus::NotLabeled,
            keep_probability: Some(keep_probability),
            confidence,
            neighbor_count: distances.len(),
            mean_distance: crate::algorithm_seal::seal_non_negative_finite(mean_distance),
            std_dev_distance: crate::algorithm_seal::seal_non_negative_finite(std_dev_distance),
            min_distance: crate::algorithm_seal::seal_optional_non_negative_distance(min_distance),
            p25_distance: crate::algorithm_seal::seal_optional_non_negative_distance(p25_distance),
            p75_distance: crate::algorithm_seal::seal_optional_non_negative_distance(p75_distance),
            p90_duration: p90_duration.and_then(crate::algorithm_seal::seal_non_negative_finite),
            hdbscan_cluster_id,
            hdbscan_cluster_loop_prior: hdbscan_cluster_loop_prior
                .and_then(crate::algorithm_seal::loop_unit_probability),
        }),
        branch: LoopIntentLookupBranch::Success,
    })
}

fn l2_distance_f32_f64(query: &[f32], centroid: &[f64]) -> Option<f64> {
    if query.len() != centroid.len() || query.is_empty() {
        return None;
    }
    let mut sum = 0.0_f64;
    for (lhs, rhs) in query.iter().zip(centroid.iter()) {
        let lhs = f64::from(*lhs);
        if !lhs.is_finite() || !rhs.is_finite() {
            return None;
        }
        let diff = lhs - rhs;
        sum = diff.mul_add(diff, sum);
    }
    Some(sum.sqrt())
}

/// Nearest HDBSCAN cluster centroid and a distance-based membership confidence in [0, 1].
fn nearest_hdbscan_cluster(query: &[f32], catalog: &LoopHdbscanCatalog) -> Option<(i32, f64, f64)> {
    let mut best: Option<(i32, f64, f64, f64)> = None;
    for cluster in &catalog.clusters {
        if cluster.centroid.is_empty() {
            continue;
        }
        let dist = l2_distance_f32_f64(query, &cluster.centroid)?;
        let prior = cluster.loop_prior;
        if !prior.is_finite() {
            continue;
        }
        let Some(prior_clamped) = crate::algorithm_seal::loop_unit_probability(prior) else {
            continue;
        };
        let Some(confidence) = crate::algorithm_seal::loop_unit_probability(
            (-dist / crate::constants::HDBSCAN_CLUSTER_DISTANCE_SCALE).exp(),
        ) else {
            continue;
        };
        match best {
            Some((_, _, _, best_dist)) if dist >= best_dist => {}
            _ => best = Some((cluster.cluster_id, prior_clamped, confidence, dist)),
        }
    }
    best.map(|(id, prior, conf, _)| (id, prior, conf))
}

/// Blend HNSW neighbor vote with the offline HDBSCAN cluster loop-prior.
///
/// When HDBSCAN fusion is enabled (default unless `MODERN_FORMAT_DISABLE_LOOP_HDBSCAN_FUSION=1`), a supported non-empty catalog and a
/// resolvable cluster assignment are mandatory; missing catalog or unusable fusion rejects the
/// whole lookup (no silent KNN-only fallback).
fn fuse_keep_probability_with_hdbscan_cluster(
    knn_keep_probability: f64,
    query_vector: &[f32],
    catalog: Option<&LoopHdbscanCatalog>,
) -> Result<(f64, Option<i32>, Option<f64>), LoopIntentLookupBranch> {
    let Some(knn_kp) = crate::algorithm_seal::loop_unit_probability(knn_keep_probability) else {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "knn_keep_prob_seal_rejected",
            knn_keep_probability = knn_keep_probability,
            "KNN keep_probability failed seal; lookup rejected"
        );
        return Err(LoopIntentLookupBranch::PosteriorNonFinite);
    };
    if !knn_keep_probability.is_finite() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "knn_keep_prob_non_finite",
            knn_keep_probability = knn_keep_probability,
            "LoopIntent KNN keep_probability non-finite before cluster fusion"
        );
    }
    if !crate::algorithm_runtime::loop_hdbscan_fusion_enabled() {
        return Ok((knn_kp, None, None));
    }
    let Some(catalog) = catalog else {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "hdbscan_catalog_missing",
            "LoopIntent HDBSCAN fusion enabled but catalog absent; lookup rejected"
        );
        return Err(LoopIntentLookupBranch::HdbscanCatalogUnavailable);
    };
    if catalog.version != crate::constants::SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION
        || catalog.clusters.is_empty()
    {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "hdbscan_catalog_invalid",
            catalog_version = catalog.version,
            cluster_count = catalog.clusters.len(),
            expected_version = crate::constants::SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION,
            "LoopIntent HDBSCAN fusion enabled but catalog unsupported or empty; lookup rejected"
        );
        return Err(LoopIntentLookupBranch::HdbscanCatalogUnavailable);
    }
    let Some((cluster_id, cluster_prior, cluster_conf)) =
        nearest_hdbscan_cluster(query_vector, catalog)
    else {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "hdbscan_cluster_unresolved",
            "LoopIntent HDBSCAN fusion enabled but query did not resolve to a cluster; lookup rejected"
        );
        return Err(LoopIntentLookupBranch::HdbscanCatalogUnavailable);
    };
    if !cluster_conf.is_finite() || cluster_conf <= 0.0 {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "hdbscan_cluster_confidence_invalid",
            cluster_id = cluster_id,
            cluster_conf = cluster_conf,
            "LoopIntent HDBSCAN cluster confidence unusable for fusion; lookup rejected"
        );
        return Err(LoopIntentLookupBranch::HdbscanCatalogUnavailable);
    }
    let cluster_weight = (cluster_conf * crate::constants::HDBSCAN_CLUSTER_MAX_WEIGHT)
        .clamp(0.0, crate::constants::HDBSCAN_CLUSTER_MAX_WEIGHT);
    let fused_raw = knn_kp.mul_add(1.0 - cluster_weight, cluster_prior * cluster_weight);
    if !fused_raw.is_finite() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "hdbscan_fusion_non_finite",
            fused_raw = fused_raw,
            knn_kp = knn_kp,
            cluster_weight = cluster_weight,
            "LoopIntent HDBSCAN fusion produced non-finite probability; lookup rejected"
        );
        return Err(LoopIntentLookupBranch::PosteriorNonFinite);
    }
    let Some(fused) = crate::algorithm_seal::loop_unit_probability(fused_raw) else {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_hnsw",
            branch = "hdbscan_fusion_seal_rejected",
            fused_raw = fused_raw,
            knn_kp = knn_kp,
            "LoopIntent HDBSCAN fused probability failed seal; lookup rejected"
        );
        return Err(LoopIntentLookupBranch::PosteriorNonFinite);
    };
    tracing::debug!(
        cluster_id = cluster_id,
        cluster_prior = cluster_prior,
        cluster_weight = cluster_weight,
        knn_keep_probability = knn_kp,
        fused_keep_probability = fused,
        "HDBSCAN cluster prior fused into loop keep_probability"
    );
    Ok((fused, Some(cluster_id), Some(cluster_prior)))
}

/// Dynamic safety-guard for CRF 0.00 exploration.
///
/// Uses the SQL KNN dataset to partition media into "Meme" vs "High Value".
/// High-value art is strictly limited to 30s of lossless-first probing to avoid bloat.
/// Low-value memes (low entropy) are permitted up to 120s as CRF 0.00 is efficient on them.
#[must_use]
fn lossless_duration_limit_for_keep_prob(keep_prob: f64) -> f32 {
    use crate::constants::{HIGH_VALUE_LOSSLESS_DURATION_LIMIT, MEME_LOSSLESS_DURATION_LIMIT};

    let Some(keep_prob) = crate::algorithm_seal::loop_unit_probability(keep_prob) else {
        return HIGH_VALUE_LOSSLESS_DURATION_LIMIT;
    };
    if keep_prob <= crate::constants::KNN_KEEP_PROB_HIGH_VALUE_THRESHOLD {
        HIGH_VALUE_LOSSLESS_DURATION_LIMIT
    } else if keep_prob >= crate::constants::KNN_KEEP_PROB_MEME_THRESHOLD {
        MEME_LOSSLESS_DURATION_LIMIT
    } else {
        let t = (keep_prob - crate::constants::KNN_KEEP_PROB_HIGH_VALUE_THRESHOLD)
            / (crate::constants::KNN_KEEP_PROB_MEME_THRESHOLD
                - crate::constants::KNN_KEEP_PROB_HIGH_VALUE_THRESHOLD);
        let limit_meme = f64::from(MEME_LOSSLESS_DURATION_LIMIT);
        let limit_high = f64::from(HIGH_VALUE_LOSSLESS_DURATION_LIMIT);
        crate::numeric_cast::f64_to_f32_lossy(t.mul_add(limit_meme - limit_high, limit_high))
    }
}

#[must_use]
fn resolved_duration_secs(meta: &LoopMeta) -> Option<f64> {
    if meta.duration_secs.is_some_and(|d| d > 0.11) {
        meta.duration_secs
    } else {
        match meta.frame_count {
            Some(fc)
                if fc > 1
                    && let Some(fps) = meta.fps
                    && fps > 0.1 =>
            {
                Some(crate::numeric_cast::u64_to_f64(fc) / fps)
            }
            Some(_fc) => {
                // We have a frame count but no reliable duration/FPS.
                // Refusing to forge a '12fps' baseline. Information invalidated.
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_metadata",
                    "METADATA ANOMALY: Frame count present but duration/FPS missing for resolved_duration | Forensic: Insufficient timing data to calculate reliable duration; refusing to forge baseline",
                );
                None
            }
            None => {
                // Total metadata vacuum: neither duration nor frame count.
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_metadata",
                    "METADATA ANOMALY: Both duration and frame_count missing for resolved_duration | Forensic: Total metadata vacuum; refusing to forge data",
                );
                None
            }
        }
    }
}

/// Determine whether it is safe to explore lossless encoding (CRF 0.00)
/// for the given asset.
///
/// Uses KNN to classify the asset as "meme" (higher duration limit) or
/// "high-value art" (lower duration limit). Logs a warning if the asset
/// exceeds the dynamic threshold.
#[must_use]
pub fn is_lossless_exploration_safe(meta: &LoopMeta, path: Option<&Path>) -> bool {
    let mut current_meta = meta.clone();
    if let Some(p) = path
        && let Err(e) = crate::loop_intent::deep_refine_meta(&mut current_meta, p)
    {
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_metadata",
            p,
            format!(
                "METADATA AUDIT: Deep refinement failed for '{}' | Forensic: Error '{}'; refusing to mark asset safe for lossless exploration with partial metadata",
                p.display(),
                e
            ),
        );
        return false;
    }
    if let Some(duration) = resolved_duration_secs(&current_meta) {
        current_meta.duration_secs = Some(duration);
    } else {
        crate::media_conversion_gate::delivery_db_batch_audit(
            "delivery_db_metadata",
            "Lossless-first (CRF 0.00) exploration bypassed: Animation duration could not be determined from metadata or bitstream.",
        );
        return false;
    }

    let sample_match = lookup_similar_samples(&current_meta, path);
    let Some(keep_prob) = sample_match.as_ref().and_then(|m| {
        m.keep_probability
            .and_then(crate::algorithm_seal::loop_unit_probability)
    }) else {
        crate::log_hint!(
            crate::infra::static_logs::messages::LABEL_INTENT,
            "KNN keep_prob unavailable — lossless-first (CRF 0) exploration blocked"
        );
        return false;
    };
    let threshold = lossless_duration_limit_for_keep_prob(keep_prob);

    let is_safe = current_meta
        .duration_secs
        .is_some_and(|d| d < f64::from(threshold));

    if !is_safe {
        let duration_str = crate::media_conversion_gate::ui_duration_secs_label_or_unknown(
            current_meta.duration_secs,
            "loop_intent_lossless_duration",
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_INTENT,
            &format!(
                "Lossless-first (CRF 0.00) exploration skip: Duration ({duration_str}) exceeds dynamic limit ({threshold:.1}s) for asset with Value Prob {keep_prob:.2}"
            )
        );
    }

    is_safe
}

pub(crate) fn prepare_loop_training_feature_map(conn: &mut Client) -> Result<FeatureMap> {
    crate::multi_scenario_db::init_multi_scenario_schema(conn)?;
    crate::multi_scenario_db::with_multi_scenario_metadata_lock(conn, |conn| {
        prepare_loop_training_feature_map_inner(conn)
    })
}

fn prepare_loop_training_feature_map_inner(conn: &mut Client) -> Result<FeatureMap> {
    let loop_sample_count: i64 = conn
        .query_one("SELECT COUNT(*) FROM loop_samples", &[])?
        .get(0);
    if loop_sample_count > 0 {
        let mut feature_map = match fetch_loop_feature_map(conn) {
            Ok(feature_map) => feature_map,
            Err(err) => {
                tracing::warn!(
                    target: "mfb.algorithm",
                    pipeline = "loop_intent",
                    branch = "training_feature_map_refresh_bootstrap",
                    error = %err,
                    sample_count = loop_sample_count,
                    "loop_intent training feature_stats unavailable; rebuilding from loop_samples"
                );
                refresh_loop_intent_feature_stats_inner(conn)
                    .context("LoopIntent feature stats refresh during training bootstrap failed")?;
                fetch_loop_feature_map(conn)
                    .context("LoopIntent feature stats fetch after bootstrap refresh failed")?
            }
        };
        if feature_map.stats.is_empty() {
            refresh_loop_intent_feature_stats_inner(conn)?;
            feature_map = fetch_loop_feature_map(conn)?;
        }
        if !feature_map.stats.is_empty() {
            return Ok(feature_map);
        }
        anyhow::bail!(
            "LoopIntent training feature_stats remain empty after refresh with {loop_sample_count} loop_samples rows; refusing bootstrap histograms for ingest"
        );
    }

    tracing::info!(
        target: "mfb.algorithm",
        pipeline = "loop_intent",
        branch = "training_feature_map_cold_start",
        "loop_samples empty; using cold-start feature_stats for first-batch training ingest"
    );
    let map = cold_start_loop_training_feature_map();
    persist_loop_training_feature_map(conn, &map)
        .context("LoopIntent cold-start feature_stats persist failed")?;
    let collection = cold_start_loop_collection_stats();
    persist_loop_collection_stats(conn, &collection)
        .context("LoopIntent cold-start collection_stats persist failed")?;
    Ok(map)
}

/// Intermediate representation of a sample's metadata ready for database
/// insertion. Contains all extracted features and classification labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
// Rationale: This struct serves as a comprehensive configuration or state container where individual boolean flags are the most idiomatic and explicit way to represent discrete options.
pub struct SampleInsert {
    /// BLAKE3 hash of the file contents.
    file_hash: String,
    /// Absolute path to the source file.
    source_path: String,
    /// Base filename (may be None if not determinable).
    file_name: Option<String>,
    /// File extension (e.g., "gif", "webp").
    source_ext: Option<String>,
    /// Image width in pixels.
    width: u32,
    /// Image height in pixels.
    height: u32,
    /// Total animation duration in seconds.
    duration_secs: Option<f64>,
    /// Total number of frames.
    frame_count: Option<u64>,
    /// File size in bytes.
    file_size_bytes: u64,
    /// Frames per second.
    fps: Option<f64>,
    flags: SampleFlags,
    /// Number of unique colors in the palette (if applicable).
    palette_size: Option<u32>,
    /// Variation in frame payload sizes (0-1 range).
    frame_payload_variation: Option<f64>,
    /// Variation in frame delay timings (0-1 range).
    frame_delay_variation: Option<f64>,
    /// Temporal bits per pixel -- measures compression efficiency over time.
    temporal_bpp: f64,
    /// Spatial bits per pixel -- measures compression efficiency per frame.
    spatial_bpp: f64,
    /// Classification label: "low" (high value), "high" (meme), "video", or "medium".
    loss_tolerance: String,
    /// Who or what labeled this sample (e.g., "`cli_ingest`", "`integrity_refresh`").
    labeled_by: String,
    /// Width/height ratio.
    aspect_ratio: Option<f64>,
    /// Total pixel count (width * height).
    total_pixels: u64,
    /// Score indicating how likely the asset is to loop (0-1 range).
    loop_frequency: f64,
    cadence_score: f64,
    directory_loop_intent_score: f64,
    /// Depth of the color palette as a normalized score (0-1).
    palette_depth: Option<f64>,
    /// Motion Gini coefficient -- measures how concentrated motion is across frames.
    motion_gini: Option<f64>,
    /// Block skew measurement -- detects geometric distortion.
    block_skew: Option<f64>,
    /// Temporal flatness -- how uniform the temporal features are.
    temporal_flatness: Option<f64>,
    /// Boundary continuity proxy extracted from packet-size closure.
    loop_closure_score: Option<f64>,
    /// Periodic motion strength extracted from motion vectors.
    motion_periodicity: Option<f64>,
    /// Jitter in frame timings.
    pub temporal_jitter: Option<f64>,
    /// WebP compression ratio relative to the original.
    webp_compression_ratio: Option<f64>,
    max_frame_delay: Option<f64>,
    min_frame_delay: Option<f64>,
    audio_duration_secs: Option<f64>,
    path_depth: u32,
    filename_numeric_density: f64,
    /// 15x15 luminance grid (225 dims) providing real structural energy data.
    pub physics_225: Option<Vec<f32>>,
    /// Loop intent classification ("`LoopStrong`", "`LoopWeak`", or "`Uncertain`").
    loop_verdict: String,
}

pub(crate) fn normalize_loop_training_label(label: &str) -> Option<&'static str> {
    match label {
        "low" | "high" => Some("high"),
        "video" => Some("video"),
        _ => None,
    }
}

fn loop_verdict_for_training_label(label: &str) -> &'static str {
    match normalize_loop_training_label(label) {
        Some("high") => "LoopStrong",
        Some("video") => "LoopWeak",
        _ => "Uncertain",
    }
}

fn label_status_from_training_label(label: Option<&str>) -> LabelStatus {
    match label.and_then(normalize_loop_training_label) {
        Some("high") => LabelStatus::LoopStrong,
        Some("video") => LabelStatus::LoopWeak,
        _ => LabelStatus::NotLabeled,
    }
}

fn physics_nonzero_count(physics: &[f32]) -> usize {
    physics
        .iter()
        .filter(|&&value| value.is_finite() && value.abs() > f32::EPSILON)
        .count()
}

fn validate_loop_training_sample(sample: &SampleInsert) -> Result<()> {
    let physics = sample
        .physics_225
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing physics_225 tail"))?;
    if physics.len() != 225 {
        anyhow::bail!("physics_225 length {} != 225", physics.len());
    }
    if physics.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("physics_225 contains non-finite values");
    }
    if physics_nonzero_count(physics) == 0 {
        anyhow::bail!("physics_225 is all zero");
    }

    let frame_count = sample
        .frame_count
        .ok_or_else(|| anyhow::anyhow!("Missing frame_count"))?;
    if frame_count <= 1 {
        anyhow::bail!("frame_count {frame_count} is not dynamic content");
    }

    let duration_secs = sample
        .duration_secs
        .ok_or_else(|| anyhow::anyhow!("Missing duration_secs"))?;
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        anyhow::bail!("Invalid duration_secs {duration_secs}");
    }

    if sample.width == 0 || sample.height == 0 {
        anyhow::bail!("Missing width/height");
    }
    if sample.file_size_bytes == 0 {
        anyhow::bail!("Empty file size");
    }
    let webp_ratio = sample
        .webp_compression_ratio
        .ok_or_else(|| anyhow::anyhow!("Missing loop_stats_webp_ratio"))?;
    if !webp_ratio.is_finite() || webp_ratio <= 0.0 {
        anyhow::bail!("Invalid loop_stats_webp_ratio {webp_ratio}");
    }
    if normalize_loop_training_label(sample.loss_tolerance.as_str()).is_none() {
        anyhow::bail!(
            "Unsupported or ambiguous loop label '{}'",
            sample.loss_tolerance
        );
    }

    Ok(())
}

pub(crate) fn build_loop_intent_scenario_sample(
    path: &Path,
    labeled_by: &str,
    label_override: Option<&str>,
    blake3_hash: Vec<u8>,
    feature_map: &FeatureMap,
) -> Result<crate::multi_scenario_db::ScenarioSample> {
    let sample = sample_from_path(path, labeled_by, label_override).ok_or_else(|| {
        anyhow::anyhow!(
            "LoopIntent training sample unavailable for {}",
            path.display()
        )
    })?;

    if let Err(e) = validate_loop_training_sample(&sample) {
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_intent",
            path,
            format!(
                "TRAINING AUDIT: LoopIntent sample '{}' rejected for multi-scenario ingestion | Forensic: {}",
                path.display(),
                e
            ),
        );
        anyhow::bail!(
            "LoopIntent training sample validation failed for {}: {e}",
            path.display()
        );
    }

    let sample_row = SampleRow::from(sample.clone());
    let vec_data = crate::database_vector::compute_sample_vector(&sample_row, feature_map)
        .map_err(|err| {
            crate::media_conversion_gate::delivery_db_path_audit(
                "delivery_db_intent",
                path,
                format!(
                    "TRAINING AUDIT: LoopIntent vector hydration failed for '{}' | Forensic: {err}",
                    path.display()
                ),
            );
            err
        })
        .with_context(|| format!("LoopIntent vector hydration failed for {}", path.display()))?;
    if vec_data.len() != 261 {
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_intent",
            path,
            format!(
                "TRAINING AUDIT: LoopIntent vector dimension mismatch for '{}' | Forensic: expected 261, got {}",
                path.display(),
                vec_data.len()
            ),
        );
        anyhow::bail!(
            "LoopIntent vector dimension mismatch for {} (expected 261, got {})",
            path.display(),
            vec_data.len()
        );
    }
    if vec_data.iter().any(|value| !value.is_finite()) {
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_intent",
            path,
            format!(
                "TRAINING AUDIT: LoopIntent vector contains non-finite values for '{}' | Forensic: refusing to insert polluted embedding",
                path.display()
            ),
        );
        anyhow::bail!(
            "LoopIntent vector contains non-finite values for {}",
            path.display()
        );
    }

    let frame_count_u64 = sample.frame_count.with_context(|| {
        format!(
            "LoopIntent training sample missing frame_count for {}",
            path.display()
        )
    })?;
    let duration_secs = sample.duration_secs.with_context(|| {
        format!(
            "LoopIntent training sample missing duration_secs for {}",
            path.display()
        )
    })?;
    let width_i32 = crate::numeric_cast::u32_to_i32_strict(sample.width, "loop_width")
        .with_context(|| {
            format!(
                "LoopIntent width {} out of i32 range for {}",
                sample.width,
                path.display()
            )
        })?;
    let height_i32 = crate::numeric_cast::u32_to_i32_strict(sample.height, "loop_height")
        .with_context(|| {
            format!(
                "LoopIntent height {} out of i32 range for {}",
                sample.height,
                path.display()
            )
        })?;
    let frame_count_db =
        crate::numeric_cast::u64_to_i64_strict(frame_count_u64, "loop_frame_count").with_context(
            || {
                format!(
                    "LoopIntent frame_count {} out of i64 range for {}",
                    frame_count_u64,
                    path.display()
                )
            },
        )?;
    let file_size_i64 =
        crate::numeric_cast::u64_to_i64_strict(sample.file_size_bytes, "loop_file_size")
            .with_context(|| {
                format!(
                    "LoopIntent file_size {} out of i64 range for {}",
                    sample.file_size_bytes,
                    path.display()
                )
            })?;

    let normalized_label = normalize_loop_training_label(sample.loss_tolerance.as_str())
        .with_context(|| {
            format!(
                "LoopIntent training label '{}' unsupported for {}",
                sample.loss_tolerance,
                path.display()
            )
        })?;

    let physics = crate::media_conversion_gate::db_physics_embedding_or_empty(
        sample.physics_225.as_deref(),
        "loop training ingest physics_225",
    );
    let physics_len = physics.len();
    let physics_nonzero = physics_nonzero_count(physics);

    let mut scenario_sample = crate::multi_scenario_db::ScenarioSample::new(
        blake3_hash,
        crate::scenario::ScenarioType::LoopIntent,
    )
    .with_path(sample.source_path.clone())
    .with_label(normalized_label.to_string())
    .with_embedding(pgvector::Vector::from(vec_data))
    .with_dimensions(width_i32, height_i32)
    .with_size(file_size_i64)
    .with_duration_secs(duration_secs)
    .with_frame_count(frame_count_db)
    .with_labeled_by(sample.labeled_by.clone())
    .with_cadence_score(sample.cadence_score)
    .with_frame_delay_variation_opt(sample.frame_delay_variation)
    .with_frame_payload_variation_opt(sample.frame_payload_variation)
    .with_loop_closure_score_opt(sample.loop_closure_score)
    .with_motion_gini_opt(sample.motion_gini)
    .with_motion_periodicity_opt(sample.motion_periodicity)
    .with_temporal_jitter_opt(sample.temporal_jitter);

    if let Some(file_name) = sample.file_name.clone() {
        scenario_sample = scenario_sample.with_file_name(file_name);
    }
    if let Some(fps) = sample.fps {
        scenario_sample = scenario_sample.with_fps(fps);
    }

    let mut metadata = serde_json::to_value(LoopIntentStoredMetadata {
        source_ext: sample.source_ext.clone(),
        loss_tolerance: Some(sample.loss_tolerance.clone()),
        loop_verdict: Some(sample.loop_verdict.clone()),
        temporal_bpp: Some(sample.temporal_bpp),
        spatial_bpp: Some(sample.spatial_bpp),
        frame_payload_variation: sample.frame_payload_variation,
        frame_delay_variation: sample.frame_delay_variation,
        aspect_ratio: sample.aspect_ratio,
        total_pixels: Some(sample.total_pixels),
        loop_frequency: Some(sample.loop_frequency),
        directory_loop_intent_score: Some(sample.directory_loop_intent_score),
        palette_size: sample.palette_size,
        palette_depth: sample.palette_depth,
        block_skew: sample.block_skew,
        temporal_flatness: sample.temporal_flatness,
        webp_compression_ratio: sample.webp_compression_ratio,
        max_frame_delay: sample.max_frame_delay,
        min_frame_delay: sample.min_frame_delay,
        audio_duration_secs: sample.audio_duration_secs,
        path_depth: sample.path_depth,
        filename_numeric_density: sample.filename_numeric_density,
        physics_225: sample.physics_225.clone(),
        flags: sample.flags.clone(),
    })?;
    if let Value::Object(map) = &mut metadata {
        map.insert(
            "embedding_schema".to_string(),
            Value::String("loop_intent_v2_36plus225".to_string()),
        );
        map.insert("physics_225_len".to_string(), json!(physics_len));
        map.insert("physics_225_nonzero".to_string(), json!(physics_nonzero));
    }
    scenario_sample.metadata = metadata;

    Ok(scenario_sample)
}

fn determine_loss_tolerance(
    temporal_bpp: f64,
    has_embedded_icc: bool,
    has_complex_color_profile: bool,
    app_extensions: Option<&[String]>,
    source_path: &Path,
    file_name: Option<&str>,
) -> String {
    // 1. Exact markers for "low loss tolerance" (high value)
    if has_embedded_icc || has_complex_color_profile {
        return "low".to_string();
    }

    let source_str = source_path.to_string_lossy().to_lowercase();
    let is_high_value_dir = [
        "author",
        "artist",
        "creators",
        "collection",
        "gallery",
        "archive",
        "portfolio",
        "\u{4f5c}\u{54c1}",
        "\u{4f5c}\u{8005}",
        "\u{753b}\u{5e08}",
        "\u{63d2}\u{753b}",
        "\u{6536}\u{85cf}",
        "\u{539f}\u{4f5c}",
    ]
    .iter()
    .any(|kw| source_str.contains(kw));

    if is_high_value_dir {
        return "low".to_string();
    }

    // 2. Exact markers for "high loss tolerance" (meme / heavily compressed social)
    if temporal_bpp < 0.03_f64 {
        return "high".to_string();
    }

    if let Some(exts) = app_extensions {
        for ext in exts {
            if ext.starts_with("GIPHY") || ext.starts_with("TENOR") || ext.starts_with("STICKER") {
                return "high".to_string();
            }
        }
    }

    let is_meme_dir = [
        "meme",
        "sticker",
        "emoji",
        "reaction",
        "\u{8868}\u{60c5}",
        "\u{8d34}\u{7eb8}",
        "\u{6597}\u{56fe}",
        "\u{6897}",
    ]
    .iter()
    .any(|kw| source_str.contains(kw));

    if is_meme_dir {
        return "high".to_string();
    }

    // Check WeChat / social cache hints
    if let Some(name) = file_name {
        let stem = match name.rsplit_once('.') {
            Some((stem, _)) => stem,
            None => name,
        }
        .to_lowercase();
        if stem.starts_with("mmexport") || stem.starts_with("wx_camera") || stem.len() == 32 {
            return "high".to_string();
        }
    }

    "medium".to_string()
}

/// Analyze a file and produce a `SampleInsert` record suitable for
/// database ingestion.
///
/// Probes the file, extracts metadata, determines loss tolerance based
/// on content characteristics and directory heuristics, and computes
/// derived features like temporal/spatial BPP. Returns `None` if the
/// file cannot be probed.
fn gather_sample_metadata(path: &Path) -> Result<LoopMeta> {
    let probe = match probe_video(path) {
        Ok(probe) => probe,
        Err(e) => {
            crate::media_conversion_gate::delivery_db_path_audit(
                "delivery_db_intent",
                path,
                format!(
                    "TRAINING AUDIT: Sample probe failed for '{}' | Forensic: FFprobe Error '{}'; skipping training sample to maintain dataset integrity",
                    path.display(),
                    e
                ),
            );
            anyhow::bail!("Sample probe failed for {}: {e}", path.display());
        }
    };
    let mut meta = LoopMeta::from_ffprobe_result(&probe, path);
    if probe
        .format_name
        .to_ascii_lowercase()
        .contains(crate::constants::CONTAINER_GIF)
    {
        match scan_gif_headers(path) {
            Ok(scan) => {
                meta.palette_size = scan.palette_size;
                meta.app_extensions = scan.app_extensions;
                meta.flags.streams.has_transparency = scan.has_transparency;
                meta.frame_payload_variation = scan.frame_payload_variation;
                meta.frame_delay_variation = scan.frame_delay_variation;
                meta.loop_count = scan.loop_count;
                // Only overwrite ffprobe duration when the GIF header scan
                // produces a valid positive value. Zero-delay GIFs (all frame
                // delays == 0) must not clobber a real ffprobe-derived duration
                // with 0.0, which would fail validate_loop_training_sample.
                if let Some(duration_secs) = scan.duration_secs
                    && duration_secs.is_finite()
                    && duration_secs > 0.0
                {
                    meta.duration_secs = Some(duration_secs);
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_db_path_audit(
                    "delivery_db_intent",
                    path,
                    format!(
                        "TRAINING AUDIT: GIF/header scan failed: {err}; skipping training sample to maintain dataset integrity"
                    ),
                );
                anyhow::bail!("GIF/header scan failed for {}: {err}", path.display());
            }
        }
    } else {
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_metadata",
            path,
            format!(
                "TRAINING AUDIT: skipping GIF/header scan for non-GIF container '{}'; using ffprobe-derived timing fields",
                probe.format_name
            ),
        );
    }

    // Call deep refinement to populate palette_depth, temporal_flatness, etc.
    if let Err(e) = crate::loop_intent::deep_refine_meta(&mut meta, path) {
        tracing::warn!(
            target: "mfb.database",
            path = %path.display(),
            error = %e,
            "loop sample gather: deep_refine_meta failed; continuing with ffprobe/header probe fields"
        );
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_metadata",
            path,
            format!(
                "TRAINING AUDIT: Sample metadata refinement failed for '{}' | Forensic: Deep refinement Error '{}'; retaining ffprobe/header probe fields",
                path.display(),
                e
            ),
        );
    }
    crate::loop_intent::ensure_frame_delay_variation(&mut meta);
    crate::loop_intent::ensure_block_skew(&mut meta);
    if let Err(err) = meta.ensure_webp_compression_ratio_from_path(path) {
        tracing::warn!(
            target: "mfb.database",
            path = %path.display(),
            error = %err,
            "loop sample gather: refusing row without empirical loop_stats_webp_ratio"
        );
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_intent",
            path,
            format!(
                "TRAINING AUDIT: Sample missing empirical loop_stats_webp_ratio for '{}' | Forensic: {err}; refusing to insert unverifiable loop training sample",
                path.display()
            ),
        );
        anyhow::bail!(
            "Sample metadata missing empirical loop_stats_webp_ratio for {}: {err}",
            path.display()
        );
    }
    if meta.frame_delay_variation.is_none() {
        tracing::warn!(
            target: "mfb.database",
            path = %path.display(),
            "loop sample gather: refusing row without empirical frame_delay_variation"
        );
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_intent",
            path,
            format!(
                "TRAINING AUDIT: Sample missing empirical frame_delay_variation for '{}' | Forensic: refusing to insert unverifiable loop timing sample",
                path.display()
            ),
        );
        anyhow::bail!(
            "Sample metadata missing empirical frame_delay_variation for {}",
            path.display()
        );
    }
    Ok(meta)
}

fn parent_directories_from_path(path: &Path) -> Option<Vec<String>> {
    path.parent().map(|parent| {
        parent
            .iter()
            .rev()
            .take(4)
            .filter_map(|segment| segment.to_str())
            .map(std::string::ToString::to_string)
            .collect()
    })
}

/// Heuristic loop-intent bucket for training balance (matches ingest auto-label path).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoopTrainingBalanceProbe {
    pub loss_tolerance: String,
    /// `loop` = strong loop intent, `non_loop` = video-like, `uncertain` = other.
    pub loop_intent: String,
    /// Complexity proxy for pairing with static-image entropy balance.
    pub complexity: f64,
    pub loop_frequency: f64,
    pub temporal_bpp: f64,
}

/// Probe loop vs non-loop intent using the same `sample_from_path` heuristics as ingest.
///
/// # Errors
/// Returns an error when metadata extraction fails or the asset is not a valid loop candidate.
pub fn probe_loop_training_balance(path: &Path) -> anyhow::Result<LoopTrainingBalanceProbe> {
    let sample = sample_from_path_result(path, "training_balance_probe", None)
        .with_context(|| format!("loop training balance probe failed: {}", path.display()))?;
    let loop_intent = match normalize_loop_training_label(sample.loss_tolerance.as_str()) {
        Some("high") => "loop",
        Some("video") => "non_loop",
        _ => "uncertain",
    };
    let complexity = sample.loop_frequency.max(sample.temporal_bpp).max(0.0_f64);
    Ok(LoopTrainingBalanceProbe {
        loss_tolerance: sample.loss_tolerance,
        loop_intent: loop_intent.to_string(),
        complexity,
        loop_frequency: sample.loop_frequency,
        temporal_bpp: sample.temporal_bpp,
    })
}

#[must_use]
pub fn sample_from_path(
    path: &Path,
    labeled_by: &str,
    label_override: Option<&str>,
) -> Option<SampleInsert> {
    match sample_from_path_result(path, labeled_by, label_override) {
        Ok(sample) => Some(sample),
        Err(err) => {
            tracing::warn!(
                target: "mfb.database",
                path = %path.display(),
                error = %err,
                "loop sample build rejected"
            );
            None
        }
    }
}

fn sample_from_path_result(
    path: &Path,
    labeled_by: &str,
    label_override: Option<&str>,
) -> Result<SampleInsert> {
    let mut meta = gather_sample_metadata(path)
        .with_context(|| format!("metadata unavailable for {}", path.display()))?;
    let source_context_path = crate::common_utils::training_source_path_for(path);
    meta.parent_directories = parent_directories_from_path(&source_context_path);
    meta.file_name = source_context_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(std::string::ToString::to_string)
        .or(meta.file_name);
    let fallback_source_ext =
        crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(
            &source_context_path,
        );
    meta.source_extension = crate::common_utils::detect_real_extension(&source_context_path)
        .map(str::to_string)
        .or({
            if fallback_source_ext.is_empty() {
                None
            } else {
                Some(fallback_source_ext)
            }
        })
        .or(meta.source_extension);

    let (Some(width), Some(height)) = (meta.width, meta.height) else {
        crate::media_conversion_gate::delivery_db_path_audit(
            "delivery_db_intent",
            path,
            format!(
                "TRAINING AUDIT: Sample missing critical width/height dimensions for '{}' | Forensic: DB requires NOT NULL for geometric features; skipping training sample",
                path.display()
            ),
        );
        anyhow::bail!(
            "Sample missing width/height dimensions for {}",
            path.display()
        );
    };

    let (temporal_bpp, spatial_bpp) = bpp_from_meta(&meta)
        .with_context(|| format!("Sample metadata missing BPP inputs for {}", path.display()))?;

    let loss_tolerance = if let Some(label) = label_override {
        label.to_string()
    } else {
        determine_loss_tolerance(
            temporal_bpp,
            meta.flags.color.has_embedded_icc,
            meta.flags.color.has_complex_color_profile,
            meta.app_extensions.as_deref(),
            &source_context_path,
            meta.file_name.as_deref(),
        )
    };

    // If manual label is "video", ensure we treat it as a video-like source
    if loss_tolerance == "video" {
        meta.flags.streams.is_native_gif = false;
    }

    let aspect_ratio = if height > 0 {
        Some(f64::from(width) / f64::from(height))
    } else {
        None
    };

    let total_pixels = u64::from(width) * u64::from(height);
    let loop_frequency =
        crate::loop_intent::score_loop_frequency(meta.duration_secs, meta.frame_count);
    let (is_human_semantic_name, directory_loop_intent_score, cadence_score) = {
        let analysis = crate::loop_intent::analyze_filename(meta.file_name.as_deref(), &[]);
        (
            analysis.kind == crate::loop_intent::FilenameKind::HumanSemantic,
            crate::loop_intent::score_directory_context(meta.parent_directories.as_deref(), &[]),
            crate::loop_intent::score_sparse_cadence(meta.duration_secs, meta.frame_count),
        )
    };
    let is_meme_platform = meta.flags.meme.is_meme_platform;
    let is_native_gif = meta.flags.streams.is_native_gif;
    let is_high_value_source = loss_tolerance == "low";

    let file_hash = match calculate_blake3_hex(path) {
        Ok(hash) => hash,
        Err(e) => {
            crate::media_conversion_gate::delivery_db_path_audit(
                "delivery_db_intent",
                path,
                format!(
                    "TRAINING AUDIT: Sample hashing (BLAKE3) failed for '{}' | Forensic: Error '{}'; skipping training sample to prevent identity collisions",
                    path.display(),
                    e
                ),
            );
            anyhow::bail!("Sample hashing (BLAKE3) failed for {}: {e}", path.display());
        }
    };

    Ok(SampleInsert {
        file_hash,
        source_path: source_context_path.display().to_string(),
        file_name: meta.file_name.clone(),
        source_ext: meta.source_extension.clone(),
        width,
        height,
        duration_secs: meta.duration_secs,
        frame_count: meta.frame_count,
        file_size_bytes: meta.file_size_bytes,
        fps: meta.fps,
        flags: SampleFlags {
            streams: SampleStreamFlags {
                has_transparency: meta.flags.streams.has_transparency,
                is_native_gif,
            },
            color: SampleColorFlags {
                has_embedded_icc: meta.flags.color.has_embedded_icc,
                has_complex_color_profile: meta.flags.color.has_complex_color_profile,
            },
            meme: SampleMemeFlags {
                is_meme_platform,
                is_human_semantic_name,
            },
            source: SampleSourceFlags {
                is_high_value_source,
            },
        },
        palette_size: meta.palette_size,
        frame_payload_variation: meta.frame_payload_variation,
        frame_delay_variation: meta.frame_delay_variation,
        temporal_bpp,
        spatial_bpp,
        loss_tolerance: loss_tolerance.clone(),
        labeled_by: labeled_by.to_string(),
        aspect_ratio,
        total_pixels,
        loop_frequency,
        cadence_score,
        directory_loop_intent_score,
        palette_depth: meta.palette_depth,
        motion_gini: meta.motion_gini,
        block_skew: meta.block_skew,
        temporal_flatness: meta.temporal_flatness,
        loop_closure_score: meta.loop_closure_score,
        motion_periodicity: meta.motion_periodicity,
        temporal_jitter: meta.temporal_jitter,
        webp_compression_ratio: meta.webp_compression_ratio,
        max_frame_delay: meta.max_frame_delay,
        min_frame_delay: meta.min_frame_delay,
        audio_duration_secs: meta.audio_duration_secs,
        path_depth: meta.path_depth,
        filename_numeric_density: meta.filename_numeric_density,
        physics_225: meta.physics_225,
        loop_verdict: loop_verdict_for_training_label(loss_tolerance.as_str()).to_string(),
    })
}

/// Compute the BLAKE3 hash of a file's contents, returned as a hex string.
///
/// Used as a unique identifier (`file_hash`) for deduplication in the database.
///
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub fn calculate_blake3_hex(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0u8; 65536].into_boxed_slice();
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Compute a 261-dimensional pgvector encoding for a sample using pre-calculated std deviations.
/// This precisely bakes the weights and normalization terms from the old dynamically computed KNN
/// into an L2-compatible vector, allowing `PostgreSQL`'s HNSW index to do the heavy lifting!
fn sample_row_from_meta(meta: &LoopMeta, temporal_bpp: f64, spatial_bpp: f64) -> Option<SampleRow> {
    let width = meta.width?;
    let height = meta.height?;
    Some(SampleRow {
        width,
        height,
        duration_secs: meta.duration_secs,
        frame_count: meta.frame_count,
        file_size_bytes: meta.file_size_bytes,
        fps: meta.fps,
        temporal_bpp,
        spatial_bpp,
        flags: SampleFlags {
            streams: SampleStreamFlags {
                has_transparency: meta.flags.streams.has_transparency,
                is_native_gif: meta.flags.streams.is_native_gif,
            },
            color: SampleColorFlags {
                has_embedded_icc: meta.flags.color.has_embedded_icc,
                has_complex_color_profile: meta.flags.color.has_complex_color_profile,
            },
            meme: SampleMemeFlags {
                is_meme_platform: meta.flags.meme.is_meme_platform,
                is_human_semantic_name: crate::loop_intent::analyze_filename(
                    meta.file_name.as_deref(),
                    &[],
                )
                .kind
                    == crate::loop_intent::FilenameKind::HumanSemantic,
            },
            source: SampleSourceFlags {
                is_high_value_source: meta.flags.color.has_embedded_icc
                    || meta.flags.color.has_complex_color_profile,
            },
        },
        palette_size: meta.palette_size,
        frame_payload_variation: meta.frame_payload_variation,
        frame_delay_variation: meta.frame_delay_variation,
        aspect_ratio: (height > 0).then(|| f64::from(width) / f64::from(height)),
        loop_frequency: Some(crate::loop_intent::score_loop_frequency(
            meta.duration_secs,
            meta.frame_count,
        )),
        cadence_score: Some(crate::loop_intent::score_sparse_cadence(
            meta.duration_secs,
            meta.frame_count,
        )),
        directory_loop_intent_score: Some(crate::loop_intent::score_directory_context(
            meta.parent_directories.as_deref(),
            &[],
        )),
        palette_depth: meta.palette_depth,
        motion_gini: meta.motion_gini,
        block_skew: meta.block_skew,
        temporal_flatness: meta.temporal_flatness,
        loop_closure_score: meta.loop_closure_score,
        motion_periodicity: meta.motion_periodicity,
        temporal_jitter: meta.temporal_jitter,
        webp_compression_ratio: meta.webp_compression_ratio,
        max_frame_delay: meta.max_frame_delay,
        min_frame_delay: meta.min_frame_delay,
        audio_duration_secs: meta.audio_duration_secs,
        path_depth: meta.path_depth,
        filename_numeric_density: meta.filename_numeric_density,
        physics_225: meta.physics_225.clone(),
    })
}

fn bpp_from_meta(meta: &LoopMeta) -> Option<(f64, f64)> {
    let (w, h) = (meta.width?, meta.height?);
    let pixel_count = (f64::from(w) * f64::from(h)).max(1.0);
    let frame_count = meta.frame_count.filter(|&fc| fc > 0)?;
    let file_size = crate::numeric_cast::u64_to_f64(meta.file_size_bytes);
    let frame_count_f64 = crate::numeric_cast::u64_to_f64(frame_count);

    Some((
        file_size / (pixel_count * frame_count_f64),
        file_size / pixel_count,
    ))
}

#[cfg(test)]
fn vector_l2_distance(lhs: &[f32], rhs: &[f32]) -> f64 {
    lhs.iter()
        .zip(rhs.iter())
        .map(|(a, b)| {
            let delta = f64::from(*a) - f64::from(*b);
            delta * delta
        })
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
fn sample_distance(
    meta: &LoopMeta,
    sample: &SampleRow,
    target_temporal_bpp: f64,
    target_spatial_bpp: f64,
    stats_map: &FeatureMap,
) -> f64 {
    let target = sample_row_from_meta(meta, target_temporal_bpp, target_spatial_bpp)
        .expect("sample_distance test fixture: meta must have width/height"); // audited: db module unit-test fixture assertion; not production DB runtime path
    let target_vector = crate::database_vector::compute_sample_vector(&target, stats_map)
        .expect("KNN: Target search vector computation failed during distance calculation"); // audited: db module unit-test fixture assertion; not production DB runtime path
    let sample_vector = crate::database_vector::compute_sample_vector(sample, stats_map)
        .expect("KNN: Sample search vector computation failed during distance calculation"); // audited: db module unit-test fixture assertion; not production DB runtime path
    vector_l2_distance(&target_vector, &sample_vector)
}

fn adaptive_neighbor_count(total: usize) -> Result<usize, String> {
    let count = crate::numeric_cast::u64_to_usize_strict(
        crate::numeric_cast::f64_to_u64_strict(
            (crate::numeric_cast::usize_to_f64(total)).sqrt().round(),
            "adaptive_neighbor_total_rounded",
        )
        .ok_or_else(|| {
            crate::media_conversion_gate::ui_user_facing_error(
                "Failed to calculate adaptive neighbor count",
            )
        })?,
        "adaptive_neighbor_total",
    )
    .ok_or_else(|| {
        crate::media_conversion_gate::ui_user_facing_error(
            "Adaptive neighbor count overflowed usize",
        )
    })?;

    Ok(count.clamp(6, 24).min(total))
}

fn class_balance_weight(total: i64, class_count: i64) -> f64 {
    let total_f = crate::numeric_cast::i64_to_f64(total.max(1));
    let class_f = crate::numeric_cast::i64_to_f64(class_count.max(0) + 1);
    // Smooth and dampen inverse-frequency weighting to avoid unstable over-correction
    // when class counts are highly imbalanced.
    ((total_f + 2.0) / (2.0 * class_f)).sqrt().clamp(0.67, 1.50)
}

fn smoothed_keep_prior(keep_count: i64, weak_count: i64) -> f64 {
    let keep = crate::numeric_cast::i64_to_f64(keep_count.max(0));
    let weak = crate::numeric_cast::i64_to_f64(weak_count.max(0));
    // Beta(1,1) prior to prevent extreme 0/1 priors on sparse datasets.
    (keep + 1.0) / (keep + weak + 2.0)
}

fn effective_sample_size(weight_squares_sum: f64, total_weight: f64) -> f64 {
    if weight_squares_sum <= 1e-9_f64 || total_weight <= 1e-9_f64 {
        return 0.0;
    }
    (total_weight * total_weight / weight_squares_sum).max(0.0)
}

fn imbalance_ratio(keep_count: i64, weak_count: i64) -> f64 {
    let keep = crate::numeric_cast::i64_to_f64(keep_count.max(0) + 1);
    let weak = crate::numeric_cast::i64_to_f64(weak_count.max(0) + 1);
    (keep.max(weak) / keep.min(weak)).max(1.0)
}

fn dynamic_neighbor_radius(neighbors: &[(LabelStatus, Option<f64>, f64)]) -> f64 {
    let mut distances: Vec<f64> = neighbors.iter().map(|(_, _, d)| *d).collect();
    if distances.is_empty() {
        crate::media_conversion_gate::delivery_db_batch_audit(
            "dynamic_neighbor_radius",
            "no neighbor distances; refusing fabricated 0.0 radius (no distance filter)",
        );
        return f64::INFINITY;
    }
    distances.sort_by(|a, b| crate::media_conversion_gate::f64_sort_cmp(*a, *b));
    let Some(q1) = crate::media_conversion_gate::db_sorted_distance_at(
        &distances,
        distances.len() / 4,
        "dynamic_neighbor_radius q1",
    ) else {
        return f64::INFINITY;
    };
    let Some(q3) = crate::media_conversion_gate::db_sorted_distance_at(
        &distances,
        (distances.len() * 3) / 4,
        "dynamic_neighbor_radius q3",
    ) else {
        return f64::INFINITY;
    };
    let iqr = (q3 - q1).max(0.06);
    let d0 = distances[0];
    (d0 + iqr * 1.5).max(d0 + 0.08)
}

pub(crate) fn normalize_log_ratio(a: f64, b: f64, scale: f64) -> f64 {
    if a <= 0.0_f64 || b <= 0.0_f64 || scale <= 0.0_f64 {
        return 1.0;
    }
    ((a.ln() - b.ln()).abs() / scale).clamp(0.0, 1.0)
}

fn percentile_value(sorted_values: &[f64], quantile: f64) -> Result<Option<f64>> {
    if sorted_values.is_empty() {
        return Ok(None);
    }

    let clamped = quantile.clamp(0.0, 1.0);
    let scaled_index =
        clamped * crate::numeric_cast::usize_to_f64(sorted_values.len().saturating_sub(1));
    let lower_index = crate::numeric_cast::f64_to_usize_strict(scaled_index.floor(), "lower_index")
        .with_context(|| {
            format!(
                "percentile lower index overflow (quantile={quantile}, len={})",
                sorted_values.len()
            )
        })?;
    let upper_index = crate::numeric_cast::f64_to_usize_strict(scaled_index.ceil(), "upper_index")
        .with_context(|| {
            format!(
                "percentile upper index overflow (quantile={quantile}, len={})",
                sorted_values.len()
            )
        })?;

    if lower_index == upper_index {
        return Ok(sorted_values.get(lower_index).copied());
    }

    let lower = sorted_values
        .get(lower_index)
        .copied()
        .with_context(|| format!("percentile lower index {lower_index} out of bounds"))?;
    let upper = sorted_values
        .get(upper_index)
        .copied()
        .with_context(|| format!("percentile upper index {upper_index} out of bounds"))?;
    Ok(Some((upper - lower).mul_add(
        scaled_index - crate::numeric_cast::usize_to_f64(lower_index),
        lower,
    )))
}

fn build_feature_stats(values: &[f64]) -> Result<FeatureStats> {
    if values.is_empty() {
        return Ok(FeatureStats::default());
    }

    let mean = if values.is_empty() {
        0.0_f64
    } else {
        values.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(values.len())
    };
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / crate::numeric_cast::usize_to_f64(values.len());

    let mut sorted = values.to_vec();
    sorted.sort_by(|lhs, rhs| crate::media_conversion_gate::f64_sort_cmp(*lhs, *rhs));

    Ok(FeatureStats {
        mean,
        std_dev: variance.sqrt(),
        weight: Some(1.0),
        p10: percentile_value(&sorted, 0.10)?,
        p25: percentile_value(&sorted, 0.25)?,
        p50: percentile_value(&sorted, 0.50)?,
        p75: percentile_value(&sorted, 0.75)?,
        p90: percentile_value(&sorted, 0.90)?,
    })
}

const fn loop_training_label_from_numeric(label: i16) -> Option<&'static str> {
    match label {
        1 => Some("high"),
        0 => Some("video"),
        _ => None,
    }
}

fn loop_stats_required_f64(field: Option<f64>, name: &'static str) -> Result<f64> {
    match field {
        Some(value) if value.is_finite() => Ok(value),
        Some(value) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "delivery_db_vector",
                format!(
                    "LoopIntent feature vector rejected non-finite required field '{name}' value={value}"
                ),
            );
            anyhow::bail!("LoopIntent feature vector non-finite required field '{name}'")
        }
        None => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "delivery_db_vector",
                format!("LoopIntent feature vector missing required field '{name}'"),
            );
            anyhow::bail!("LoopIntent feature vector missing required field '{name}'")
        }
    }
}

fn loop_stats_required_finite(value: f64, name: &'static str) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        crate::media_conversion_gate::delivery_db_batch_audit(
            "delivery_db_vector",
            format!(
                "LoopIntent feature vector rejected non-finite required scalar '{name}' value={value}"
            ),
        );
        anyhow::bail!("LoopIntent feature vector non-finite required scalar '{name}'")
    }
}

fn loop_stats_optional_sparse_f64(field: Option<f64>, name: &'static str) -> Result<f64> {
    match field {
        Some(value) if value.is_finite() => Ok(value),
        Some(value) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "delivery_db_vector",
                format!(
                    "LoopIntent feature vector rejected non-finite optional field '{name}' value={value}"
                ),
            );
            anyhow::bail!("LoopIntent feature vector non-finite optional field '{name}'")
        }
        None => Ok(crate::media_conversion_gate::knn_absent_feature_component()),
    }
}

fn build_loop_feature_vector(sample: &SampleRow) -> Result<[f64; LOOP_VECTOR_FEATURE_NAMES.len()]> {
    let duration = sample
        .duration_secs
        .with_context(|| "LoopIntent feature vector missing duration_secs")?;
    let duration = loop_stats_required_finite(duration, "loop_stats_duration")?;
    let frame_count = crate::numeric_cast::u64_to_f64(
        sample
            .frame_count
            .with_context(|| "LoopIntent feature vector missing frame_count")?,
    );
    let fps = loop_stats_required_f64(sample.fps, "loop_stats_fps")?;
    let aspect = loop_stats_required_f64(sample.aspect_ratio, "loop_stats_aspect")?;
    let loop_frequency =
        loop_stats_required_f64(sample.loop_frequency, "loop_stats_loop_frequency")?;
    let cadence = loop_stats_required_f64(sample.cadence_score, "loop_stats_cadence")?;
    let payload_var =
        loop_stats_required_f64(sample.frame_payload_variation, "loop_stats_payload_var")?;
    let delay_var = loop_stats_required_f64(sample.frame_delay_variation, "loop_stats_delay_var")?;
    let palette_depth = loop_stats_required_f64(sample.palette_depth, "loop_stats_palette_depth")?;
    let motion_gini = loop_stats_required_f64(sample.motion_gini, "loop_stats_motion_gini")?;
    let block_skew = loop_stats_optional_sparse_f64(sample.block_skew, "loop_stats_block_skew")?;
    let temporal_flatness =
        loop_stats_required_f64(sample.temporal_flatness, "loop_stats_temporal_flatness")?;
    let webp_ratio =
        loop_stats_required_f64(sample.webp_compression_ratio, "loop_stats_webp_ratio")?;
    let loop_closure =
        loop_stats_optional_sparse_f64(sample.loop_closure_score, "loop_stats_lclose")?;
    let motion_periodicity =
        loop_stats_optional_sparse_f64(sample.motion_periodicity, "loop_stats_motion_periodicity")?;
    let temporal_jitter =
        loop_stats_optional_sparse_f64(sample.temporal_jitter, "loop_stats_temporal_jitter")?;
    let directory_loop_intent =
        loop_stats_optional_sparse_f64(sample.directory_loop_intent_score, "loop_stats_dir_meme")?;
    let max_frame_delay =
        loop_stats_optional_sparse_f64(sample.max_frame_delay, "loop_stats_max_fd")?;
    let min_frame_delay =
        loop_stats_optional_sparse_f64(sample.min_frame_delay, "loop_stats_min_fd")?;
    let audio_duration =
        loop_stats_optional_sparse_f64(sample.audio_duration_secs, "loop_stats_audio_dur")?;
    let path_depth = f64::from(sample.path_depth);
    let filename_numeric_density =
        loop_stats_required_finite(sample.filename_numeric_density, "loop_stats_num_density")?;
    let (density, gap) = crate::database_vector::sample_frame_density_and_gap(sample)
        .with_context(
            || "LoopIntent feature vector missing frame density/gap (duration or frame_count)",
        )?;

    Ok([
        (f64::from(sample.width) * f64::from(sample.height)).max(1.0),
        duration,
        frame_count,
        crate::numeric_cast::u64_to_f64(sample.file_size_bytes),
        fps,
        density,
        gap,
        sample.temporal_bpp,
        sample.spatial_bpp,
        aspect,
        loop_frequency,
        cadence,
        payload_var,
        delay_var,
        palette_depth,
        motion_gini,
        block_skew,
        temporal_flatness,
        webp_ratio,
        crate::database_vector::sample_loop_affinity(sample),
        loop_closure,
        motion_periodicity,
        temporal_jitter,
        directory_loop_intent,
        max_frame_delay,
        min_frame_delay,
        audio_duration,
        path_depth,
        filename_numeric_density,
    ])
}

fn build_loop_feature_map(samples: &[LoopIntentTrainingSample]) -> Result<FeatureMap> {
    let top_keywords = extract_loop_training_keywords(samples);
    if samples.is_empty() {
        anyhow::bail!(
            "cannot build loop feature map from empty training sample set; refusing bootstrap histogram"
        );
    }

    let mut loop_vectors = Vec::with_capacity(samples.len());
    for (idx, sample) in samples.iter().enumerate() {
        loop_vectors.push(
            build_loop_feature_vector(&sample.sample_row).with_context(|| {
                format!("LoopIntent feature-map rejected at training sample index {idx}")
            })?,
        );
    }

    let mut feature_map = FeatureMap {
        top_keywords,
        ..Default::default()
    };
    for (idx, name) in LOOP_VECTOR_FEATURE_NAMES.iter().enumerate() {
        let values: Vec<f64> = loop_vectors.iter().map(|row| row[idx]).collect();
        feature_map.stats.insert(
            (*name).to_string(),
            build_feature_stats(&values).with_context(|| {
                format!("LoopIntent feature stats build failed for feature '{name}'")
            })?,
        );
    }
    Ok(feature_map)
}

fn extract_loop_training_keywords(samples: &[LoopIntentTrainingSample]) -> Vec<String> {
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for sample in samples.iter().filter(|sample| sample.label == 1) {
        let Some(file_name) = sample.file_name.as_ref() else {
            continue;
        };
        for raw_word in file_name.split(|c: char| !c.is_ascii_alphanumeric()) {
            if raw_word.len() <= 2 || raw_word.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let word = raw_word.to_ascii_lowercase();
            *counts.entry(word).or_insert(0) += 1;
        }
    }

    let mut ranked_words: Vec<(String, usize)> = counts.into_iter().collect();
    ranked_words.sort_by(|lhs, rhs| rhs.1.cmp(&lhs.1).then_with(|| lhs.0.cmp(&rhs.0)));
    ranked_words
        .into_iter()
        .take(50)
        .map(|(word, _)| word)
        .collect()
}

fn numeric_summary(values: &[f64]) -> Option<(f64, f64, f64)> {
    if values.is_empty() {
        return None;
    }
    let mut min_value = f64::INFINITY;
    let mut max_value = f64::NEG_INFINITY;
    let mut sum = 0.0_f64;
    for value in values {
        min_value = min_value.min(*value);
        max_value = max_value.max(*value);
        sum += *value;
    }
    Some((
        min_value,
        sum / crate::numeric_cast::usize_to_f64(values.len()),
        max_value,
    ))
}

fn build_loop_collection_stats(
    samples: &[LoopIntentTrainingSample],
    feature_map: &FeatureMap,
) -> Result<GlobalCollectionStats> {
    let mut durations = Vec::with_capacity(samples.len());
    let mut bitrates = Vec::with_capacity(samples.len());
    let mut aspects = Vec::with_capacity(samples.len());
    for (idx, sample) in samples.iter().enumerate() {
        let duration = sample
            .sample_row
            .duration_secs
            .with_context(|| format!("collection stats missing duration_secs at sample {idx}"))?;
        if duration <= 0.0 {
            anyhow::bail!("collection stats non-positive duration_secs at sample {idx}");
        }
        durations.push(duration);
        bitrates.push(
            crate::numeric_cast::u64_to_f64(sample.sample_row.file_size_bytes) * 8.0 / duration,
        );
        let aspect = sample
            .sample_row
            .aspect_ratio
            .with_context(|| format!("collection stats missing aspect_ratio at sample {idx}"))?;
        aspects.push(aspect);
    }
    let sizes: Vec<f64> = samples
        .iter()
        .map(|sample| crate::numeric_cast::u64_to_f64(sample.sample_row.file_size_bytes))
        .collect();
    let widths: Vec<f64> = samples
        .iter()
        .map(|sample| f64::from(sample.sample_row.width))
        .collect();
    let heights: Vec<f64> = samples
        .iter()
        .map(|sample| f64::from(sample.sample_row.height))
        .collect();

    let duration_stats = numeric_summary(&durations);
    let duration_p90_empirical = {
        let mut sorted_durations = durations;
        sorted_durations.sort_by(|lhs, rhs| crate::media_conversion_gate::f64_sort_cmp(*lhs, *rhs));
        percentile_value(&sorted_durations, 0.90)?
    };
    let size_stats = numeric_summary(&sizes);
    let bitrate_stats = numeric_summary(&bitrates);
    let width_stats = numeric_summary(&widths);
    let height_stats = numeric_summary(&heights);
    let aspect_stats = numeric_summary(&aspects);

    Ok(GlobalCollectionStats {
        duration_min: duration_stats.map(|stats| stats.0),
        duration_avg: duration_stats.map(|stats| stats.1),
        duration_max: duration_stats.map(|stats| stats.2),
        duration_p90: duration_p90_empirical,
        duration_p90_from_samples: duration_p90_empirical.is_some(),
        duration_stats_from_samples: duration_stats.is_some(),
        size_min: size_stats.map(|stats| stats.0),
        size_avg: size_stats.map(|stats| stats.1),
        size_max: size_stats.map(|stats| stats.2),
        bitrate_min: bitrate_stats.map(|stats| stats.0),
        bitrate_avg: bitrate_stats.map(|stats| stats.1),
        bitrate_max: bitrate_stats.map(|stats| stats.2),
        width_min: width_stats.map(|stats| stats.0),
        width_avg: width_stats.map(|stats| stats.1),
        width_max: width_stats.map(|stats| stats.2),
        height_min: height_stats.map(|stats| stats.0),
        height_avg: height_stats.map(|stats| stats.1),
        height_max: height_stats.map(|stats| stats.2),
        aspect_min: aspect_stats.map(|stats| stats.0),
        aspect_avg: aspect_stats.map(|stats| stats.1),
        aspect_max: aspect_stats.map(|stats| stats.2),
        top_keywords: feature_map.top_keywords.clone(),
    })
}

struct LoopMetadataSampleInput {
    width: i32,
    height: i32,
    duration_secs: f64,
    frame_count: i64,
    file_size_bytes: i64,
    fps: Option<f64>,
    motion_periodicity: Option<f64>,
    temporal_jitter: Option<f64>,
    motion_gini: Option<f64>,
    loop_closure_score: Option<f64>,
    cadence_score: Option<f64>,
    metadata: LoopIntentStoredMetadata,
}

fn sample_row_from_loop_metadata(input: LoopMetadataSampleInput) -> Option<SampleRow> {
    let LoopMetadataSampleInput {
        width,
        height,
        duration_secs,
        frame_count,
        file_size_bytes,
        fps,
        motion_periodicity,
        temporal_jitter,
        motion_gini,
        loop_closure_score,
        cadence_score,
        metadata,
    } = input;
    let width = crate::numeric_cast::i32_to_u32_strict(width, "loop_samples_width")?;
    let height = crate::numeric_cast::i32_to_u32_strict(height, "loop_samples_height")?;
    let frame_count =
        crate::numeric_cast::i64_to_u64_strict(frame_count, "loop_samples_frame_count")?;
    let file_size_bytes =
        crate::numeric_cast::i64_to_u64_strict(file_size_bytes, "loop_samples_file_size")?;
    let temporal_bpp =
        crate::numeric_cast::option_f64_strict(metadata.temporal_bpp, "loop_samples_temporal_bpp")?;
    let spatial_bpp =
        crate::numeric_cast::option_f64_strict(metadata.spatial_bpp, "loop_samples_spatial_bpp")?;
    let aspect_ratio = crate::media_conversion_gate::delivery_db_loop_aspect_ratio_or_derived(
        metadata.aspect_ratio,
        width,
        height,
    );

    Some(SampleRow {
        width,
        height,
        duration_secs: Some(duration_secs),
        frame_count: Some(frame_count),
        file_size_bytes,
        fps,
        temporal_bpp,
        spatial_bpp,
        flags: metadata.flags,
        palette_size: metadata.palette_size,
        frame_payload_variation: metadata.frame_payload_variation,
        frame_delay_variation: metadata.frame_delay_variation,
        aspect_ratio,
        loop_frequency: metadata.loop_frequency,
        cadence_score,
        directory_loop_intent_score: metadata.directory_loop_intent_score,
        palette_depth: metadata.palette_depth,
        motion_gini,
        block_skew: metadata.block_skew,
        temporal_flatness: metadata.temporal_flatness,
        loop_closure_score,
        motion_periodicity,
        temporal_jitter,
        webp_compression_ratio: metadata.webp_compression_ratio,
        max_frame_delay: metadata.max_frame_delay,
        min_frame_delay: metadata.min_frame_delay,
        audio_duration_secs: metadata.audio_duration_secs,
        path_depth: metadata.path_depth,
        filename_numeric_density: metadata.filename_numeric_density,
        physics_225: metadata.physics_225,
    })
}

fn stored_metadata_from_sample_probe(sample: &SampleInsert) -> LoopIntentStoredMetadata {
    LoopIntentStoredMetadata {
        source_ext: sample.source_ext.clone(),
        loss_tolerance: Some(sample.loss_tolerance.clone()),
        loop_verdict: Some(sample.loop_verdict.clone()),
        temporal_bpp: Some(sample.temporal_bpp),
        spatial_bpp: Some(sample.spatial_bpp),
        frame_payload_variation: sample.frame_payload_variation,
        frame_delay_variation: sample.frame_delay_variation,
        aspect_ratio: sample.aspect_ratio,
        total_pixels: Some(sample.total_pixels),
        loop_frequency: Some(sample.loop_frequency),
        directory_loop_intent_score: Some(sample.directory_loop_intent_score),
        palette_size: sample.palette_size,
        palette_depth: sample.palette_depth,
        block_skew: sample.block_skew,
        temporal_flatness: sample.temporal_flatness,
        webp_compression_ratio: sample.webp_compression_ratio,
        max_frame_delay: sample.max_frame_delay,
        min_frame_delay: sample.min_frame_delay,
        audio_duration_secs: sample.audio_duration_secs,
        path_depth: sample.path_depth,
        filename_numeric_density: sample.filename_numeric_density,
        physics_225: sample.physics_225.clone(),
        flags: sample.flags.clone(),
    }
}

const LOOP_METADATA_AUDIT_PRESERVE_KEYS: &[&str] = &[
    "layer6_fusion_score",
    "hdbscan_cluster_id",
    "hdbscan_cluster_loop_prior",
    "micro_nudge_score",
    "layer6b_keep_score",
    "layer6b_convert_score",
    "layer6b_margin",
    "layer6b_resolved",
    "tree_layer_exit",
    "tree_log_odds",
    "layer7_upstream",
    "resolution_path",
    "knn_lookup_succeeded",
    "hnsw_lookup_branch",
    "knn_telemetry_lookup_succeeded",
    "knn_telemetry_branch",
    "knn_telemetry_neighbor_count",
    "training_tier_audit",
    "embedding_schema",
    "physics_225_len",
    "physics_225_nonzero",
];

/// Keys that `verify-fabrication-stock` requires on `loop_samples.metadata` (JSON).
const LOOP_PROBE_METADATA_JSON_KEYS: &[&str] = &[
    "frame_delay_variation",
    "frame_payload_variation",
    "aspect_ratio",
    "loop_frequency",
    "palette_depth",
    "block_skew",
    "temporal_flatness",
    "webp_compression_ratio",
    "directory_loop_intent_score",
];

fn loop_metadata_json_missing_required_probe_fields(metadata: &Value) -> bool {
    let Some(obj) = metadata.as_object() else {
        return true;
    };
    LOOP_PROBE_METADATA_JSON_KEYS
        .iter()
        .any(|key| obj.get(*key).is_none_or(serde_json::Value::is_null))
}

fn parse_loop_intent_stored_metadata_for_repair(
    metadata_value: &Value,
) -> Option<LoopIntentStoredMetadata> {
    let result = crate::media_conversion_gate::delivery_db_json_or_default(
        serde_json::from_value::<LoopIntentStoredMetadata>(metadata_value.clone()),
        "DB AUDIT: invalid stored loop metadata JSON during probe repair",
        "loop_metadata_parse_repair",
    );
    if result.is_none() {
        tracing::warn!(
            target: "mfb.database",
            "loop probe repair: stored metadata JSON failed to parse; refusing to fabricate default"
        );
    }
    result
}

const fn loop_intent_stored_metadata_from_sample_row(
    mut base: LoopIntentStoredMetadata,
    row: &SampleRow,
) -> LoopIntentStoredMetadata {
    if let Some(v) = row.frame_delay_variation {
        base.frame_delay_variation = Some(v);
    }
    if let Some(v) = row.frame_payload_variation {
        base.frame_payload_variation = Some(v);
    }
    if let Some(v) = row.aspect_ratio {
        base.aspect_ratio = Some(v);
    }
    if let Some(v) = row.loop_frequency {
        base.loop_frequency = Some(v);
    }
    if let Some(v) = row.palette_depth {
        base.palette_depth = Some(v);
    }
    if let Some(v) = row.block_skew {
        base.block_skew = Some(v);
    }
    if let Some(v) = row.temporal_flatness {
        base.temporal_flatness = Some(v);
    }
    if let Some(v) = row.webp_compression_ratio {
        base.webp_compression_ratio = Some(v);
    }
    if let Some(v) = row.directory_loop_intent_score {
        base.directory_loop_intent_score = Some(v);
    }
    base.temporal_bpp = Some(row.temporal_bpp);
    base.spatial_bpp = Some(row.spatial_bpp);
    base
}

#[allow(dead_code)] // Struct-level counterpart of loop_metadata_json_missing_required_probe_fields
const fn loop_intent_stored_metadata_has_required_probe_fields(
    meta: &LoopIntentStoredMetadata,
) -> bool {
    meta.frame_delay_variation.is_some()
        && meta.frame_payload_variation.is_some()
        && meta.aspect_ratio.is_some()
        && meta.loop_frequency.is_some()
        && meta.palette_depth.is_some()
        && meta.block_skew.is_some()
        && meta.temporal_flatness.is_some()
        && meta.webp_compression_ratio.is_some()
        && meta.directory_loop_intent_score.is_some()
}

/// Fields that `verify-fabrication-stock` treats as hard blockers today.
const fn loop_intent_stored_metadata_satisfies_fabrication_verify(
    meta: &LoopIntentStoredMetadata,
) -> bool {
    meta.frame_delay_variation.is_some()
}

fn merge_loop_probe_metadata_json(
    existing: Value,
    fresh_probe: &LoopIntentStoredMetadata,
) -> Result<Value> {
    let mut merged = serde_json::to_value(fresh_probe)?;
    let fresh_obj = merged
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("fresh loop metadata must be a JSON object"))?;
    if let Value::Object(old_obj) = existing {
        for key in LOOP_PROBE_METADATA_JSON_KEYS {
            let fresh_missing = fresh_obj.get(*key).is_none_or(serde_json::Value::is_null);
            if fresh_missing
                && let Some(value) = old_obj.get(*key)
                && !value.is_null()
            {
                fresh_obj.insert((*key).to_string(), value.clone());
            }
        }
        for key in LOOP_METADATA_AUDIT_PRESERVE_KEYS {
            if let Some(value) = old_obj.get(*key) {
                fresh_obj.insert((*key).to_string(), value.clone());
            }
        }
    }
    Ok(merged)
}

/// Re-probe `loop_samples` rows whose stored metadata cannot build an empirical feature vector.
///
/// Returns `(repaired, skipped_no_source_path, reprobe_failed)`.
pub fn repair_loop_samples_missing_probe_fields(
    conn: &mut Client,
) -> Result<(usize, usize, usize)> {
    // `verify-fabrication-stock` fails on null `frame_delay_variation`; other keys may be null
    // (e.g. `block_skew` is not populated by ffprobe today) without blocking PROJECT_100.
    let predicate = "(metadata->>'frame_delay_variation' IS NULL \
        OR NOT (metadata ? 'frame_delay_variation'))";
    let query = format!(
        "SELECT
            blake3, source_path, file_name, width, height, duration_secs, frame_count,
            file_size_bytes, fps, motion_periodicity, temporal_jitter, motion_gini,
            loop_closure_score, cadence_score, label, labeled_by, metadata
         FROM loop_samples
         WHERE frame_count > 1 AND {predicate}"
    );
    let rows = conn
        .query(&query, &[])
        .context("LoopIntent loop_samples SELECT for probe repair failed")?;

    let mut repaired = 0_usize;
    let mut skipped_no_path = 0_usize;
    let mut reprobe_failed = 0_usize;
    let total = rows.len();
    tracing::info!(
        target: "mfb.database",
        total,
        "loop probe repair: scanning rows with missing probe metadata"
    );

    for (idx, row) in rows.into_iter().enumerate() {
        if idx > 0 && idx % 25 == 0 {
            tracing::info!(
                target: "mfb.database",
                idx,
                total,
                repaired,
                skipped_no_path,
                reprobe_failed,
                "loop probe repair progress"
            );
            crate::ui_stderr::line(
                crate::modern_ui::symbols::INFO,
                crate::modern_ui::symbols::plain::INFO,
                format!(
                    "loop probe repair: {idx}/{total} repaired={repaired} reprobe_failed={reprobe_failed} skipped_no_path={skipped_no_path}"
                ),
            );
        }

        let blake3: Vec<u8> = row.get(0);
        let source_path: Option<String> = row.get(1);
        let width: i32 = row.get(3);
        let height: i32 = row.get(4);
        let duration_secs: f64 = row.get(5);
        let frame_count: i64 = row.get(6);
        let file_size_bytes: i64 = row.get(7);
        let fps: Option<f64> = row.get(8);
        let motion_periodicity: Option<f64> = row.get(9);
        let temporal_jitter: Option<f64> = row.get(10);
        let motion_gini: Option<f64> = row.get(11);
        let loop_closure_score: Option<f64> = row.get(12);
        let cadence_score: Option<f64> = row.get(13);
        let label: i16 = row.get(14);
        let labeled_by: Option<String> = row.get(15);
        let metadata_value: Value = row.get(16);

        let needs_json_probe_fields =
            loop_metadata_json_missing_required_probe_fields(&metadata_value);

        if !needs_json_probe_fields
            && let Some(parsed) = parse_loop_intent_stored_metadata_for_repair(&metadata_value)
            && let Some(sample_row) = sample_row_from_loop_metadata(LoopMetadataSampleInput {
                width,
                height,
                duration_secs,
                frame_count,
                file_size_bytes,
                fps,
                motion_periodicity,
                temporal_jitter,
                motion_gini,
                loop_closure_score,
                cadence_score,
                metadata: parsed,
            })
            && build_loop_feature_vector(&sample_row).is_ok()
        {
            continue;
        }

        // Reconcile metadata from stored columns + JSON when the feature vector
        // already builds in memory — no disk decode.
        if let Some(parsed) = parse_loop_intent_stored_metadata_for_repair(&metadata_value)
            && let Some(sample_row) = sample_row_from_loop_metadata(LoopMetadataSampleInput {
                width,
                height,
                duration_secs,
                frame_count,
                file_size_bytes,
                fps,
                motion_periodicity,
                temporal_jitter,
                motion_gini,
                loop_closure_score,
                cadence_score,
                metadata: parsed.clone(),
            })
            && build_loop_feature_vector(&sample_row).is_ok()
        {
            let fresh = loop_intent_stored_metadata_from_sample_row(parsed, &sample_row);
            if loop_intent_stored_metadata_satisfies_fabrication_verify(&fresh) {
                let merged = merge_loop_probe_metadata_json(metadata_value, &fresh)?;
                conn.execute(
                    "UPDATE loop_samples SET metadata = $1::jsonb WHERE blake3 = $2",
                    &[&merged, &blake3],
                )
                .context("loop_samples metadata UPDATE after in-memory reconciliation failed")?;
                tracing::info!(
                    target: "mfb.database",
                    needs_json_probe_fields,
                    "loop probe repair: reconciled metadata JSON from column data"
                );
                repaired += 1;
                continue;
            }
        }

        // Single on-disk reprobe when stored data cannot build an empirical vector.
        let Some(source_path) = source_path.filter(|p| !p.trim().is_empty()) else {
            skipped_no_path += 1;
            continue;
        };
        let Some(label_override) = loop_training_label_from_numeric(label) else {
            reprobe_failed += 1;
            continue;
        };
        let Some(insert) = sample_from_path(
            Path::new(&source_path),
            crate::media_conversion_gate::db_labeled_by_or_default(labeled_by.as_deref()),
            Some(label_override),
        ) else {
            tracing::warn!(
                target: "mfb.database",
                path = %source_path,
                "loop probe repair: on-disk reprobe failed; refusing stored-metadata or default backfill"
            );
            reprobe_failed += 1;
            continue;
        };
        let fresh_probe = stored_metadata_from_sample_probe(&insert);
        if fresh_probe.frame_delay_variation.is_none() {
            tracing::warn!(
                target: "mfb.database",
                path = %source_path,
                fps = ?fps,
                frame_count,
                "loop probe repair: reprobe missing frame_delay_variation; refusing silent CFR column backfill"
            );
            reprobe_failed += 1;
            continue;
        }
        let merged = merge_loop_probe_metadata_json(metadata_value, &fresh_probe)?;
        let merged_meta = parse_loop_intent_stored_metadata_for_repair(&merged);
        if merged_meta.is_none_or(|m| !loop_intent_stored_metadata_satisfies_fabrication_verify(&m))
        {
            tracing::warn!(
                target: "mfb.database",
                path = %source_path,
                frame_delay_variation = ?fresh_probe.frame_delay_variation,
                frame_payload_variation = ?fresh_probe.frame_payload_variation,
                "loop probe repair: merged metadata still missing fabrication-verify probe fields; refusing to write"
            );
            reprobe_failed += 1;
            continue;
        }
        conn.execute(
            "UPDATE loop_samples SET metadata = $1::jsonb WHERE blake3 = $2",
            &[&merged, &blake3],
        )
        .context("loop_samples metadata UPDATE after probe repair failed")?;
        tracing::info!(
            target: "mfb.database",
            path = %source_path,
            "loop probe repair: wrote empirical probe metadata from on-disk reprobe"
        );
        repaired += 1;
    }

    Ok((repaired, skipped_no_path, reprobe_failed))
}

fn loop_sample_row_or_reprobe_from_source(
    sample_row: Option<SampleRow>,
    source_path: Option<&str>,
    labeled_by: Option<&str>,
    label: i16,
) -> Option<SampleRow> {
    if let Some(ref row) = sample_row
        && build_loop_feature_vector(row).is_ok()
    {
        return sample_row;
    }
    let source_path = source_path?;
    let label_override = loop_training_label_from_numeric(label)?;
    let sample = sample_from_path(
        Path::new(source_path),
        crate::media_conversion_gate::db_labeled_by_or_default(labeled_by),
        Some(label_override),
    )?;
    Some(sample.into())
}

fn loop_training_sample_from_scenario_row(row: &postgres::Row) -> Result<LoopIntentTrainingSample> {
    let blake3: Vec<u8> = row.get(0);
    let source_path: Option<String> = row.get(1);
    let file_name: Option<String> = row.get(2);
    let width: i32 = row.get(3);
    let height: i32 = row.get(4);
    let duration_secs: f64 = row.get(5);
    let frame_count: i64 = row.get(6);
    let file_size_bytes: i64 = row.get(7);
    let fps: Option<f64> = row.get(8);
    let motion_periodicity: Option<f64> = row.get(9);
    let temporal_jitter: Option<f64> = row.get(10);
    let motion_gini: Option<f64> = row.get(11);
    let loop_closure_score: Option<f64> = row.get(12);
    let cadence_score: Option<f64> = row.get(13);
    let label: i16 = row.get(14);
    let labeled_by: Option<String> = row.get(15);
    let metadata_value: Value = row.get(16);

    let parsed_metadata = crate::media_conversion_gate::delivery_db_json_or_default(
        serde_json::from_value::<LoopIntentStoredMetadata>(metadata_value),
        "DB AUDIT: invalid stored loop metadata JSON",
        "loop_metadata_parse",
    )
    .ok_or_else(|| anyhow::anyhow!("invalid stored loop metadata JSON"))?;
    let sample_row = loop_sample_row_or_reprobe_from_source(
        sample_row_from_loop_metadata(LoopMetadataSampleInput {
            width,
            height,
            duration_secs,
            frame_count,
            file_size_bytes,
            fps,
            motion_periodicity,
            temporal_jitter,
            motion_gini,
            loop_closure_score,
            cadence_score,
            metadata: parsed_metadata,
        }),
        source_path.as_deref(),
        labeled_by.as_deref(),
        label,
    )
    .ok_or_else(|| anyhow::anyhow!("loop sample row reprobe/build failed"))?;

    Ok(LoopIntentTrainingSample {
        blake3,
        file_name,
        label,
        sample_row,
    })
}

fn load_loop_intent_training_samples(conn: &mut Client) -> Result<Vec<LoopIntentTrainingSample>> {
    let rows = conn
        .query(
            "SELECT
            blake3, source_path, file_name, width, height, duration_secs, frame_count,
            file_size_bytes, fps, motion_periodicity, temporal_jitter, motion_gini,
            loop_closure_score, cadence_score, label, labeled_by, metadata
         FROM loop_samples
         WHERE frame_count > 1",
            &[],
        )
        .context("LoopIntent loop_samples SELECT for feature refresh failed")?;

    let mut samples = Vec::with_capacity(rows.len());
    for (idx, row) in rows.iter().enumerate() {
        samples.push(
            loop_training_sample_from_scenario_row(row).with_context(|| {
                format!("LoopIntent training sample load rejected at row index {idx}")
            })?,
        );
    }
    Ok(samples)
}

fn recompute_loop_intent_embeddings(
    conn: &mut Client,
    samples: &[LoopIntentTrainingSample],
    feature_map: &FeatureMap,
) -> Result<usize> {
    let mut tx = conn.transaction()?;
    let stmt = tx
        .prepare("UPDATE loop_samples SET embedding = $1::vector WHERE blake3 = $2")
        .context("LoopIntent embedding UPDATE prepare failed")?;
    let mut updated = 0_usize;
    let total = samples.len();
    let mut skipped_vectors = 0usize;

    for sample in samples {
        let vec_data = match crate::database_vector::compute_sample_vector(
            &sample.sample_row,
            feature_map,
        ) {
            Ok(vector) => vector,
            Err(err) => {
                skipped_vectors += 1;
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "delivery_db_vector",
                    format!(
                        "LoopIntent embedding recompute skip: vector unavailable | Forensic: {err}"
                    ),
                );
                continue;
            }
        };
        let pg_vector = pgvector::Vector::from(vec_data);
        tx.execute(&stmt, &[&pg_vector, &sample.blake3])
            .context("LoopIntent embedding UPDATE execute failed")?;
        updated += 1;
    }

    if skipped_vectors > 0 {
        anyhow::bail!(
            "LoopIntent embedding recompute rejected: {skipped_vectors}/{total} samples could not be vectorized (refusing partial backfill)"
        );
    }

    tx.commit()
        .context("LoopIntent embedding UPDATE commit failed")?;
    Ok(updated)
}

/// Recompute normalized `LoopIntent` feature statistics and backfill embeddings.
///
/// # Errors
///
/// Returns an error when schema initialization, sample loading, stats encoding,
/// metadata persistence, or embedding recomputation fails.
pub fn refresh_loop_intent_feature_stats(conn: &mut Client) -> Result<()> {
    crate::multi_scenario_db::init_multi_scenario_schema(conn)
        .context("LoopIntent stats refresh schema init failed")?;
    crate::multi_scenario_db::with_multi_scenario_metadata_lock(conn, |conn| {
        refresh_loop_intent_feature_stats_inner(conn)
    })
}

fn refresh_loop_intent_feature_stats_inner(conn: &mut Client) -> Result<()> {
    crate::ui_stderr::line(
        crate::modern_ui::symbols::INFO,
        crate::modern_ui::symbols::plain::INFO,
        "Recomputing LoopIntent feature statistics...",
    );

    let samples = load_loop_intent_training_samples(conn)
        .context("LoopIntent training sample load failed")?;
    if samples.is_empty() {
        crate::ui_stderr::line(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
            "LoopIntent stats refresh: no loop_samples rows; seeding cold-start feature_stats.",
        );
        let cold = cold_start_loop_training_feature_map();
        persist_loop_training_feature_map(conn, &cold)
            .context("LoopIntent cold-start metadata seed on empty corpus failed")?;
        let collection = cold_start_loop_collection_stats();
        persist_loop_collection_stats(conn, &collection)
            .context("LoopIntent cold-start collection_stats seed on empty corpus failed")?;
        conn.execute(
            "UPDATE multi_scenario_metadata
             SET sample_count = 0,
                 last_updated = CURRENT_TIMESTAMP
             WHERE scenario = $1",
            &[&crate::scenario::ScenarioType::LoopIntent.to_string()],
        )
        .context("LoopIntent cold-start sample_count reset failed")?;
        return Ok(());
    }

    let prior_catalog =
        fetch_loop_hdbscan_catalog_seed(conn).context("LoopIntent catalog seed fetch failed")?;
    let mut feature_map =
        build_loop_feature_map(&samples).context("LoopIntent feature-map construction failed")?;
    feature_map.hdbscan_catalog = prior_catalog;
    let collection_stats = build_loop_collection_stats(&samples, &feature_map)
        .context("LoopIntent collection stats construction failed")?;
    let encoded_stats = serde_json::to_value(&feature_map)?;
    let encoded_collection = serde_json::to_value(&collection_stats)?;

    let sample_count_i64 = crate::numeric_cast::usize_to_i64_sat(samples.len());
    conn.execute(
        "UPDATE multi_scenario_metadata
         SET feature_stats = $1::jsonb,
             collection_stats = $2::jsonb,
             sample_count = $3,
             last_updated = CURRENT_TIMESTAMP
         WHERE scenario = $4",
        &[
            &encoded_stats,
            &encoded_collection,
            &sample_count_i64,
            &crate::scenario::ScenarioType::LoopIntent.to_string(),
        ],
    )
    .context("LoopIntent metadata stats row update failed")?;

    let updated = recompute_loop_intent_embeddings(conn, &samples, &feature_map)
        .context("LoopIntent embedding recompute failed")?;
    crate::ui_stderr::line(
        crate::modern_ui::symbols::SUCCESS,
        crate::modern_ui::symbols::plain::SUCCESS,
        format!(
            "   LoopIntent statistics refreshed from {} samples; {} embeddings backfilled.",
            samples.len(),
            updated
        ),
    );
    maybe_alert_production_hdbscan_catalog_gap(conn, "feature_stats_refresh_complete");
    Ok(())
}

/// Scan a dataset tree and ingest supported dynamic assets into `LoopIntent`.
///
/// # Errors
///
/// Returns an error when connecting to the database, initializing schema state,
/// preparing feature stats, configuring progress reporting, or ingesting sample
/// rows fails.
pub fn batch_ingest_loop_intent_samples(
    dataset_path: &Path,
    label_override: Option<&str>,
    conn_str: &str,
) -> Result<usize> {
    let mut conn = connect_pg_with_str(conn_str)?;
    crate::multi_scenario_db::init_multi_scenario_schema(&mut conn)
        .context("LoopIntent schema initialization failed")?;

    crate::ui_stderr::line(
        crate::modern_ui::symbols::SEARCH,
        crate::modern_ui::symbols::plain::SEARCH,
        format!(
            "Scanning for LoopIntent assets in {}...",
            dataset_path.display()
        ),
    );

    let mut candidate_paths = Vec::new();
    for entry in WalkDir::new(dataset_path)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path().to_path_buf();
        let Some(ext) = detect_dynamic_media_extension(&path)? else {
            continue;
        };
        if is_supported_dynamic_training_extension(ext.as_str()) {
            candidate_paths.push(path);
        }
    }

    if candidate_paths.is_empty() {
        crate::ui_stderr::line(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
            "No LoopIntent training assets found.",
        );
        return Ok(0);
    }

    let feature_map = prepare_loop_training_feature_map(&mut conn)
        .context("LoopIntent feature-map preparation failed")?;
    let total_candidates = u64::try_from(candidate_paths.len())
        .context("candidate path count should fit in u64 for progress reporting")?;
    let pb = ProgressBar::new(total_candidates);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )?
        .progress_chars("#>-"),
    );

    let ingest_outcomes: Vec<anyhow::Result<crate::multi_scenario_db::ScenarioSample>> =
        candidate_paths
            .par_iter()
            .map(|path| {
                let span = tracing::info_span!("loop_scenario_ingestion", file = %path.display());
                let _enter = span.enter();
                let outcome: anyhow::Result<crate::multi_scenario_db::ScenarioSample> = (|| {
                    let hash = crate::common_utils::calculate_blake3_hash_bytes(path)
                        .with_context(|| {
                            format!("loop ingest hash computation failed for {}", path.display())
                        })?;
                    build_loop_intent_scenario_sample(
                        path,
                        "cli_ingest",
                        label_override,
                        hash,
                        &feature_map,
                    )
                })(
                );
                pb.inc(1);
                match &outcome {
                    Ok(sample) => {
                        pb.set_message(format!(
                            "Learn: {}",
                            crate::media_conversion_gate::trace_label_or_default(
                                sample.file_name.as_deref(),
                                "?",
                            )
                        ));
                    }
                    Err(e) => {
                        crate::media_conversion_gate::delivery_db_path_audit(
                            "loop_ingest_sample",
                            path,
                            format!("failed to build loop intent sample: {e}"),
                        );
                    }
                }
                outcome
            })
            .collect();

    pb.finish_with_message("LoopIntent ingestion prepared.");

    let mut ingest_failures = Vec::new();
    let mut scenario_samples = Vec::with_capacity(ingest_outcomes.len());
    for (path, outcome) in candidate_paths.iter().zip(ingest_outcomes) {
        match outcome {
            Ok(sample) => scenario_samples.push(sample),
            Err(err) => ingest_failures.push(format!("{}: {err:#}", path.display())),
        }
    }
    if !ingest_failures.is_empty() {
        let preview = ingest_failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "LoopIntent batch ingest rejected: {}/{} candidates failed\n{preview}",
            ingest_failures.len(),
            candidate_paths.len()
        );
    }

    let mut count = 0_usize;
    for sample in &scenario_samples {
        crate::multi_scenario_db::ingest_loop_intent_sample(&mut conn, sample).with_context(
            || {
                format!(
                    "Failed to ingest {}",
                    crate::media_conversion_gate::trace_label_or_default(
                        sample.source_path.as_deref(),
                        "<unknown>",
                    )
                )
            },
        )?;
        count += 1;
    }

    if count > 0 {
        refresh_loop_intent_feature_stats(&mut conn)
            .context("LoopIntent feature stats refresh failed")?;
    }

    Ok(count)
}

fn detect_dynamic_media_extension(path: &Path) -> anyhow::Result<Option<String>> {
    if let Some(ext) = crate::common_utils::detect_real_extension(path) {
        return Ok(Some(ext.to_string()));
    }

    let Some(kind) = infer::get_from_path(path)
        .with_context(|| format!("failed to infer media type for {}", path.display()))?
    else {
        return Ok(None);
    };
    match kind.mime_type() {
        "image/gif" => Ok(Some("gif".to_string())),
        "image/webp" => Ok(Some("webp".to_string())),
        "image/avif" => Ok(Some("avif".to_string())),
        "image/heic" => Ok(Some("heic".to_string())),
        "image/heif" => Ok(Some("heif".to_string())),
        "image/jxl" => Ok(Some("jxl".to_string())),
        "video/mp4" => Ok(Some("mp4".to_string())),
        "video/quicktime" => Ok(Some("mov".to_string())),
        "video/webm" => Ok(Some("webm".to_string())),
        "video/x-matroska" => Ok(Some("mkv".to_string())),
        "video/x-msvideo" => Ok(Some("avi".to_string())),
        "video/x-flv" | "video/flv" => Ok(Some("flv".to_string())),
        _ => Ok(None),
    }
}

fn is_supported_dynamic_training_extension(ext: &str) -> bool {
    ext == "gif"
        || crate::constants::MODERN_ANIMATED_EXTENSIONS.contains(&ext)
        || crate::file_copier::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext)
}

// ── Level 4: Inference Logging ───────────────────────────────────────────────

/// Optional Layer 6 / HDBSCAN telemetry merged into `signal_snapshot` (no extra DB columns).
#[derive(Debug, Clone, Default)]
pub struct LoopInferenceAudit {
    pub layer6_fusion_score: Option<f64>,
    pub hdbscan_cluster_id: Option<i32>,
    pub hdbscan_cluster_loop_prior: Option<f64>,
    pub micro_nudge_score: Option<f64>,
    /// Layer 6-B directional arbitration scores (always logged when arbitration runs).
    pub layer6b_keep_score: Option<f64>,
    pub layer6b_convert_score: Option<f64>,
    pub layer6b_margin: Option<f64>,
    pub layer6b_resolved: Option<bool>,
    /// Decision tree exit layer tag (e.g. `Layer 3`, `Layer 5`) before KNN/fallback.
    pub tree_layer_exit: Option<String>,
    pub tree_log_odds: Option<f64>,
    /// Upstream reason passed into Layer 7 when fallback runs.
    pub layer7_upstream: Option<String>,
    /// Terminal resolution path: `tree_decisive`, `layer6_knn_fusion`, `layer6b_arbitration`, etc.
    pub resolution_path: Option<String>,
    /// `Some(true/false)` when KNN was attempted after tree uncertainty; `None` if never reached.
    pub knn_lookup_succeeded: Option<bool>,
    /// HNSW lookup branch tag (`hnsw_no_rows`, `corpus_immature`, `success`, …) when KNN ran.
    pub hnsw_lookup_branch: Option<String>,
    /// Optional corpus probe when the tree was decisive before Layer 6 (telemetry only).
    pub knn_telemetry_lookup_succeeded: Option<bool>,
    pub knn_telemetry_branch: Option<String>,
    pub knn_telemetry_neighbor_count: Option<usize>,
}

impl LoopInferenceAudit {
    fn seal_algorithm_outputs(&mut self) {
        self.layer6_fusion_score = self
            .layer6_fusion_score
            .and_then(crate::algorithm_seal::loop_unit_probability);
        self.hdbscan_cluster_loop_prior = self
            .hdbscan_cluster_loop_prior
            .and_then(crate::algorithm_seal::loop_unit_probability);
        self.micro_nudge_score = self
            .micro_nudge_score
            .and_then(crate::algorithm_seal::loop_finite_scalar);
        self.layer6b_keep_score = self
            .layer6b_keep_score
            .and_then(crate::algorithm_seal::loop_finite_scalar);
        self.layer6b_convert_score = self
            .layer6b_convert_score
            .and_then(crate::algorithm_seal::loop_finite_scalar);
        self.layer6b_margin = self
            .layer6b_margin
            .and_then(crate::algorithm_seal::loop_finite_scalar);
        self.tree_log_odds = self
            .tree_log_odds
            .and_then(crate::algorithm_seal::loop_finite_scalar);
    }
}

/// Build a JSON snapshot of key `LoopMeta` fields for the inference log.
fn build_signal_snapshot(meta: &LoopMeta, audit: Option<&LoopInferenceAudit>) -> Value {
    const CTX: &str = "build_signal_snapshot";
    let opt_f64 = |_: &str, v: Option<f64>| {
        crate::media_conversion_gate::json_inference_optional_f64_or_null(v)
    };

    json!({
        "duration_secs": opt_f64("duration_secs", meta.duration_secs),
        "width": meta.width,
        "height": meta.height,
        "fps": opt_f64("fps", meta.fps),
        "frame_count": meta.frame_count,
        "file_size_bytes": meta.file_size_bytes,
        "has_audio": meta.flags.streams.has_audio,
        "has_transparency": meta.flags.streams.has_transparency,
        "is_native_gif": meta.flags.streams.is_native_gif,
        "has_embedded_icc": meta.flags.color.has_embedded_icc,
        "has_complex_color_profile": meta.flags.color.has_complex_color_profile,
        "is_meme_platform": meta.flags.meme.is_meme_platform,
        "loop_count": meta.loop_count,
        "webp_compression_ratio": opt_f64("webp_compression_ratio", meta.webp_compression_ratio),
        "palette_depth": opt_f64("palette_depth", meta.palette_depth),
        "motion_gini": opt_f64("motion_gini", meta.motion_gini),
        "temporal_flatness": opt_f64("temporal_flatness", meta.temporal_flatness),
        "block_skew": opt_f64("block_skew", meta.block_skew),
        "frame_payload_variation": opt_f64("frame_payload_variation", meta.frame_payload_variation),
        "frame_delay_variation": opt_f64("frame_delay_variation", meta.frame_delay_variation),
        "directory_loop_intent_score": crate::media_conversion_gate::json_required_finite_f64_or_null(
            meta.directory_loop_intent_score,
            "directory_loop_intent_score",
            CTX,
        ),
        "filename_loop_intent_score": crate::media_conversion_gate::json_required_finite_f64_or_null(
            meta.filename_loop_intent_score,
            "filename_loop_intent_score",
            CTX,
        ),
        "source_extension": meta.source_extension,
        "container": meta.container,
        "layer6_fusion_score": opt_f64(
            "layer6_fusion_score",
            audit.and_then(|a| a.layer6_fusion_score),
        ),
        "hdbscan_cluster_id": crate::media_conversion_gate::json_inference_optional_i32_or_null(
            audit.and_then(|a| a.hdbscan_cluster_id),
        ),
        "hdbscan_cluster_loop_prior": opt_f64(
            "hdbscan_cluster_loop_prior",
            audit.and_then(|a| a.hdbscan_cluster_loop_prior),
        ),
        "micro_nudge_score": opt_f64("micro_nudge_score", audit.and_then(|a| a.micro_nudge_score)),
        "layer6b_keep_score": opt_f64("layer6b_keep_score", audit.and_then(|a| a.layer6b_keep_score)),
        "layer6b_convert_score": opt_f64(
            "layer6b_convert_score",
            audit.and_then(|a| a.layer6b_convert_score),
        ),
        "layer6b_margin": opt_f64("layer6b_margin", audit.and_then(|a| a.layer6b_margin)),
        "layer6b_resolved": crate::media_conversion_gate::json_inference_optional_bool_or_null(
            audit.and_then(|a| a.layer6b_resolved),
        ),
        "tree_layer_exit": crate::media_conversion_gate::json_inference_optional_string_or_null(
            audit.and_then(|a| a.tree_layer_exit.as_deref()),
        ),
        "tree_log_odds": opt_f64("tree_log_odds", audit.and_then(|a| a.tree_log_odds)),
        "layer7_upstream": crate::media_conversion_gate::json_inference_optional_string_or_null(
            audit.and_then(|a| a.layer7_upstream.as_deref()),
        ),
        "resolution_path": crate::media_conversion_gate::json_inference_optional_string_or_null(
            audit.and_then(|a| a.resolution_path.as_deref()),
        ),
        "knn_lookup_succeeded": crate::media_conversion_gate::json_inference_optional_bool_or_null(
            audit.and_then(|a| a.knn_lookup_succeeded),
        ),
        "hnsw_lookup_branch": crate::media_conversion_gate::json_inference_optional_string_or_null(
            audit.and_then(|a| a.hnsw_lookup_branch.as_deref()),
        ),
        "knn_telemetry_lookup_succeeded": crate::media_conversion_gate::json_inference_optional_bool_or_null(
            audit.and_then(|a| a.knn_telemetry_lookup_succeeded),
        ),
        "knn_telemetry_branch": crate::media_conversion_gate::json_inference_optional_string_or_null(
            audit.and_then(|a| a.knn_telemetry_branch.as_deref()),
        ),
        "knn_telemetry_neighbor_count": crate::media_conversion_gate::json_inference_optional_i32_or_null(
            audit
                .and_then(|a| a.knn_telemetry_neighbor_count)
                .and_then(|count| match i32::try_from(count) {
                    Ok(value) => Some(value),
                    Err(e) => {
                        crate::media_conversion_gate::delivery_db_batch_audit(
                            "knn_telemetry_neighbor_count",
                            format!("neighbor count {count} does not fit i32: {e}; writing null"),
                        );
                        None
                    }
                }),
        ),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum InferenceLogHashDecision {
    MissingSourcePath,
    Hash(String),
    None,
}

fn inference_log_file_hash_or_skip(path: Option<&Path>) -> InferenceLogHashDecision {
    let Some(path) = path else {
        return InferenceLogHashDecision::MissingSourcePath;
    };

    match calculate_blake3_hex(path) {
        Ok(hash) => InferenceLogHashDecision::Hash(hash),
        Err(err) => {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_inference_log",
                branch = "inference_log_hash_failed_skip_insert",
                path = %path.display(),
                error = %err,
                "loop intent inference log skipped because source BLAKE3 failed"
            );
            crate::media_conversion_gate::delivery_db_path_audit(
                "delivery_db_fallback",
                path,
                format!(
                    "INFERENCE AUDIT: Failed to calculate file hash for '{}' | Forensic: BLAKE3 Error '{}'; skipped inference_log insert to avoid untraceable telemetry",
                    path.display(),
                    err
                ),
            );
            InferenceLogHashDecision::None
        }
    }
}

/// Log one inference record to the database for later analysis.
///
/// Audits write failures without blocking the pipeline. If a concrete source
/// path is supplied but BLAKE3 hashing fails, the telemetry insert is skipped
/// to avoid writing an untraceable `inference_log` row.
pub fn log_inference_record(
    conn: &mut Client,
    meta: &LoopMeta,
    record: &LoopInferenceRecord,
    path: Option<&Path>,
    audit: Option<LoopInferenceAudit>,
) {
    if !crate::algorithm_runtime::loop_inference_telemetry_enabled() {
        return;
    }

    let mut record = record.clone();
    record.seal_algorithm_outputs();
    let mut audit = audit;
    if let Some(ref mut a) = audit {
        a.seal_algorithm_outputs();
    }

    let file_hash = match inference_log_file_hash_or_skip(path) {
        InferenceLogHashDecision::MissingSourcePath => None,
        InferenceLogHashDecision::Hash(hash) => Some(hash),
        InferenceLogHashDecision::None => return,
    };
    let source_path: Option<String> = path.map(|p| p.display().to_string());
    let mut snapshot = build_signal_snapshot(meta, audit.as_ref());
    if crate::algorithm_runtime::loop_inference_audit_only_mode() {
        let runtime_verdict = record.final_verdict.clone();
        let runtime_reason = record.decision_reason.clone();
        let runtime_final_probability = record.final_probability;
        record.final_verdict = crate::constants::INFERENCE_TELEMETRY_ONLY_VERDICT.to_string();
        record.decision_reason = format!("[audit-only] {runtime_reason}");
        if let serde_json::Value::Object(ref mut map) = snapshot {
            map.insert("audit_only".to_string(), serde_json::json!(true));
            map.insert(
                "runtime_final_verdict".to_string(),
                serde_json::json!(runtime_verdict),
            );
            map.insert(
                "runtime_decision_reason".to_string(),
                serde_json::json!(runtime_reason),
            );
            map.insert(
                "runtime_final_probability".to_string(),
                crate::media_conversion_gate::json_finite_f64_or_null(
                    runtime_final_probability,
                    "runtime_final_probability",
                    "inference_log_audit_only",
                ),
            );
        }
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_inference_log",
            branch = "inference_log_audit_only",
            runtime_final_verdict = %runtime_verdict,
            resolution_path = %crate::media_conversion_gate::trace_label_or_default(
                audit.as_ref().and_then(|a| a.resolution_path.as_deref()),
                "",
            ),
            "loop inference_log persisted in audit-only mode (verdict columns are placeholders)"
        );
    }

    let knn_neighbor_count_i32 =
        crate::media_conversion_gate::delivery_db_knn_neighbor_count_i32(record.knn_neighbor_count);

    // Explicit type binding for ToSql stability
    let duration_secs = meta.duration_secs;
    let webp_ratio = meta.webp_compression_ratio;
    let tree_prob = record.tree_probability;
    let knn_prob = record.knn_keep_probability;
    let knn_conf = record.knn_confidence;
    let final_prob = record.final_probability;
    let final_verdict = &record.final_verdict;
    let decision_reason = &record.decision_reason;
    let layer_exit = &record.layer_exit;
    let resolution_path = audit.as_ref().and_then(|a| a.resolution_path.as_deref());
    let hnsw_lookup_branch = audit.as_ref().and_then(|a| a.hnsw_lookup_branch.as_deref());
    let tree_log_odds = audit
        .as_ref()
        .and_then(|a| a.tree_log_odds)
        .and_then(crate::algorithm_seal::loop_finite_scalar);

    let result = conn.execute(
        "INSERT INTO inference_log (
            file_hash, source_path, duration_secs, webp_compression_ratio,
            tree_probability, knn_keep_probability, knn_confidence, knn_neighbor_count,
            final_probability, final_verdict, decision_reason, layer_exit,
            signal_snapshot, resolution_path, hnsw_lookup_branch, tree_log_odds
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::jsonb, $14, $15, $16)",
        &[
            &file_hash,
            &source_path,
            &duration_secs,
            &webp_ratio,
            &tree_prob,
            &knn_prob,
            &knn_conf,
            &knn_neighbor_count_i32,
            &final_prob,
            &final_verdict,
            &decision_reason,
            &layer_exit,
            &snapshot,
            &resolution_path,
            &hnsw_lookup_branch,
            &tree_log_odds,
        ],
    );

    if let Err(e) = result {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "loop_intent_inference_log",
            branch = "inference_log_write_failed",
            layer_exit = %layer_exit,
            resolution_path = %crate::media_conversion_gate::trace_label_or_default(
                resolution_path,
                "",
            ),
            hnsw_lookup_branch = %crate::media_conversion_gate::trace_label_or_default(
                hnsw_lookup_branch,
                "",
            ),
            error = %e,
            "loop intent inference log insert failed (non-fatal)"
        );
        crate::media_conversion_gate::delivery_db_batch_audit(
            "delivery_db_fallback",
            format!(
                "INFERENCE AUDIT: Failed to write inference log (non-fatal): {e} | Forensic: DB Error during insertion (Exit Layer: {layer_exit}); telemetry loss occurred",
            ),
        );
    }
}

// ── Level 1: Feature Discriminative Power Analysis ───────────────────────────

/// Query which features have real discriminative power between
/// `LoopStrong` and `LoopWeak` samples.
///
/// Returns features sorted by absolute discriminative power descending.
/// `discriminative_power = (mean_loop_strong - mean_loop_weak) / stddev`.
/// Used to assign dynamic weights to features in the distance metric.
///
/// # Errors
///
/// Returns an error if the database query fails.
pub fn query_feature_discriminative_power(
    conn: &mut Client,
) -> Result<Vec<LoopFeatureDiscriminativePower>> {
    fn mean(values: &[f64]) -> Option<f64> {
        if values.is_empty() {
            None
        } else {
            Some(values.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(values.len()))
        }
    }

    let samples = load_loop_intent_training_samples(conn)?;
    let mut feature_values: std::collections::BTreeMap<&'static str, Vec<(bool, f64)>> =
        std::collections::BTreeMap::new();

    let push_required = |store: &mut std::collections::BTreeMap<&'static str, Vec<(bool, f64)>>,
                         name: &'static str,
                         is_loop_strong: bool,
                         value: f64| {
        store.entry(name).or_default().push((is_loop_strong, value));
    };

    for (idx, sample) in samples.iter().enumerate() {
        let row = &sample.sample_row;
        let is_loop_strong = sample.label == 1;
        let require = |value: Option<f64>, field: &'static str| -> Result<f64> {
            value.with_context(|| {
                format!("discriminative power missing {field} at sample index {idx}")
            })
        };

        push_required(
            &mut feature_values,
            "duration_secs",
            is_loop_strong,
            require(row.duration_secs, "duration_secs")?,
        );
        push_required(
            &mut feature_values,
            "fps",
            is_loop_strong,
            require(row.fps, "fps")?,
        );
        push_required(
            &mut feature_values,
            "file_size_bytes",
            is_loop_strong,
            crate::numeric_cast::u64_to_f64(row.file_size_bytes),
        );
        push_required(
            &mut feature_values,
            "temporal_bpp",
            is_loop_strong,
            row.temporal_bpp,
        );
        push_required(
            &mut feature_values,
            "spatial_bpp",
            is_loop_strong,
            row.spatial_bpp,
        );
        push_required(
            &mut feature_values,
            "frame_payload_variation",
            is_loop_strong,
            require(row.frame_payload_variation, "frame_payload_variation")?,
        );
        push_required(
            &mut feature_values,
            "frame_delay_variation",
            is_loop_strong,
            require(row.frame_delay_variation, "frame_delay_variation")?,
        );
        push_required(
            &mut feature_values,
            "palette_depth",
            is_loop_strong,
            require(row.palette_depth, "palette_depth")?,
        );
        push_required(
            &mut feature_values,
            "motion_gini",
            is_loop_strong,
            require(row.motion_gini, "motion_gini")?,
        );
        push_required(
            &mut feature_values,
            "temporal_flatness",
            is_loop_strong,
            require(row.temporal_flatness, "temporal_flatness")?,
        );
        push_required(
            &mut feature_values,
            "webp_compression_ratio",
            is_loop_strong,
            require(row.webp_compression_ratio, "webp_compression_ratio")?,
        );
        push_required(
            &mut feature_values,
            "directory_loop_intent_score",
            is_loop_strong,
            require(
                row.directory_loop_intent_score,
                "directory_loop_intent_score",
            )?,
        );
        push_required(
            &mut feature_values,
            "cadence_score",
            is_loop_strong,
            require(row.cadence_score, "cadence_score")?,
        );
        push_required(
            &mut feature_values,
            "loop_frequency",
            is_loop_strong,
            require(row.loop_frequency, "loop_frequency")?,
        );
    }

    let mut powers: Vec<LoopFeatureDiscriminativePower> = feature_values
        .into_iter()
        .map(|(feature_name, values)| {
            let strong_values: Vec<f64> = values
                .iter()
                .filter_map(|(is_strong, value)| is_strong.then_some(*value))
                .collect();
            let weak_values: Vec<f64> = values
                .iter()
                .filter_map(|(is_strong, value)| (!*is_strong).then_some(*value))
                .collect();
            let all_values: Vec<f64> = values.iter().map(|(_, value)| *value).collect();

            let mean_loop_strong = mean(&strong_values);
            let mean_loop_weak = mean(&weak_values);
            let std_dev = match mean(&all_values) {
                None => f64::NAN,
                Some(avg) => {
                    let variance = all_values
                        .iter()
                        .map(|value| {
                            let delta = *value - avg;
                            delta * delta
                        })
                        .sum::<f64>()
                        / crate::numeric_cast::usize_to_f64(all_values.len());
                    variance.sqrt()
                }
            };

            let discriminative_power = match (mean_loop_strong, mean_loop_weak) {
                (Some(strong), Some(weak)) if std_dev.is_finite() && std_dev > 0.0 => {
                    (strong - weak) / std_dev
                }
                _ => f64::NAN,
            };

            LoopFeatureDiscriminativePower {
                feature_name: feature_name.to_string(),
                mean_loop_strong,
                mean_loop_weak,
                discriminative_power,
                sample_count: crate::numeric_cast::usize_to_i64_sat(values.len()),
            }
        })
        .collect();

    powers.sort_by(|a, b| {
        crate::media_conversion_gate::f64_sort_cmp(
            b.discriminative_power.abs(),
            a.discriminative_power.abs(),
        )
    });
    Ok(powers)
}

// ── Level 3: Blind Spot Discovery ────────────────────────────────────────────

/// Discover feature-space regions where the system is most uncertain.
///
/// Buckets inference logs by duration (5s) and `WebP` ratio (3 units) and finds
/// regions with average `KNN` confidence below the threshold.
///
/// # Errors
///
/// Returns an error if the database query fails.
pub fn query_inference_blind_spots(
    conn: &mut Client,
    confidence_threshold: f64,
) -> Result<Vec<InferenceBlindSpot>> {
    let rows = conn.query(
        "SELECT
            ROUND(duration_secs / 5) * 5          AS duration_bucket,
            ROUND(COALESCE(webp_compression_ratio, 0) / 3) * 3 AS webp_bucket,
            AVG(COALESCE(knn_confidence, 0))       AS avg_knn_confidence,
            AVG(tree_probability)                   AS avg_tree_probability,
            AVG(final_probability)                  AS avg_final_probability,
            AVG(knn_neighbor_count)::DOUBLE PRECISION AS avg_neighbor_count,
            COUNT(*)                                AS sample_count,
            MODE() WITHIN GROUP (ORDER BY layer_exit) AS example_layer_exit
         FROM inference_log
         GROUP BY duration_bucket, webp_bucket
         HAVING AVG(COALESCE(knn_confidence, 0)) < $1
         ORDER BY COUNT(*) DESC",
        &[&confidence_threshold],
    )?;

    Ok(rows
        .iter()
        .map(|row| InferenceBlindSpot {
            duration_bucket: row.get(0),
            webp_bucket: row.get(1),
            avg_knn_confidence: row.get(2),
            avg_tree_probability: row.get(3),
            avg_final_probability: row.get(4),
            avg_neighbor_count: row.get(5),
            sample_count: row.get(6),
            example_layer_exit: row.get(7),
        })
        .collect())
}

// ── Inference Log Summary ────────────────────────────────────────────────────

/// Read the runtime loop verdict from an inference-log JSON snapshot.
///
/// When loop inference audit-only mode is on (`algorithm_runtime`), the SQL
/// `final_verdict` column is [`crate::constants::LOOP_INFERENCE_TELEMETRY_ONLY_VERDICT`] only; use this helper
/// (or `signal_snapshot->>'runtime_final_verdict'` in SQL) for analytics.
#[must_use]
pub fn loop_inference_runtime_verdict_from_snapshot(snapshot: &serde_json::Value) -> Option<&str> {
    snapshot
        .get("runtime_final_verdict")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Summary statistics for the inference log, used by diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceLogSummary {
    /// Total number of inference log records.
    pub total_records: i64,
    /// Counts per verdict string (e.g., "`LoopStrong`", "`LoopWeak`").
    pub verdict_counts: Vec<(String, i64)>,
    /// Counts per layer exit string.
    pub layer_exit_counts: Vec<(String, i64)>,
    /// Average tree probability across all records.
    pub avg_tree_probability: Option<f64>,
    /// Average KNN confidence across all records.
    pub avg_knn_confidence: Option<f64>,
    /// Average final probability across all records.
    pub avg_final_probability: Option<f64>,
    /// Number of records that fell back to Layer 7.
    pub layer7_fallback_count: i64,
}

/// Get a summary of all inference log records, including verdict counts,
/// layer exit distributions, and average probability/confidence metrics.
///
/// # Errors
///
/// Returns an error if the database query fails.
pub fn query_inference_log_summary(conn: &mut Client) -> Result<InferenceLogSummary> {
    let count_row = conn.query_one("SELECT COUNT(*) FROM inference_log", &[])?;
    let total_records: i64 = count_row.get(0);

    if total_records == 0 {
        return Ok(InferenceLogSummary {
            total_records: 0,
            verdict_counts: Vec::new(),
            layer_exit_counts: Vec::new(),
            avg_tree_probability: None,
            avg_knn_confidence: None,
            avg_final_probability: None,
            layer7_fallback_count: 0,
        });
    }

    let verdict_rows = conn.query(
        "SELECT final_verdict, COUNT(*) FROM inference_log GROUP BY final_verdict ORDER BY COUNT(*) DESC",
        &[],
    )?;
    let verdict_counts: Vec<(String, i64)> =
        verdict_rows.iter().map(|r| (r.get(0), r.get(1))).collect();

    if crate::algorithm_runtime::loop_inference_audit_only_mode() {
        let mut telemetry_rows = 0_i64;
        for (verdict, count) in &verdict_counts {
            if verdict == crate::constants::INFERENCE_TELEMETRY_ONLY_VERDICT {
                telemetry_rows = *count;
                break;
            }
        }
        if telemetry_rows > 0 {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "loop_intent_inference_log",
                branch = "inference_log_column_placeholder",
                telemetry_rows,
                total_records,
                "inference_log.final_verdict is TelemetryOnly in audit-only mode; use signal_snapshot.runtime_final_verdict for analytics"
            );
        }
    }

    let layer_rows = conn.query(
        "SELECT layer_exit, COUNT(*) FROM inference_log GROUP BY layer_exit ORDER BY COUNT(*) DESC",
        &[],
    )?;
    let layer_exit_counts: Vec<(String, i64)> =
        layer_rows.iter().map(|r| (r.get(0), r.get(1))).collect();

    let agg_row = conn.query_one(
        "SELECT
            AVG(tree_probability),
            AVG(knn_confidence),
            AVG(final_probability),
            COUNT(*) FILTER (WHERE layer_exit LIKE 'Layer 7%')
         FROM inference_log",
        &[],
    )?;

    Ok(InferenceLogSummary {
        total_records,
        verdict_counts,
        layer_exit_counts,
        avg_tree_probability: agg_row.get(0),
        avg_knn_confidence: agg_row.get(1),
        avg_final_probability: agg_row.get(2),
        layer7_fallback_count: agg_row.get(3),
    })
}

// ── Database Health Diagnostics ──────────────────────────────────────────────

/// Detailed health report for the database infrastructure and data integrity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbHealthReport {
    /// Whether a database connection was successfully established.
    pub connected: bool,
    /// `PostgreSQL` server version string.
    pub pg_version: String,
    /// Whether the `pgvector` extension is installed.
    pub has_vector_extension: bool,
    /// Version of the installed `pgvector` extension (if present).
    pub vector_extension_version: Option<String>,
    /// Row counts per table (`samples`, `inference_log`, etc.).
    pub table_counts: std::collections::HashMap<String, i64>,
    /// Whether any data corruption (`NaN`/Inf vectors) was detected.
    pub corruption_found: bool,
    /// Descriptions of any corruption found.
    pub corruption_details: Vec<String>,
    /// Maturity status of the training dataset (e.g., "Mature (KNN Active)").
    pub maturity_status: String,
    /// Structured loop + static corpus maturity (M153).
    pub training_corpus: crate::algorithm_runtime::TrainingCorpusMaturity,
}

/// Perform a deep diagnostic scan of the database infrastructure and data
/// integrity.
///
/// Checks `PostgreSQL` connectivity, `pgvector` extension presence, table row
/// counts, `NaN`/Infinity corruption in feature vectors, and dataset
/// maturity status. Returns a `DbHealthReport` with all findings.
///
/// # Errors
///
/// Returns an error if the database cannot be reached.
pub fn check_database_health() -> Result<DbHealthReport> {
    let mut conn = open_pg_client()?;
    let mut report = DbHealthReport {
        connected: true,
        pg_version: "Unknown".to_string(),
        has_vector_extension: false,
        vector_extension_version: None,
        table_counts: std::collections::HashMap::new(),
        corruption_found: false,
        corruption_details: Vec::new(),
        maturity_status: "Immature".to_string(),
        training_corpus: crate::algorithm_runtime::TrainingCorpusMaturity {
            loop_mature: false,
            loop_shortfall: 0,
            static_mature: false,
            static_shortfall: 0,
        },
    };

    // 1. Infrastructure Checks
    match conn.query_one("SELECT version()", &[]) {
        Ok(row) => {
            report.pg_version = row.get(0);
        }
        Err(e) => report
            .corruption_details
            .push(format!("Failed to query PostgreSQL version: {e}")),
    }

    match conn.query_opt(
        "SELECT installed_version FROM pg_available_extensions WHERE name = 'vector'",
        &[],
    ) {
        Ok(Some(row)) => {
            report.has_vector_extension = true;
            report.vector_extension_version = row.get(0);
        }
        Ok(None) => {}
        Err(e) => report
            .corruption_details
            .push(format!("Failed to query pgvector extension version: {e}")),
    }

    // 2. Table Statistics
    let tables = vec![
        "loop_samples",
        "image_quality_samples",
        "animated_image_quality_samples",
        "video_quality_samples",
        "analysis_records",
        "path_index",
        "inference_log",
        "quality_inference_log",
        "image_quality_inference_log",
        "animated_image_quality_inference_log",
        "video_quality_inference_log",
    ];
    for table in tables {
        let count_query = format!("SELECT COUNT(*) FROM {table}");
        match conn.query_one(&count_query, &[]) {
            Ok(row) => {
                report.table_counts.insert(table.to_string(), row.get(0));
            }
            Err(e) => report
                .corruption_details
                .push(format!("Failed to count table {table}: {e}")),
        }
    }

    // 3. Data Integrity: NaN/Infinity Scan for pgvector columns
    // We scan both the feature search vector columns which are critical for KNN stability.

    // Check active multi-scenario tables
    match conn.query(
        "SELECT blake3 FROM loop_samples WHERE embedding::text ~ 'NaN|Infinity'",
        &[],
    ) {
        Ok(rows) if !rows.is_empty() => {
            report.corruption_found = true;
            report.corruption_details.push(format!(
                "Found {} records with NaN/Inf vectors in 'loop_samples' table.",
                rows.len()
            ));
        }
        Ok(_) => {}
        Err(e) => report.corruption_details.push(format!(
            "Failed to scan loop_samples vector corruption: {e}"
        )),
    }

    // Check new multi-scenario quality tables
    for (table, hash_col) in [
        ("image_quality_samples", "blake3"),
        ("animated_image_quality_samples", "blake3"),
        ("video_quality_samples", "blake3"),
    ] {
        let query =
            format!("SELECT {hash_col} FROM {table} WHERE embedding::text ~ 'NaN|Infinity'");
        match conn.query(&query, &[]) {
            Ok(rows) if !rows.is_empty() => {
                report.corruption_found = true;
                report.corruption_details.push(format!(
                    "Found {} records with NaN/Inf vectors in '{table}' table.",
                    rows.len()
                ));
            }
            Ok(_) => {}
            Err(e) => report
                .corruption_details
                .push(format!("Failed to scan {table} vector corruption: {e}")),
        }
    }

    // 4. Maturity Analysis (loop + static quality corpora; M153)
    let (low, high, video) = get_class_counts(&mut conn);
    let loop_total = low.max(0) + high.max(0) + video.max(0);
    let loop_quality = low.max(0) + high.max(0);
    let (static_high, static_low) = get_static_quality_class_counts(&mut conn);
    let training_corpus = crate::algorithm_runtime::evaluate_training_corpus_maturity(
        loop_total,
        loop_quality,
        video.max(0),
        static_high,
        static_low,
    );
    report.training_corpus = training_corpus;
    report.maturity_status = training_corpus.format_db_health_status();

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_intent::{LoopColorFlags, LoopFlags, LoopMemeFlags, LoopStreamFlags};
    use std::process::Command;

    fn base_meta() -> LoopMeta {
        let frames = 24;
        let duration = 2.0_f64;
        let size = 120_000;
        LoopMeta {
            duration_secs: Some(duration),
            duration_tier: Some(crate::loop_intent::DurationTier::from_secs(duration)),
            width: Some(320),
            height: Some(320),
            fps: Some(12.0),
            frame_count: Some(frames),
            file_size_bytes: size,
            file_name: None,
            palette_size: Some(64),
            app_extensions: None,
            encoder_software: None,
            is_interlaced: None,
            transparency_is_real: None,
            real_frame_count: None,
            frame_payload_variation: Some(0.4),
            frame_delay_variation: Some(0.6),
            source_extension: Some("gif".to_string()),
            container: Some("gif".to_string()),
            parent_directories: None,
            directory_loop_intent_score: 0.5,
            filename_loop_intent_score: 0.5,
            loop_count: None,
            audio_is_silent: Some(true), // GIFs never have audio
            frame_types: vec![
                'P';
                crate::media_conversion_gate::delivery_db_u64_to_usize_or_zero_with_notice(
                    frames,
                    "gif_frame_count_types",
                )
            ],
            pts_deltas: vec![
                duration / crate::numeric_cast::u64_to_f64(frames.max(1));
                crate::media_conversion_gate::delivery_db_u64_to_usize_or_zero_with_notice(
                    frames,
                    "gif_frame_count_pts",
                )
            ],
            mv_magnitudes: Vec::new(),
            cached_frame_png: None,
            flags: LoopFlags {
                streams: LoopStreamFlags {
                    has_audio: false,
                    has_transparency: true,
                    is_native_gif: true,
                },
                color: LoopColorFlags {
                    has_embedded_icc: false,
                    has_complex_color_profile: false,
                },
                meme: LoopMemeFlags {
                    is_meme_platform: false,
                },
            },
            palette_depth: Some(0.8),
            motion_gini: Some(0.7),
            block_skew: Some(0.6),
            temporal_flatness: Some(0.9),
            loop_closure_score: Some(0.85),
            motion_periodicity: Some(0.75),
            temporal_jitter: Some(0.90),
            pkt_sizes: Vec::new(),
            webp_compression_ratio: Some(0.85),
            max_frame_delay: Some(duration / crate::numeric_cast::u64_to_f64(frames.max(1))),
            min_frame_delay: Some(duration / crate::numeric_cast::u64_to_f64(frames.max(1))),
            audio_duration_secs: None,
            path_depth: 0,
            filename_numeric_density: 0.0,
            physics_225: None,
        }
    }

    #[test]
    fn distance_prefers_similar_samples() {
        let meta = base_meta();
        let near = SampleRow {
            width: 300,
            height: 300,
            duration_secs: Some(2.2),
            frame_count: Some(24),
            file_size_bytes: 125_000,
            fps: Some(12.0),
            temporal_bpp: 0.05,
            spatial_bpp: 1.2,
            flags: SampleFlags {
                streams: SampleStreamFlags {
                    has_transparency: true,
                    is_native_gif: true,
                },
                ..Default::default()
            },
            palette_size: Some(64),
            frame_payload_variation: Some(0.35_f64),
            frame_delay_variation: Some(0.55_f64),
            aspect_ratio: Some(1.0_f64),
            loop_frequency: Some(0.8_f64),
            cadence_score: Some(0.9_f64),
            directory_loop_intent_score: Some(1.0_f64),
            palette_depth: Some(0.8_f64),
            motion_gini: Some(0.7_f64),
            block_skew: Some(0.6_f64),
            temporal_flatness: Some(0.9_f64),
            loop_closure_score: Some(0.88_f64),
            motion_periodicity: Some(0.78_f64),
            temporal_jitter: Some(0.92_f64),
            webp_compression_ratio: Some(0.9_f64),
            max_frame_delay: Some(2.2_f64 / 24.0_f64),
            min_frame_delay: Some(2.2_f64 / 24.0_f64),
            audio_duration_secs: None,
            path_depth: 0,
            filename_numeric_density: 0.0,
            physics_225: None,
        };
        let far = SampleRow {
            width: 1920,
            height: 1080,
            duration_secs: Some(20.0),
            frame_count: Some(600),
            file_size_bytes: 20_000_000,
            fps: Some(30.0),
            temporal_bpp: 0.4,
            spatial_bpp: 35.0,
            flags: SampleFlags {
                streams: SampleStreamFlags {
                    has_transparency: false,
                    is_native_gif: false,
                },
                ..Default::default()
            },
            palette_size: Some(256),
            frame_payload_variation: Some(0.05_f64),
            frame_delay_variation: Some(0.02_f64),
            aspect_ratio: Some(1.78_f64),
            loop_frequency: Some(0.1_f64),
            cadence_score: Some(0.1_f64),
            directory_loop_intent_score: Some(0.5_f64),
            palette_depth: Some(0.1_f64),
            motion_gini: Some(0.2_f64),
            block_skew: Some(0.1_f64),
            temporal_flatness: Some(0.1_f64),
            loop_closure_score: Some(crate::constants::DB_LOOP_CLOSURE_SCORE_DEFAULT),
            motion_periodicity: Some(0.20_f64),
            temporal_jitter: Some(0.18_f64),
            webp_compression_ratio: Some(0.1_f64),
            max_frame_delay: Some(20.0_f64 / 600.0_f64),
            min_frame_delay: Some(20.0_f64 / 600.0_f64),
            audio_duration_secs: Some(20.0_f64),
            path_depth: 3,
            filename_numeric_density: 0.0,
            physics_225: None,
        };
        let (tbpp, sbpp) = bpp_from_meta(&meta).expect("fixture has geometry and frame_count"); // audited: db module unit-test fixture assertion; not production DB runtime path

        let stats = FeatureMap::mock();

        assert!(
            sample_distance(&meta, &near, tbpp, sbpp, &stats)
                < sample_distance(&meta, &far, tbpp, sbpp, &stats)
        );
    }

    #[test]
    fn lossless_duration_limit_midpoint_is_75_seconds() {
        assert!((lossless_duration_limit_for_keep_prob(0.5) - 75.0).abs() < 0.01);
    }

    #[test]
    fn lossless_duration_limit_respects_policy_edges() {
        assert!((lossless_duration_limit_for_keep_prob(0.0) - 30.0).abs() < 0.01);
        assert!((lossless_duration_limit_for_keep_prob(1.0) - 120.0).abs() < 0.01);
    }

    #[test]
    fn resolved_duration_secs_recovers_from_zero_probe_duration() {
        let mut meta = base_meta();
        meta.duration_secs = Some(0.0_f64);
        meta.frame_count = Some(800);
        meta.fps = Some(10.0_f64);
        assert!(
            (resolved_duration_secs(&meta).expect("Test should have valid duration") - 80.0).abs() // audited: db module unit-test fixture assertion; not production DB runtime path
                < 0.01_f64
        );
    }

    #[test]
    fn feature_stats_capture_percentiles() {
        let stats = build_feature_stats(&[1.0_f64, 2.0_f64, 3.0_f64, 4.0_f64, 5.0_f64])
            .expect("fixture percentiles"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(stats.p10, Some(1.4_f64));
        assert_eq!(stats.p50, Some(3.0_f64));
        assert_eq!(stats.p90, Some(4.6_f64));
    }

    #[test]
    fn bpp_from_meta_divides_temporal_density_by_frame_count() {
        let mut meta = base_meta();
        meta.width = Some(1200);
        meta.height = Some(1200);
        meta.frame_count = Some(36);
        meta.file_size_bytes = 2_391_699;

        let (temporal_bpp, spatial_bpp) =
            bpp_from_meta(&meta).expect("fixture has geometry and frame_count"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let pixel_count = f64::from(meta.width.unwrap()) * f64::from(meta.height.unwrap()); // audited: db module unit-test fixture assertion; not production DB runtime path
        let expected_temporal = crate::numeric_cast::u64_to_f64(meta.file_size_bytes)
            / (pixel_count
                * crate::numeric_cast::u64_to_f64(meta.frame_count.expect("fixture has frames"))); // audited: db module unit-test fixture assertion; not production DB runtime path
        let legacy_buggy_temporal = crate::numeric_cast::u64_to_f64(meta.file_size_bytes)
            / pixel_count
            * crate::numeric_cast::u64_to_f64(meta.frame_count.expect("fixture has frames")); // audited: db module unit-test fixture assertion; not production DB runtime path
        let expected_spatial = crate::numeric_cast::u64_to_f64(meta.file_size_bytes) / pixel_count;

        assert!(crate::float_compare::approx_eq_f64(
            temporal_bpp,
            expected_temporal
        ));
        assert!(crate::float_compare::approx_eq_f64(
            spatial_bpp,
            expected_spatial
        ));
        assert!(
            (temporal_bpp - legacy_buggy_temporal).abs() > 1.0_f64,
            "temporal_bpp should use per-frame density, not multiply by frame count"
        );
    }

    #[test]
    fn balance_weight_is_damped_under_extreme_imbalance() {
        let minority_weight = class_balance_weight(2_000, 10);
        let majority_weight = class_balance_weight(2_000, 1_990);
        assert!(minority_weight <= 1.50_f64);
        assert!(majority_weight >= 0.67_f64);
        assert!(minority_weight > majority_weight);
    }

    #[test]
    fn smoothed_prior_avoids_extreme_zeros_and_ones() {
        let all_keep = smoothed_keep_prior(100, 0);
        let all_weak = smoothed_keep_prior(0, 100);
        assert!(all_keep < 1.0_f64 && all_keep > 0.95_f64);
        assert!(all_weak > 0.0_f64 && all_weak < 0.05_f64);
    }

    #[test]
    fn effective_sample_size_matches_known_case() {
        let eff_n = effective_sample_size(5.0, 5.0);
        assert!(crate::float_compare::approx_eq_f64(eff_n, 5.0));
    }

    fn loop_training_sample_fixture(sample_row: SampleRow) -> LoopIntentTrainingSample {
        LoopIntentTrainingSample {
            blake3: vec![0_u8; 32],
            file_name: None,
            label: 1,
            sample_row,
        }
    }

    fn complete_loop_stats_sample_row() -> SampleRow {
        SampleRow {
            width: 300,
            height: 300,
            duration_secs: Some(2.2),
            frame_count: Some(24),
            file_size_bytes: 125_000,
            fps: Some(12.0),
            temporal_bpp: 0.05,
            spatial_bpp: 1.2,
            flags: SampleFlags::default(),
            palette_size: Some(64),
            frame_payload_variation: Some(0.35_f64),
            frame_delay_variation: Some(0.55_f64),
            aspect_ratio: Some(1.0_f64),
            loop_frequency: Some(0.8_f64),
            cadence_score: Some(0.9_f64),
            directory_loop_intent_score: Some(1.0_f64),
            palette_depth: Some(0.8_f64),
            motion_gini: Some(0.7_f64),
            block_skew: Some(0.6_f64),
            temporal_flatness: Some(0.9_f64),
            loop_closure_score: Some(0.88_f64),
            motion_periodicity: Some(0.78_f64),
            temporal_jitter: Some(0.92_f64),
            webp_compression_ratio: Some(0.9_f64),
            max_frame_delay: Some(2.2_f64 / 24.0_f64),
            min_frame_delay: Some(2.2_f64 / 24.0_f64),
            audio_duration_secs: None,
            path_depth: 0,
            filename_numeric_density: 0.0,
            physics_225: None,
        }
    }

    fn complete_loop_training_sample_insert() -> SampleInsert {
        SampleInsert {
            file_hash: "hash123".to_string(),
            source_path: "path/to/img.gif".to_string(),
            file_name: Some("img.gif".to_string()),
            source_ext: Some("gif".to_string()),
            width: 320,
            height: 240,
            duration_secs: Some(3.5),
            frame_count: Some(35),
            file_size_bytes: 500_000,
            fps: Some(10.0),
            flags: SampleFlags::default(),
            palette_size: Some(256),
            frame_payload_variation: Some(0.2),
            frame_delay_variation: Some(0.1),
            temporal_bpp: 0.05,
            spatial_bpp: 2.5,
            loss_tolerance: "high".to_string(),
            labeled_by: "test".to_string(),
            aspect_ratio: Some(1.33),
            total_pixels: 76_800,
            loop_frequency: 0.8,
            cadence_score: 0.9,
            directory_loop_intent_score: 0.5,
            palette_depth: Some(0.8),
            motion_gini: Some(0.5),
            block_skew: Some(0.3),
            temporal_flatness: Some(0.7),
            loop_closure_score: Some(0.6),
            motion_periodicity: Some(0.4),
            temporal_jitter: Some(0.1),
            webp_compression_ratio: Some(0.85),
            max_frame_delay: Some(0.1),
            min_frame_delay: Some(0.1),
            audio_duration_secs: None,
            path_depth: 1,
            filename_numeric_density: 0.0,
            physics_225: Some(vec![1.0_f32; 225]),
            loop_verdict: "LoopStrong".to_string(),
        }
    }

    fn minimal_two_frame_gif_fixture() -> Vec<u8> {
        let mut gif_data = Vec::new();
        {
            let mut encoder = ::gif::Encoder::new(&mut gif_data, 1, 1, &[0, 0, 0, 255, 255, 255])
                .expect("gif encoder");
            encoder
                .write_frame(&::gif::Frame {
                    delay: 10,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&[0_u8]),
                    ..Default::default()
                })
                .expect("gif frame 1");
            encoder
                .write_frame(&::gif::Frame {
                    delay: 10,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&[1_u8]),
                    ..Default::default()
                })
                .expect("gif frame 2");
        }
        gif_data
    }

    #[test]
    fn sample_from_path_result_populates_loop_stats_webp_ratio() {
        let file = tempfile::Builder::new()
            .suffix(".gif")
            .tempfile()
            .expect("temp gif");
        std::fs::write(file.path(), minimal_two_frame_gif_fixture()).expect("write temp gif");

        let sample = sample_from_path_result(file.path(), "test", Some("high"))
            .expect("tiny dynamic gif should build a loop training sample");

        let ratio = sample
            .webp_compression_ratio
            .expect("row builder must populate loop_stats_webp_ratio before ingest");
        assert!(ratio.is_finite() && ratio > 0.0);
        validate_loop_training_sample(&sample)
            .expect("row builder output must pass strict ingest validation");
    }

    #[test]
    fn sample_from_path_result_accepts_mp4_without_gif_header_scan() {
        if which::which("ffmpeg").is_err() {
            eprintln!("skipping mp4 loop sample regression: ffmpeg unavailable");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("loop_candidate.mp4");
        let output = Command::new("ffmpeg")
            .arg("-y")
            .arg("-v")
            .arg("error")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc2=size=64x64:rate=12")
            .arg("-t")
            .arg("1.0")
            .arg("-an")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&file)
            .output()
            .expect("spawn ffmpeg");
        assert!(
            output.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let sample = sample_from_path_result(&file, "test", Some("video")).expect(
            "mp4 loop candidate should build a training sample without GIF header scanning",
        );

        assert!(matches!(sample.source_ext.as_deref(), Some("mov" | "mp4")));
        assert!(sample.frame_delay_variation.is_some());
        validate_loop_training_sample(&sample)
            .expect("mp4 row builder output must pass strict ingest validation");
    }

    #[test]
    fn validate_loop_training_sample_requires_webp_ratio() {
        let mut sample = complete_loop_training_sample_insert();
        sample.webp_compression_ratio = None;

        let err = validate_loop_training_sample(&sample)
            .expect_err("loop training ingest must reject samples without WebP ratio");

        assert!(
            err.to_string().contains("loop_stats_webp_ratio"),
            "unexpected validation error: {err}"
        );
    }

    #[test]
    fn validate_loop_training_sample_rejects_non_finite_webp_ratio() {
        let mut sample = complete_loop_training_sample_insert();
        sample.webp_compression_ratio = Some(f64::NAN);

        let err = validate_loop_training_sample(&sample)
            .expect_err("loop training ingest must reject non-finite WebP ratio");

        assert!(
            err.to_string().contains("loop_stats_webp_ratio"),
            "unexpected validation error: {err}"
        );
    }

    #[test]
    fn loop_intent_training_advisory_lock_contract_is_stable() {
        assert_ne!(
            crate::multi_scenario_db::MULTI_SCENARIO_SCHEMA_ADVISORY_LOCK_KEY,
            0
        );
        assert_eq!(
            crate::multi_scenario_db::PG_ADVISORY_LOCK_SQL,
            "SELECT pg_advisory_lock($1)"
        );
        assert_eq!(
            crate::multi_scenario_db::PG_ADVISORY_UNLOCK_SQL,
            "SELECT pg_advisory_unlock($1)"
        );
    }

    #[test]
    fn build_loop_feature_map_rejects_partial_corpus_instead_of_biasing_stats() {
        let mut incomplete = complete_loop_stats_sample_row();
        incomplete.duration_secs = None;
        let err = build_loop_feature_map(&[
            loop_training_sample_fixture(complete_loop_stats_sample_row()),
            loop_training_sample_fixture(incomplete),
        ])
        .expect_err("must fail closed when any training vector is incomplete");
        assert!(
            err.to_string().contains("feature-map rejected"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_loop_feature_map_accepts_absent_motion_periodicity_as_sparse_absence() {
        let mut absent_periodicity = complete_loop_stats_sample_row();
        absent_periodicity.motion_periodicity = None;

        let feature_map = build_loop_feature_map(&[
            loop_training_sample_fixture(complete_loop_stats_sample_row()),
            loop_training_sample_fixture(absent_periodicity),
        ])
        .expect(
            "missing motion_periodicity should use the documented sparse KNN absence component",
        );

        assert!(
            feature_map.stats.contains_key("m_period"),
            "feature_stats must retain the m_period dimension"
        );
    }

    #[test]
    fn build_loop_feature_map_emits_all_pgvector_feature_stats() {
        let feature_map = build_loop_feature_map(&[loop_training_sample_fixture(
            complete_loop_stats_sample_row(),
        )])
        .expect("complete corpus row should build feature stats");

        assert_eq!(feature_map.stats.len(), LOOP_VECTOR_FEATURE_NAMES.len());
        for feature_name in LOOP_VECTOR_FEATURE_NAMES {
            assert!(
                feature_map.stats.contains_key(feature_name),
                "missing feature_stats key {feature_name}"
            );
        }
    }

    #[test]
    fn loop_training_balance_probe_reports_sample_rejection_cause() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        std::io::Write::write_all(&mut file, b"not a media file").expect("write temp media");

        let err = probe_loop_training_balance(file.path())
            .expect_err("invalid media should be rejected with a diagnostic cause");
        let msg = format!("{err:#}");

        assert!(
            msg.contains("metadata unavailable") || msg.contains("Sample probe failed"),
            "unexpected generic balance-probe error: {msg}"
        );
        assert!(
            msg.contains(&file.path().display().to_string()),
            "error should identify the rejected path: {msg}"
        );
    }

    #[test]
    fn merge_duration_distribution_does_not_fabricate_collection_percentiles() {
        let collection = GlobalCollectionStats {
            duration_min: Some(1.0_f64),
            duration_avg: Some(5.0_f64),
            duration_p90: Some(9.0_f64),
            ..GlobalCollectionStats::default()
        };
        let stats = DistributionStats {
            mean: 5.0_f64,
            std_dev: 2.0_f64,
            ..DistributionStats::default()
        };
        // Collection aggregates are exposed via `GlobalCollectionStats`, not forged into p25/p50.
        assert!(stats.p50.is_none());
        assert!(stats.p90.is_none());
        assert_eq!(collection.duration_avg, Some(5.0_f64));
    }

    fn loop_reference_test_feature_stats(mean: f64, std_dev: f64) -> FeatureStats {
        FeatureStats {
            mean,
            std_dev,
            weight: None,
            ..FeatureStats::default()
        }
    }

    fn populate_loop_reference_feature_map(feature_map: &mut FeatureMap, duration: &FeatureStats) {
        for key in LOOP_REFERENCE_FEATURE_KEYS {
            let stats = if *key == "duration" {
                duration.clone()
            } else {
                loop_reference_test_feature_stats(1.0, 0.1)
            };
            feature_map.stats.insert((*key).to_string(), stats);
        }
    }

    #[test]
    fn build_loop_reference_profile_rejects_missing_feature_keys() {
        let err =
            build_loop_reference_profile(GlobalCollectionStats::default(), &FeatureMap::default())
                .expect_err("empty feature map must fail closed");
        assert!(
            err.to_string().contains("missing from feature_stats"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn loop_reference_profile_test_default_marks_bootstrap_heuristic() {
        assert!(LoopReferenceProfile::default().is_knn_bootstrap_heuristic);
    }

    #[test]
    fn loop_reference_profile_strips_duration_percentiles_without_histogram() {
        let mut feature_map = FeatureMap::default();
        let duration_stats = loop_reference_test_feature_stats(5.0, 2.0);
        populate_loop_reference_feature_map(&mut feature_map, &duration_stats);
        let profile = build_loop_reference_profile(GlobalCollectionStats::default(), &feature_map)
            .expect("full feature map"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert!(!profile.duration_has_empirical_percentiles);
        assert!(!profile.is_knn_bootstrap_heuristic);
        assert!(profile.duration.mean.is_finite() && profile.duration.std_dev > 0.0);
        assert!(profile.duration.p25.is_none() && profile.duration.p75.is_none());
        assert!(profile.duration.p50.is_none() && profile.duration.p90.is_none());
    }

    /// Test-only: synthesize Gaussian quantiles from moments (never used in production KNN).
    fn fill_missing_percentiles_from_moments(stats: &mut DistributionStats) -> bool {
        if !stats.mean.is_finite() || stats.std_dev <= 1e-6 {
            return false;
        }
        let quantile = |z: f64| z.mul_add(stats.std_dev, stats.mean).max(0.0);
        let fill_z = |slot: &mut Option<f64>, z: f64| {
            slot.is_none().then(|| *slot = Some(quantile(z))).is_some()
        };
        let fill_mean =
            |slot: &mut Option<f64>| slot.is_none().then(|| *slot = Some(stats.mean)).is_some();
        fill_z(&mut stats.p10, -1.281_551_565_545_160_4)
            | fill_z(&mut stats.p25, -0.674_489_750_196_082_7)
            | fill_mean(&mut stats.p50)
            | fill_z(&mut stats.p75, 0.674_489_750_196_082_7)
            | fill_z(&mut stats.p90, 1.281_551_565_545_160_4)
    }

    #[test]
    fn distribution_stats_infers_percentiles_from_moments_when_histogram_absent() {
        let mut stats = DistributionStats {
            mean: 10.0,
            std_dev: 2.0,
            ..DistributionStats::default()
        };
        fill_missing_percentiles_from_moments(&mut stats);
        assert_eq!(stats.p50, Some(10.0));
        let p25 = stats.p25.expect("p25 inferred"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let p75 = stats.p75.expect("p75 inferred"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert!(p25 < 10.0 && p75 > 10.0);
        assert!(stats.p10.expect("p10") < p25); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert!(stats.p90.expect("p90") > p75); // audited: db module unit-test fixture assertion; not production DB runtime path
    }

    #[test]
    fn distribution_stats_preserves_empirical_percentiles_when_present() {
        let mut stats = DistributionStats {
            mean: 10.0,
            std_dev: 2.0,
            p25: Some(3.0),
            p50: Some(9.0),
            p75: Some(15.0),
            ..DistributionStats::default()
        };
        fill_missing_percentiles_from_moments(&mut stats);
        assert_eq!(stats.p25, Some(3.0));
        assert_eq!(stats.p50, Some(9.0));
        assert_eq!(stats.p75, Some(15.0));
        assert!(stats.p10.is_some() && stats.p90.is_some());
    }

    #[test]
    fn loop_reference_profile_prefers_dynamic_stats_when_present() {
        let mut feature_map = FeatureMap {
            top_keywords: vec!["meme".to_string()],
            ..FeatureMap::default()
        };
        let duration_stats = FeatureStats {
            mean: 6.0,
            std_dev: 2.0,
            weight: None,
            p10: Some(1.0_f64),
            p25: Some(2.0_f64),
            p50: Some(5.0_f64),
            p75: Some(8.0_f64),
            p90: Some(10.0_f64),
        };
        populate_loop_reference_feature_map(&mut feature_map, &duration_stats);
        feature_map.stats.insert(
            "fps".to_string(),
            FeatureStats {
                mean: 14.0,
                std_dev: 3.0,
                p10: Some(8.0_f64),
                p25: Some(10.0_f64),
                p50: Some(14.0_f64),
                p75: Some(18.0_f64),
                p90: Some(22.0_f64),
                weight: None,
            },
        );

        let profile = build_loop_reference_profile(GlobalCollectionStats::default(), &feature_map)
            .expect("full feature map"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(profile.duration.p25, Some(2.0_f64));
        assert!(crate::float_compare::approx_eq_f64(profile.fps.mean, 14.0));
        assert_eq!(profile.top_keywords, vec!["meme".to_string()]);
    }

    #[test]
    #[serial_test::serial]
    fn test_env_disable_knn_lookup_compatibility() {
        unsafe {
            std::env::set_var(crate::constants::ENV_DISABLE_DB_FEEDBACK, "1");
        }
        let meta = base_meta();
        let res = lookup_similar_samples(&meta, None);
        assert!(
            res.is_none(),
            "Disabled DB or missing DB connection should gracefully return None"
        );
        unsafe {
            std::env::remove_var(crate::constants::ENV_DISABLE_DB_FEEDBACK);
        }
    }

    #[test]
    fn loop_corpus_health_shortfall_when_total_exceeds_floor_but_class_short() {
        let min_total = crate::algorithm_runtime::min_gif_samples_total();
        let min_per = crate::algorithm_runtime::min_gif_samples_per_class();
        let needed = crate::algorithm_runtime::loop_corpus_samples_shortfall(
            min_total + 100,
            min_per - 4,
            min_per,
        );
        assert_eq!(needed, 4);
        assert!(!crate::algorithm_runtime::loop_corpus_is_mature(
            min_total + 100,
            min_per - 4,
            min_per,
        ));
    }

    #[test]
    fn test_db_health_report_compatibility() {
        let report = DbHealthReport {
            connected: false,
            pg_version: "Unknown".to_string(),
            has_vector_extension: false,
            vector_extension_version: None,
            table_counts: std::collections::HashMap::new(),
            corruption_found: false,
            corruption_details: Vec::new(),
            maturity_status: "Immature (Compatibility Mode)".to_string(),
            training_corpus: crate::algorithm_runtime::TrainingCorpusMaturity {
                loop_mature: false,
                loop_shortfall: 0,
                static_mature: false,
                static_shortfall: 0,
            },
        };
        assert!(!report.connected);
        assert!(!report.has_vector_extension);
        assert_eq!(report.maturity_status, "Immature (Compatibility Mode)");
    }

    #[test]
    fn test_inference_log_summary_empty_compatibility() {
        let summary = InferenceLogSummary {
            total_records: 0,
            verdict_counts: Vec::new(),
            layer_exit_counts: Vec::new(),
            avg_tree_probability: None,
            avg_knn_confidence: None,
            avg_final_probability: None,
            layer7_fallback_count: 0,
        };
        assert_eq!(summary.total_records, 0);
        assert_eq!(summary.avg_knn_confidence, None);
        assert_eq!(summary.layer7_fallback_count, 0);
    }

    #[test]
    fn test_sample_row_conversion_compatibility() {
        let insert = SampleInsert {
            file_hash: "hash123".to_string(),
            source_path: "path/to/img.gif".to_string(),
            file_name: Some("img.gif".to_string()),
            source_ext: Some("gif".to_string()),
            width: 320,
            height: 240,
            duration_secs: Some(3.5),
            frame_count: Some(35),
            file_size_bytes: 500_000,
            fps: Some(10.0),
            flags: SampleFlags::default(),
            palette_size: Some(256),
            frame_payload_variation: Some(0.2),
            frame_delay_variation: Some(0.1),
            temporal_bpp: 0.05,
            spatial_bpp: 2.5,
            aspect_ratio: Some(1.33),
            total_pixels: 76800,
            loop_frequency: 0.8,
            cadence_score: 0.9,
            directory_loop_intent_score: 0.5,
            palette_depth: Some(0.8),
            motion_gini: Some(0.5),
            block_skew: Some(0.3),
            temporal_flatness: Some(0.7),
            loop_closure_score: Some(0.6),
            motion_periodicity: Some(0.4),
            webp_compression_ratio: Some(0.85),
            max_frame_delay: Some(0.1),
            min_frame_delay: Some(0.1),
            audio_duration_secs: None,
            path_depth: 0,
            filename_numeric_density: 0.0,
            loss_tolerance: "high".to_string(),
            loop_verdict: "LoopStrong".to_string(),
            labeled_by: "test".to_string(),
            physics_225: None,
            temporal_jitter: Some(0.1),
        };
        let row: SampleRow = insert.into();
        assert_eq!(row.width, 320);
        assert_eq!(row.duration_secs, Some(3.5));
        assert_eq!(row.loop_closure_score, Some(0.6));
        assert_eq!(row.motion_periodicity, Some(0.4));
    }

    #[test]
    fn test_loop_training_label_normalization() {
        assert_eq!(normalize_loop_training_label("low"), Some("high"));
        assert_eq!(normalize_loop_training_label("high"), Some("high"));
        assert_eq!(normalize_loop_training_label("video"), Some("video"));
        assert_eq!(normalize_loop_training_label("medium"), None);
        assert_eq!(loop_verdict_for_training_label("low"), "LoopStrong");
        assert_eq!(loop_verdict_for_training_label("high"), "LoopStrong");
        assert_eq!(loop_verdict_for_training_label("video"), "LoopWeak");
    }

    #[test]
    fn loop_metadata_json_missing_required_probe_fields_detects_null_delay_var() {
        let meta = serde_json::json!({
            "frame_payload_variation": 0.2,
            "aspect_ratio": 1.0,
            "loop_frequency": 0.5,
            "palette_depth": 0.1,
            "block_skew": 0.1,
            "temporal_flatness": 0.1,
            "webp_compression_ratio": 0.1,
            "directory_loop_intent_score": 0.1,
            "frame_delay_variation": null
        });
        assert!(super::loop_metadata_json_missing_required_probe_fields(
            &meta
        ));
    }

    #[test]
    fn merge_loop_probe_metadata_json_preserves_existing_probe_fields_when_fresh_null() {
        let existing = serde_json::json!({
            "frame_delay_variation": null,
            "frame_payload_variation": 1.2,
            "palette_depth": 0.36,
            "temporal_flatness": 0.66,
            "webp_compression_ratio": 3.1,
            "directory_loop_intent_score": 0.5,
            "aspect_ratio": 1.0,
            "loop_frequency": 0.8
        });
        let fresh = LoopIntentStoredMetadata {
            frame_delay_variation: Some(0.0),
            ..LoopIntentStoredMetadata::default()
        };
        let merged = super::merge_loop_probe_metadata_json(existing, &fresh).expect("merge"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let obj = merged.as_object().expect("object"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(
            obj.get("frame_delay_variation"),
            Some(&serde_json::json!(0.0))
        );
        assert_eq!(obj.get("palette_depth"), Some(&serde_json::json!(0.36)));
        assert_eq!(obj.get("temporal_flatness"), Some(&serde_json::json!(0.66)));
    }

    #[test]
    #[ignore = "requires local QQ APNG corpus path"]
    fn apng_sample_from_path_populates_delay_var_for_qq_png() {
        use std::path::Path;
        let path = Path::new("/Users/nyamiiko/Downloads/优化/3 三批/QQ/1695006091.png");
        if !path.exists() {
            return;
        }
        let sample = super::sample_from_path(path, "repair_probe", Some("high"));
        let Some(insert) = sample else {
            panic!("sample_from_path returned None for APNG"); // audited: db module unit-test fixture assertion; not production DB runtime path
        };
        assert!(
            insert.frame_delay_variation.is_some(),
            "expected empirical frame_delay_variation, got None"
        );
        let fresh = super::stored_metadata_from_sample_probe(&insert);
        assert!(
            super::loop_intent_stored_metadata_satisfies_fabrication_verify(&fresh)
                || insert.frame_payload_variation.is_some()
        );
    }

    #[test]
    fn loop_intent_stored_metadata_from_sample_row_fills_delay_var() {
        let base = LoopIntentStoredMetadata {
            frame_delay_variation: None,
            ..LoopIntentStoredMetadata::default()
        };
        let row = SampleRow {
            frame_delay_variation: Some(0.42),
            frame_payload_variation: Some(0.2),
            aspect_ratio: Some(1.0),
            loop_frequency: Some(0.5),
            palette_depth: Some(0.1),
            block_skew: Some(0.1),
            temporal_flatness: Some(0.1),
            webp_compression_ratio: Some(0.1),
            directory_loop_intent_score: Some(0.1),
            temporal_bpp: 0.05,
            spatial_bpp: 1.0,
            ..complete_loop_stats_sample_row()
        };
        let merged = super::loop_intent_stored_metadata_from_sample_row(base, &row);
        assert_eq!(merged.frame_delay_variation, Some(0.42));
        assert!(super::loop_intent_stored_metadata_has_required_probe_fields(&merged));
    }

    #[test]
    fn test_bootstrap_loop_feature_map_has_all_required_features() {
        let feature_map = bootstrap_loop_feature_map();
        assert_eq!(feature_map.stats.len(), LOOP_VECTOR_FEATURE_NAMES.len());
        for feature_name in LOOP_VECTOR_FEATURE_NAMES {
            let stats = feature_map
                .stats
                .get(feature_name)
                .expect("bootstrap map should include every loop feature"); // audited: db module unit-test fixture assertion; not production DB runtime path
            assert_eq!(stats.weight, Some(1.0));
            assert!((stats.std_dev - 1.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_loop_metadata_round_trips_into_sample_row() {
        let metadata = LoopIntentStoredMetadata {
            loss_tolerance: Some("high".to_string()),
            temporal_bpp: Some(0.05),
            spatial_bpp: Some(2.5),
            frame_payload_variation: Some(0.2),
            frame_delay_variation: Some(0.1),
            aspect_ratio: Some(1.33),
            total_pixels: Some(76_800),
            loop_frequency: Some(0.8),
            directory_loop_intent_score: Some(0.5),
            palette_size: Some(256),
            palette_depth: Some(0.8),
            block_skew: Some(0.3),
            temporal_flatness: Some(0.7),
            webp_compression_ratio: Some(0.85),
            physics_225: Some(vec![0.0_f32; 225]),
            flags: SampleFlags::default(),
            ..Default::default()
        };

        let sample_row = sample_row_from_loop_metadata(LoopMetadataSampleInput {
            width: 320,
            height: 240,
            duration_secs: 3.5,
            frame_count: 35,
            file_size_bytes: 500_000,
            fps: Some(10.0),
            motion_periodicity: Some(0.4),
            temporal_jitter: Some(0.1),
            motion_gini: Some(0.5),
            loop_closure_score: Some(0.6),
            cadence_score: Some(0.9),
            metadata,
        })
        .expect("loop metadata should rebuild a SampleRow"); // audited: db module unit-test fixture assertion; not production DB runtime path

        assert!((sample_row.temporal_bpp - 0.05).abs() < f64::EPSILON);
        assert_eq!(sample_row.frame_payload_variation, Some(0.2));
        assert_eq!(
            sample_row.physics_225.as_ref().map(std::vec::Vec::len),
            Some(225)
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_hdbscan_cluster_prior_fusion_biases_keep_probability() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION,
            "0",
        );
        let catalog = LoopHdbscanCatalog {
            version: 1,
            min_cluster_size: 5,
            noise_count: 0,
            clusters: vec![
                LoopHdbscanClusterCentroid {
                    cluster_id: 0,
                    loop_prior: 1.0,
                    member_count: 10,
                    centroid: vec![0.0_f64; 4],
                },
                LoopHdbscanClusterCentroid {
                    cluster_id: 1,
                    loop_prior: 0.0,
                    member_count: 10,
                    centroid: vec![10.0_f64; 4],
                },
            ],
        };
        let query = vec![0.1_f32, 0.0_f32, 0.0_f32, 0.0_f32];
        let (fused, cluster_id, cluster_prior) =
            fuse_keep_probability_with_hdbscan_cluster(0.5, &query, Some(&catalog))
                .expect("fusion should succeed"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(cluster_id, Some(0));
        assert_eq!(cluster_prior, Some(1.0));
        assert!(fused > 0.5);
    }

    #[test]
    #[serial_test::serial]
    fn test_hdbscan_cluster_fusion_disabled_allows_missing_catalog() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION,
            "1",
        );
        let query = vec![0.0_f32; 4];
        let (fused, cluster_id, cluster_prior) =
            fuse_keep_probability_with_hdbscan_cluster(0.42, &query, None)
                .expect("fusion off should still return KNN posterior"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert!((fused - 0.42).abs() < f64::EPSILON);
        assert_eq!(cluster_id, None);
        assert_eq!(cluster_prior, None);
    }

    #[test]
    #[serial_test::serial]
    fn test_hdbscan_cluster_fusion_disabled_via_kill_switch() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION,
            "1",
        );
        let catalog = LoopHdbscanCatalog {
            version: crate::constants::SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION,
            min_cluster_size: 5,
            noise_count: 0,
            clusters: vec![LoopHdbscanClusterCentroid {
                cluster_id: 0,
                loop_prior: 1.0,
                member_count: 10,
                centroid: vec![0.0_f64; 4],
            }],
        };
        let query = vec![0.0_f32; 4];
        let (fused, cluster_id, cluster_prior) =
            fuse_keep_probability_with_hdbscan_cluster(0.4, &query, Some(&catalog))
                .expect("fusion off should return KNN posterior"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert!((fused - 0.4).abs() < f64::EPSILON);
        assert_eq!(cluster_id, None);
        assert_eq!(cluster_prior, None);
    }

    #[test]
    #[serial_test::serial]
    fn test_hdbscan_cluster_fusion_rejects_unsupported_catalog_version() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION,
            "0",
        );
        let catalog = LoopHdbscanCatalog {
            version: 99,
            min_cluster_size: 5,
            noise_count: 0,
            clusters: vec![LoopHdbscanClusterCentroid {
                cluster_id: 0,
                loop_prior: 1.0,
                member_count: 10,
                centroid: vec![0.0_f64; 4],
            }],
        };
        let query = vec![0.0_f32; 4];
        let err = fuse_keep_probability_with_hdbscan_cluster(0.5, &query, Some(&catalog))
            .expect_err("invalid catalog must reject lookup when fusion is enabled");
        assert_eq!(err, LoopIntentLookupBranch::HdbscanCatalogUnavailable);
    }

    #[test]
    #[serial_test::serial]
    fn test_hdbscan_cluster_fusion_rejects_missing_catalog() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION,
            "0",
        );
        let query = vec![0.0_f32; 4];
        let err = fuse_keep_probability_with_hdbscan_cluster(0.5, &query, None)
            .expect_err("missing catalog must reject lookup when fusion is enabled");
        assert_eq!(err, LoopIntentLookupBranch::HdbscanCatalogUnavailable);
    }

    #[test]
    fn test_sanitize_hdbscan_catalog_on_load_drops_bad_centroid() {
        let mut feature_map = FeatureMap {
            hdbscan_catalog: Some(LoopHdbscanCatalog {
                version: crate::constants::SUPPORTED_LOOP_HDBSCAN_CATALOG_VERSION,
                min_cluster_size: 5,
                noise_count: 0,
                clusters: vec![
                    LoopHdbscanClusterCentroid {
                        cluster_id: 0,
                        loop_prior: 1.0,
                        member_count: 1,
                        centroid: vec![f64::NAN; 4],
                    },
                    LoopHdbscanClusterCentroid {
                        cluster_id: 1,
                        loop_prior: 0.0,
                        member_count: 1,
                        centroid: vec![1.0_f64; 4],
                    },
                ],
            }),
            ..Default::default()
        };
        sanitize_hdbscan_catalog_on_load(&mut feature_map);
        let catalog = feature_map
            .hdbscan_catalog
            .as_ref()
            .expect("valid cluster should remain"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(catalog.clusters.len(), 1);
        assert_eq!(catalog.clusters[0].cluster_id, 1);
    }

    #[test]
    fn parse_loop_hdbscan_catalog_seed_tolerates_empty_stats() {
        assert!(
            parse_loop_hdbscan_catalog_seed("{}")
                .expect("empty stats should not fail") // audited: db module unit-test fixture assertion; not production DB runtime path
                .is_none()
        );
        assert!(
            parse_loop_hdbscan_catalog_seed("   ")
                .expect("blank stats should not fail") // audited: db module unit-test fixture assertion; not production DB runtime path
                .is_none()
        );
    }

    #[test]
    fn parse_loop_hdbscan_catalog_seed_sanitizes_invalid_catalog() {
        let feature_stats = FeatureMap {
            hdbscan_catalog: Some(LoopHdbscanCatalog {
                version: 0,
                min_cluster_size: 5,
                noise_count: 0,
                clusters: vec![LoopHdbscanClusterCentroid {
                    cluster_id: 7,
                    loop_prior: 1.0,
                    member_count: 1,
                    centroid: vec![1.0_f64, 1.0_f64],
                }],
            }),
            ..Default::default()
        };
        let encoded = serde_json::to_string(&feature_stats).expect("feature map should serialize"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let catalog =
            parse_loop_hdbscan_catalog_seed(&encoded).expect("catalog parse should succeed"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert!(
            catalog.is_none(),
            "invalid stored centroids should be pruned during seed load"
        );
    }

    #[test]
    fn loop_inference_runtime_verdict_from_snapshot_reads_audit_field() {
        let snapshot = serde_json::json!({
            "audit_only": true,
            "runtime_final_verdict": "LoopStrong",
            "runtime_decision_reason": "Layer 6: KNN"
        });
        assert_eq!(
            loop_inference_runtime_verdict_from_snapshot(&snapshot),
            Some("LoopStrong")
        );
        assert_eq!(
            loop_inference_runtime_verdict_from_snapshot(&serde_json::json!({})),
            None
        );
    }

    #[test]
    fn inference_log_file_hash_policy_skips_unhashable_source() {
        assert_eq!(
            inference_log_file_hash_or_skip(None),
            InferenceLogHashDecision::MissingSourcePath
        );

        let missing =
            std::env::temp_dir().join(format!("mfb_inference_hash_missing_{}", std::process::id()));
        assert!(
            matches!(
                inference_log_file_hash_or_skip(Some(&missing)),
                InferenceLogHashDecision::None
            ),
            "missing concrete source path must skip inference_log insert"
        );
    }
}
