//! PostgreSQL-backed KNN learning system for loop-intent classification.
//!
//! This module provides database schema management, sample ingestion,
//! feature vector computation (pgvector/HNSW), similarity search,
//! inference logging, and health diagnostics. It enables the system
//! to learn from labeled GIF/video samples and improve classification
//! accuracy over time.

use crate::Rational;
use crate::loop_intent::LoopMeta;
use crate::media_meta_utils::scan_gif_headers;
use crate::progress_mode::emit_stderr;
use anyhow::{Context, Result};
use blake3::Hasher;
use indicatif::{ProgressBar, ProgressStyle};
use postgres::Client;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use tracing::warn;
use walkdir::WalkDir;

const PG_DEFAULT_CONNSTR: &str = "host=localhost dbname=modern_format_boost";
const IMPORT_KEY: &str = "dataset_seeds_import_v4";
const STATS_KEY: &str = "feature_stats_v1";

static DB_WARN_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DB_SCHEMA_INIT_LOGGED_ONCE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct FeatureMap {
    pub(crate) stats: std::collections::HashMap<String, FeatureStats>,
    pub(crate) top_keywords: Vec<String>,
}

#[cfg(test)]
impl FeatureMap {
    pub(crate) fn mock() -> Self {
        let mut stats = std::collections::HashMap::new();
        let features = [
            "pixels",
            "duration",
            "frame_count",
            "file_size_bytes",
            "density",
            "gap",
            "temporal_bpp",
            "spatial_bpp",
            "webp_ratio",
            "loop_freq",
            "cadence",
            "payload_var",
            "delay_var",
            "aspect",
            "p_depth",
            "loop_affin",
            "m_gini",
            "b_skew",
            "t_flat",
            "l_close",
            "m_period",
            "t_jitter",
            "dir_meme",
        ];

        for f in features {
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

        Self {
            stats,
            top_keywords: Vec::new(),
        }
    }
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
}

/// A single inference result logged to the `inference_log` table for feedback analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopInferenceRecord {
    /// Probability output from the decision tree classifier.
    pub tree_probability: f64,
    /// KNN-derived keep probability (None if KNN was not available).
    pub knn_keep_probability: Option<f64>,
    /// KNN confidence score (None if KNN was not available).
    pub knn_confidence: Option<f64>,
    /// Number of KNN neighbors used (None if KNN was not available).
    pub knn_neighbor_count: Option<usize>,
    /// Final blended probability after combining tree and KNN signals.
    pub final_probability: f64,
    /// The final verdict string (e.g., "`LoopStrong`", "`LoopWeak`").
    pub final_verdict: String,
    /// Human-readable explanation of the decision.
    pub decision_reason: String,
    /// Which decision layer produced the exit (e.g., "Layer 1-A").
    pub layer_exit: String,
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

    /// Minimum file size in bytes.
    pub size_min: f64,
    /// Average file size in bytes.
    pub size_avg: f64,
    /// Maximum file size in bytes.
    pub size_max: f64,

    /// Minimum bitrate in bits/sec.
    pub bitrate_min: f64,
    /// Average bitrate in bits/sec.
    pub bitrate_avg: f64,
    /// Maximum bitrate in bits/sec.
    pub bitrate_max: f64,

    /// Minimum width in pixels.
    pub width_min: f64,
    /// Average width in pixels.
    pub width_avg: f64,
    /// Maximum width in pixels.
    pub width_max: f64,

    /// Minimum height in pixels.
    pub height_min: f64,
    /// Average height in pixels.
    pub height_avg: f64,
    /// Maximum height in pixels.
    pub height_max: f64,

    /// Minimum aspect ratio (width/height).
    pub aspect_min: f64,
    /// Average aspect ratio (width/height).
    pub aspect_avg: f64,
    /// Maximum aspect ratio (width/height).
    pub aspect_max: f64,
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
}

impl Default for GlobalCollectionStats {
    fn default() -> Self {
        use crate::constants::{
            DEFAULT_LOOP_BASELINE_DURATION_SECS, MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS,
        };
        Self {
            duration_min: Some(0.1),
            duration_avg: Some(DEFAULT_LOOP_BASELINE_DURATION_SECS),
            duration_max: Some(30.0),
            duration_p90: Some(MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS),

            size_min: 1000.0,
            size_avg: 1_000_000.0,
            size_max: 5_000_000.0,

            bitrate_min: 10_000.0,
            bitrate_avg: 500_000.0,
            bitrate_max: 2_000_000.0,

            width_min: 32.0,
            width_avg: 512.0,
            width_max: 1280.0,

            height_min: 32.0,
            height_avg: 512.0,
            height_max: 1280.0,

            aspect_min: 0.5,
            aspect_avg: 1.0,
            aspect_max: 2.0,
            top_keywords: Vec::new(),
        }
    }
}

impl Default for LoopReferenceProfile {
    // Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
    #[allow(
        clippy::too_many_lines,
        reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
    )]
    fn default() -> Self {
        let collection = GlobalCollectionStats::default();
        let pixels_min = collection.width_min * collection.height_min;
        let pixels_avg = collection.width_avg * collection.height_avg;
        let pixels_max = collection.width_max * collection.height_max;

        Self {
            duration: DistributionStats {
                mean: collection
                    .duration_avg
                    .unwrap_or(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS),
                std_dev: ((collection.duration_max.unwrap_or(30.0)
                    - collection.duration_min.unwrap_or(0.1))
                    / 4.0)
                    .max(0.5),
                p10: collection.duration_min,
                p25: Some(f64::midpoint(
                    collection.duration_min.unwrap_or(0.1),
                    collection
                        .duration_avg
                        .unwrap_or(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS),
                )),
                p50: collection.duration_avg,
                p75: Some(f64::midpoint(
                    collection
                        .duration_avg
                        .unwrap_or(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS),
                    collection
                        .duration_p90
                        .unwrap_or(crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS),
                )),
                p90: collection.duration_p90,
                weight: None,
            },
            fps: DistributionStats {
                mean: 12.0,
                std_dev: 8.0,
                p10: Some(4.0),
                p25: Some(8.0),
                p50: Some(12.0),
                p75: Some(18.0),
                p90: Some(24.0),
                weight: None,
            },
            frame_density: DistributionStats {
                mean: 12.0,
                std_dev: 8.0,
                p10: Some(4.0),
                p25: Some(8.0),
                p50: Some(12.0),
                p75: Some(18.0),
                p90: Some(24.0),
                weight: None,
            },
            file_size_bytes: DistributionStats {
                mean: collection.size_avg,
                std_dev: ((collection.size_max - collection.size_min) / 4.0).max(64_000.0),
                p10: Some(collection.size_min),
                p25: Some(f64::midpoint(collection.size_min, collection.size_avg)),
                p50: Some(collection.size_avg),
                p75: Some(f64::midpoint(collection.size_avg, collection.size_max)),
                p90: Some(collection.size_max),
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
                mean: 0.05,
                std_dev: 0.05,
                p10: Some(0.01),
                p25: Some(0.02),
                p50: Some(0.05),
                p75: Some(0.08),
                p90: Some(0.12),
                weight: None,
            },
            spatial_bpp: DistributionStats {
                mean: 4.0,
                std_dev: 3.0,
                p10: Some(1.0),
                p25: Some(2.0),
                p50: Some(4.0),
                p75: Some(6.0),
                p90: Some(10.0),
                weight: None,
            },
            payload_variation: DistributionStats {
                mean: 0.5,
                std_dev: 0.2,
                p10: Some(0.2),
                p25: Some(0.35),
                p50: Some(0.5),
                p75: Some(0.65),
                p90: Some(0.8),
                weight: None,
            },
            delay_variation: DistributionStats {
                mean: 0.25,
                std_dev: 0.15,
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
                mean: 0.55,
                std_dev: 0.18,
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
                mean: 10.0,
                std_dev: 4.0,
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
            collection,
        }
    }
}

/// Row shape for GIF/video KNN features; some fields are stored for DB round-trip / future use.
#[derive(Debug, Clone)]
// Rationale: This struct serves as a comprehensive configuration or state container where individual boolean flags are the most idiomatic and explicit way to represent discrete options.
#[allow(
    clippy::struct_excessive_bools,
    reason = "Data models naturally require multiple boolean flags to map independent configuration features. Grouping them into bitflags would break explicit serde mapping."
)]
pub(crate) struct SampleRow {
    pub(crate) _loss_tolerance: Option<String>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) duration_secs: Option<f64>,
    pub(crate) frame_count: Option<u64>,
    pub(crate) file_size_bytes: u64,
    pub(crate) fps: Option<f64>,
    pub(crate) temporal_bpp: f64,
    pub(crate) spatial_bpp: f64,
    pub(crate) has_transparency: bool,
    pub(crate) has_embedded_icc: bool,
    pub(crate) has_complex_color_profile: bool,
    pub(crate) palette_size: Option<u32>,
    pub(crate) frame_payload_variation: Option<f64>,
    pub(crate) frame_delay_variation: Option<f64>,
    pub(crate) aspect_ratio: Option<f64>,
    pub(crate) _total_pixels: Option<u64>,
    pub(crate) loop_frequency: Option<f64>,
    pub(crate) is_meme_platform: bool,
    pub(crate) is_human_semantic_name: bool,
    pub(crate) cadence_score: Option<f64>,
    pub(crate) directory_loop_intent_score: Option<f64>,
    pub(crate) is_high_value_source: bool,
    pub(crate) is_native_gif: bool,
    pub(crate) palette_depth: Option<f64>,
    pub(crate) motion_gini: Option<f64>,
    pub(crate) block_skew: Option<f64>,
    pub(crate) temporal_flatness: Option<f64>,
    pub(crate) loop_closure_score: Option<f64>,
    pub(crate) motion_periodicity: Option<f64>,
    pub(crate) temporal_jitter: Option<f64>,
    pub(crate) webp_compression_ratio: Option<f64>,
    pub(crate) _labeled_by: Option<String>,
}

impl From<SampleInsert> for SampleRow {
    fn from(s: SampleInsert) -> Self {
        Self {
            _loss_tolerance: Some(s.loss_tolerance),
            width: s.width,
            height: s.height,
            duration_secs: s.duration_secs,
            frame_count: s.frame_count,
            file_size_bytes: s.file_size_bytes,
            fps: s.fps,
            temporal_bpp: s.temporal_bpp,
            spatial_bpp: s.spatial_bpp,
            has_transparency: s.has_transparency,
            has_embedded_icc: s.has_embedded_icc,
            has_complex_color_profile: s.has_complex_color_profile,
            palette_size: s.palette_size,
            frame_payload_variation: s.frame_payload_variation,
            frame_delay_variation: s.frame_delay_variation,
            aspect_ratio: s.aspect_ratio,
            _total_pixels: Some(s.total_pixels),
            loop_frequency: Some(s.loop_frequency),
            is_meme_platform: s.is_meme_platform,
            is_human_semantic_name: s.is_human_semantic_name,
            cadence_score: Some(s.cadence_score),
            directory_loop_intent_score: Some(s.directory_loop_intent_score),
            is_high_value_source: s.is_high_value_source,
            is_native_gif: s.is_native_gif,
            palette_depth: s.palette_depth,
            motion_gini: s.motion_gini,
            block_skew: s.block_skew,
            temporal_flatness: s.temporal_flatness,
            loop_closure_score: None,
            motion_periodicity: None,
            temporal_jitter: None,
            webp_compression_ratio: s.webp_compression_ratio,
            _labeled_by: Some(s.labeled_by),
        }
    }
}

fn pg_connstr() -> String {
    std::env::var("MFB_PG_CONNSTR").unwrap_or_else(|_| PG_DEFAULT_CONNSTR.to_string())
}

/// Reads the connection string from `MFB_PG_CONNSTR` env var or falls back
/// to the default localhost connection. Emits a one-time warning to stderr
/// if the connection fails.
///
/// # Errors
/// Returns an error if the database connection fails.
pub fn open_pg_client() -> Result<Client> {
    let connstr = pg_connstr();
    match Client::connect(&connstr, postgres::NoTls) {
        Ok(client) => Ok(client),
        Err(e) => {
            if !DB_WARN_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let msg = format!("⚠️  Database Unavailable: {e}");
                crate::progress_mode::emit_stderr(&msg);
                crate::progress_mode::emit_stderr(
                    "💡 System running in [LEGACY LIMITED MODE] (Heuristic Tree only, no KNN/Learning).",
                );
                crate::progress_mode::emit_stderr(
                    "💡 To enable full intelligence, run: 'python3 crates/dev/scripts/database_manager.py' (option 1: Database Setup)",
                );
            }
            Err(e).with_context(|| format!("Failed to connect to PostgreSQL: {connstr}"))
        }
    }
}

/// Prints a one-line status message indicating whether the database is reachable.
pub fn report_db_status() {
    if let Ok(_conn) = open_pg_client() {
        crate::progress_mode::emit_stderr(
            "🐘 Database [PostgreSQL]: CONNECTED (Full Learning Mode Active)",
        );
    }
}

/// Look up similar samples in the database using HNSW vector search.
///
/// Returns a `SampleMatch` if enough labeled training data exists and
/// similar neighbors are found. Returns `None` on DB error or if the
/// database is too immature for reliable KNN.
#[must_use]
pub fn lookup_similar_samples(meta: &LoopMeta, path: Option<&Path>) -> Option<SampleMatch> {
    match lookup_similar_samples_inner(meta, path) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.map(|p| p.display().to_string()).unwrap_or_default(),
                "similar sample lookup failed; falling back to heuristic-only decision"
            );
            None
        }
    }
}

/// Retrieve aggregate collection statistics from the metadata table.
///
/// Returns default stats if no stored stats are found.
///
/// # Errors
/// Returns an error if the database query fails.
pub fn fetch_global_collection_stats(conn: &mut Client) -> Result<GlobalCollectionStats> {
    let row = conn.query_opt(
        "SELECT value FROM sample_metadata WHERE key = 'collection_stats_v1'",
        &[],
    )?;

    row.map_or_else(
        || Ok(GlobalCollectionStats::default()),
        |row| {
            let json: String = row.get(0);
            Ok(serde_json::from_str(&json)?)
        },
    )
}

/// Fetch the full loop reference profile, combining collection stats
/// with per-feature distribution statistics.
///
/// # Errors
/// Returns an error if the underlying database fetches fail.
pub fn fetch_loop_reference_profile(conn: &mut Client) -> Result<LoopReferenceProfile> {
    let collection = fetch_global_collection_stats(conn)?;
    let feature_map = fetch_feature_map(conn)?;
    Ok(build_loop_reference_profile(collection, &feature_map))
}

fn fetch_feature_map(conn: &mut Client) -> Result<FeatureMap> {
    let row_opt = conn.query_opt(
        "SELECT value FROM sample_metadata WHERE key = $1",
        &[&STATS_KEY],
    )?;

    match row_opt {
        Some(row) => {
            let value: String = row.get(0);
            Ok(serde_json::from_str(&value)?)
        }
        None => Ok(FeatureMap::default()),
    }
}

fn build_loop_reference_profile(
    collection: GlobalCollectionStats,
    feature_map: &FeatureMap,
) -> LoopReferenceProfile {
    let mut profile = LoopReferenceProfile {
        collection,
        top_keywords: feature_map.top_keywords.clone(),
        ..LoopReferenceProfile::default()
    };
    profile.duration = distribution_from_feature(feature_map, "duration", profile.duration.clone());
    profile.fps = distribution_from_feature(feature_map, "fps", profile.fps.clone());
    profile.frame_density =
        distribution_from_feature(feature_map, "density", profile.frame_density.clone());
    profile.file_size_bytes = distribution_from_feature(
        feature_map,
        "file_size_bytes",
        profile.file_size_bytes.clone(),
    );
    profile.pixels = distribution_from_feature(feature_map, "pixels", profile.pixels.clone());
    profile.temporal_bpp =
        distribution_from_feature(feature_map, "temporal_bpp", profile.temporal_bpp.clone());
    profile.spatial_bpp =
        distribution_from_feature(feature_map, "spatial_bpp", profile.spatial_bpp.clone());
    profile.payload_variation = distribution_from_feature(
        feature_map,
        "payload_var",
        profile.payload_variation.clone(),
    );
    profile.delay_variation =
        distribution_from_feature(feature_map, "delay_var", profile.delay_variation.clone());
    profile.palette_depth =
        distribution_from_feature(feature_map, "p_depth", profile.palette_depth.clone());
    profile.motion_gini =
        distribution_from_feature(feature_map, "m_gini", profile.motion_gini.clone());
    profile.temporal_flatness =
        distribution_from_feature(feature_map, "t_flat", profile.temporal_flatness.clone());
    profile.webp_ratio =
        distribution_from_feature(feature_map, "webp_ratio", profile.webp_ratio.clone());
    profile.cadence = distribution_from_feature(feature_map, "cadence", profile.cadence.clone());
    profile
}

fn distribution_from_feature(
    feature_map: &FeatureMap,
    key: &str,
    fallback: DistributionStats,
) -> DistributionStats {
    feature_map
        .stats
        .get(key)
        .map_or(fallback, DistributionStats::from)
}

/// Retrieves the count of samples in 'low', 'high', and 'video' classes.
fn get_class_counts(conn: &mut Client) -> (i64, i64, i64) {
    let Ok(rows) = conn.query(
        "SELECT 
            CASE WHEN loss_tolerance = 'low' THEN 'low'
                 WHEN loss_tolerance = 'high' THEN 'high' 
                 WHEN loss_tolerance = 'video' THEN 'video' 
                 ELSE 'other' 
            END as class, 
            count(*) 
         FROM samples 
         WHERE loss_tolerance IN ('low', 'high', 'video')
         GROUP BY 1",
        &[],
    ) else {
        return (0, 0, 0);
    };

    let mut low_count: i64 = 0;
    let mut high_count: i64 = 0;
    let mut video_count: i64 = 0;
    for row in rows {
        let class: String = row.get(0);
        let count: i64 = row.get(1);
        if class == "low" {
            low_count = count;
        } else if class == "high" {
            high_count = count;
        } else if class == "video" {
            video_count = count;
        }
    }
    (low_count, high_count, video_count)
}

/// Validates whether the GIF database has enough diverse samples to merit KNN lookup.
fn check_gif_db_maturity(conn: &mut Client) -> bool {
    let (low_count, high_count, video_count) = get_class_counts(conn);
    let total = low_count + high_count + video_count;

    // For maturity, we consider 'low' as our high-quality baseline (LoopStrong)
    // and 'high' + 'video' as our conversion baseline (LoopWeak).
    let quality_class = low_count;
    let video_equivalent_class = high_count + video_count;

    log::info!(
        "📊 GIF DB Check: low(quality)={}, high={}, video={}, total={} (Needed per class: {})",
        low_count,
        high_count,
        video_count,
        total,
        crate::constants::MIN_GIF_SAMPLES_PER_CLASS
    );

    total >= crate::constants::MIN_GIF_SAMPLES_TOTAL
        && quality_class >= crate::constants::MIN_GIF_SAMPLES_PER_CLASS
        && video_equivalent_class >= crate::constants::MIN_GIF_SAMPLES_PER_CLASS
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn lookup_similar_samples_inner(
    meta: &LoopMeta,
    _path: Option<&Path>,
) -> Result<Option<SampleMatch>> {
    let mut conn = match open_pg_client() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("⚠️ PostgreSQL connection failed (graceful fallback): {e}");
            log::warn!(
                "💡 Suggestion: Run 'python3 crates/dev/scripts/database_manager.py' (option 1: Database Setup) to initialize and start the local database service."
            );
            return Ok(None);
        }
    };

    init_schema(&mut conn)?;
    seed_positive_dataset_if_needed(&mut conn)?;

    // ── Automated Vector Hydration Check ──
    // If we have samples but no feature vectors, trigger a one-time backfill
    let missing_vec_count: i64 = conn
        .query_one("SELECT COUNT(*) FROM samples WHERE features IS NULL", &[])?
        .get(0);
    if missing_vec_count > 0 {
        let total_count: i64 = conn.query_one("SELECT COUNT(*) FROM samples", &[])?.get(0);
        if total_count > 0 {
            log::info!(
                "🧩 Detected {missing_vec_count} samples with missing feature vectors. Triggering automated recompute..."
            );
            recompute_all_features(&mut conn)?;
        }
    }

    if !check_gif_db_maturity(&mut conn) {
        log::info!(
            "🔬 GIF Database is immature (needs >={} total, >={} per class). Bypassing KNN.",
            crate::constants::MIN_GIF_SAMPLES_TOTAL,
            crate::constants::MIN_GIF_SAMPLES_PER_CLASS
        );
        return Ok(None);
    }

    // Map the incoming LoopMeta into a SampleRow to compute its HNSW search vector
    let (target_temporal_bpp, target_spatial_bpp) = bpp_from_meta(meta);

    let target_sample = sample_row_from_meta(meta, target_temporal_bpp, target_spatial_bpp);

    let feature_stats = fetch_feature_map(&mut conn)?;
    let Some(target_vector) =
        crate::database_vector::compute_sample_vector(&target_sample, &feature_stats)
    else {
        tracing::debug!(
            "KNN: Target sample lacks required features; falling back to non-KNN scoring."
        );
        return Ok(None);
    };
    let target_pg_vector = pgvector::Vector::from(target_vector);

    // Deep pgvector Integration: We let PostgreSQL use the HNSW index to rapidly return the closest labels
    let rows = conn.query(
        "SELECT
            loss_tolerance, duration_secs, 
            features <-> $1::vector AS dist
         FROM samples
         WHERE loss_tolerance IS NOT NULL AND features IS NOT NULL AND frame_count > 1
         ORDER BY features <-> $1::vector
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
        return Ok(None);
    }

    let candidates: Vec<(LabelStatus, Option<f64>, f64)> = rows
        .iter()
        .map(|row| {
            let label_str: Option<String> = row.get(0);
            let label = match label_str.as_deref() {
                Some("low") => LabelStatus::LoopStrong,
                Some("high" | "video") => LabelStatus::LoopWeak,
                Some("uncertain") => LabelStatus::Uncertain,
                _ => LabelStatus::NotLabeled,
            };
            return (
                label,                        // label status
                row.get::<_, Option<f64>>(1), // duration_secs
                row.get::<_, f64>(2),         // dist
            );
        })
        .collect();

    let neighbor_count = match adaptive_neighbor_count(candidates.len()) {
        Ok(count) => count,
        Err(err) => {
            tracing::error!(
                "☢️ [CRITICAL ANOMALY] {}; skipping KNN classification to avoid forgery.",
                err
            );
            return Ok(None);
        }
    };
    let neighbors = &candidates[..neighbor_count.min(candidates.len())];

    let min_distance = neighbors.first().map_or(0.0_f64, |(_, _, d)| *d);
    let radius = dynamic_neighbor_radius(neighbors);

    let (low_count, high_count, video_count) = get_class_counts(&mut conn);
    let total_samples = low_count + high_count + video_count;
    let quality_count = low_count;
    let video_equivalent_count = high_count + video_count;

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

        let relative_distance = (*distance - min_distance).max(0.0);
        let Some(rel_dist_r) = rug::Rational::from_f64(
            1.0_f64 / (relative_distance * relative_distance).mul_add(3.0, 1.0),
        ) else {
            tracing::warn!(
                distance,
                "☢️ [ANOMALY] NaN/Inf distance in KNN neighbor — skipping corrupt neighbor"
            );
            continue;
        };
        let distance_weight = rel_dist_r;

        let class_weight = match label {
            LabelStatus::LoopStrong => {
                rug::Rational::from_f64(w_quality).expect("w_quality is a finite constant")
            }
            LabelStatus::LoopWeak => {
                rug::Rational::from_f64(w_video).expect("w_video is a finite constant")
            }
            _ => Rational::from(1),
        };

        let final_weight = distance_weight * class_weight;

        let prob = match label {
            LabelStatus::LoopStrong => Rational::from(1), // Loop intent (Meme/Sticker/Video sticker)
            LabelStatus::LoopWeak => Rational::from(0), // Non-loop intent (Clip/Record/Long Video)
            _ => Rational::from((1, 2)),                // Uncertain/Fallback
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
        return Ok(None);
    }

    let min_weight = rug::Rational::from_f64(1e-6).expect("1e-6 is strictly finite");
    let divisor = if total_weight > min_weight {
        total_weight.clone()
    } else {
        min_weight
    };
    let local_keep_probability = (weighted_keep / divisor).to_f64();
    let eff_n = effective_sample_size(weight_squares_sum.to_f64(), total_weight.to_f64());
    // With higher imbalance, require stronger local evidence before moving away from global prior.
    let prior_strength = 2.0f64.mul_add(global_imbalance_ratio.ln_1p(), 3.0);
    let shrink = (eff_n / (eff_n + prior_strength)).clamp(0.0, 1.0);
    let keep_probability = global_keep_prior
        .mul_add(1.0 - shrink, local_keep_probability * shrink)
        .clamp(0.0, 1.0);
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
    let balance_penalty = (1.0 / global_imbalance_ratio.sqrt()).clamp(0.45, 1.0);
    confidence *= balance_penalty;
    confidence *= (eff_n / (eff_n + 2.0)).clamp(0.25, 1.0);
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
    distances.sort_by(|a: &f64, b: &f64| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = distances.len();
    let min_distance = distances.first().copied();
    let p25_distance = distances.get(n / 4).copied();
    let p75_distance = distances.get(3 * n / 4).copied();

    let p90_duration = if loop_durations.is_empty() {
        None
    } else {
        loop_durations
            .sort_by(|a: &f64, b: &f64| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = crate::numeric_cast::f64_to_usize_strict(
            (crate::numeric_cast::usize_to_f64(loop_durations.len()) * 0.90).floor(),
            "p90_idx",
        )
        .ok_or_else(|| {
            crate::progress_mode::emit_stderr("☢️ [ANOMALY] p90_idx overflow! Refusing to forge percentile. Information invalidated.");
            anyhow::anyhow!("p90_idx overflow")
        })?;
        Some(
            *loop_durations
                .get(idx.min(loop_durations.len().saturating_sub(1)))
                .ok_or_else(|| {
                    crate::progress_mode::emit_stderr(
                        "☢️ [ANOMALY] Percentile index out of bounds! Refusing to forge data.",
                    );
                    anyhow::anyhow!("Percentile index out of bounds")
                })?,
        )
    };

    Ok(Some(SampleMatch {
        exact_label: LabelStatus::NotLabeled,
        keep_probability: Some(keep_probability),
        confidence,
        neighbor_count: distances.len(),
        mean_distance: Some(mean_distance),
        std_dev_distance: Some(std_dev_distance),
        min_distance,
        p25_distance,
        p75_distance,
        p90_duration,
    }))
}

/// Dynamic safety-guard for CRF 0.00 exploration.
///
/// Uses the SQL KNN dataset to partition media into "Meme" vs "High Value".
/// High-value art is strictly limited to 30s of lossless-first probing to avoid bloat.
/// Low-value memes (low entropy) are permitted up to 120s as CRF 0.00 is efficient on them.
#[must_use]
fn lossless_duration_limit_for_keep_prob(keep_prob: f64) -> f32 {
    use crate::constants::{HIGH_VALUE_LOSSLESS_DURATION_LIMIT, MEME_LOSSLESS_DURATION_LIMIT};

    if keep_prob <= 0.3 {
        HIGH_VALUE_LOSSLESS_DURATION_LIMIT
    } else if keep_prob >= 0.7 {
        MEME_LOSSLESS_DURATION_LIMIT
    } else {
        let t = (keep_prob - 0.3) / 0.4;
        let limit_meme = f64::from(MEME_LOSSLESS_DURATION_LIMIT);
        let limit_high = f64::from(HIGH_VALUE_LOSSLESS_DURATION_LIMIT);
        crate::numeric_cast::f64_to_f32_lossy(limit_high + (t * (limit_meme - limit_high)))
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
                warn!(
                    "☢️ [ANOMALY] Frame count present but duration/FPS missing for resolved_duration! Refusing to forge data."
                );
                None
            }
            None => {
                // Total metadata vacuum: neither duration nor frame count.
                warn!(
                    "☢️ [ANOMALY] Duration and frame_count missing for resolved_duration! Refusing to forge data."
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
    use crate::constants::HIGH_VALUE_LOSSLESS_DURATION_LIMIT;

    let mut current_meta = meta.clone();
    if let Some(p) = path
        && let Err(e) = crate::loop_intent::deep_refine_meta(&mut current_meta, p)
    {
        tracing::warn!(
            error = %e,
            path = %p.display(),
            "lossless exploration metadata refinement failed; using existing metadata"
        );
    }
    if let Some(duration) = resolved_duration_secs(&current_meta) {
        current_meta.duration_secs = Some(duration);
    } else {
        crate::log_eprintln!(
            "   ☢️  Lossless-first (CRF 0.00) skip: metadata invalidated (duration unknown)."
        );
        return false;
    }

    let sample_match = lookup_similar_samples(&current_meta, path);
    let keep_prob = sample_match.as_ref().and_then(|m| m.keep_probability);

    // Dynamic threshold:
    // keep_prob close to 1.0 (Meme / High Tolerance) -> 120s limit
    // keep_prob close to 0.0 (Art / High Value)  -> 30s limit
    let threshold = keep_prob.map_or_else(
        || {
            crate::log_eprintln!(
                "   ⚠️  Lossless-first safety KNN unavailable or unknown — using conservative limit {:.1}s",
                HIGH_VALUE_LOSSLESS_DURATION_LIMIT
            );
            HIGH_VALUE_LOSSLESS_DURATION_LIMIT
        },
        lossless_duration_limit_for_keep_prob,
    );

    let is_safe = current_meta
        .duration_secs
        .is_some_and(|d| d < f64::from(threshold));

    if !is_safe {
        if let Some(keep_prob) = keep_prob {
            crate::log_eprintln!(
                "   ⚠️  Lossless-first (CRF 0.00) skip: duration {:?}s exceeds dynamic limit {:.1}s (Value Prob: {:.2})",
                current_meta.duration_secs,
                threshold,
                keep_prob
            );
        } else {
            crate::log_eprintln!(
                "   ⚠️  Lossless-first (CRF 0.00) skip: duration {:?}s exceeds conservative unknown-evidence limit {:.1}s",
                current_meta.duration_secs,
                threshold
            );
        }
    }

    is_safe
}

/// Creates the `samples`, `sample_metadata`, and `feature_stats` tables.
///
/// If they do not exist, enables the `pgvector` extension, creates necessary
/// indexes, and performs any necessary column additions or migrations.
///
/// # Errors
/// Returns an error if the database schema cannot be initialized or migrated.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn init_schema(conn: &mut Client) -> Result<()> {
    if !DB_SCHEMA_INIT_LOGGED_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::debug!("Initializing Database Schema (PostgreSQL + pgvector)");
    }
    // Enable pgvector extension
    if let Err(e) = conn.execute("CREATE EXTENSION IF NOT EXISTS vector", &[]) {
        emit_stderr(&format!("⚠️  pgvector extension failed to load: {e}"));
    }

    conn.execute(
        "CREATE TABLE IF NOT EXISTS samples (
            file_hash TEXT PRIMARY KEY,
            source_path TEXT,
            file_name TEXT,
            source_ext TEXT,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            duration_secs DOUBLE PRECISION,
            frame_count BIGINT NOT NULL,
            file_size_bytes BIGINT NOT NULL,
            fps DOUBLE PRECISION,
            has_embedded_icc BOOLEAN NOT NULL DEFAULT FALSE,
            has_complex_color_profile BOOLEAN NOT NULL DEFAULT FALSE,
            has_transparency BOOLEAN NOT NULL DEFAULT FALSE,
            palette_size INTEGER,
            frame_payload_variation DOUBLE PRECISION,
            frame_delay_variation DOUBLE PRECISION,
            temporal_bpp DOUBLE PRECISION NOT NULL,
            spatial_bpp DOUBLE PRECISION NOT NULL,
            aspect_ratio DOUBLE PRECISION,
            total_pixels BIGINT,
            loop_frequency DOUBLE PRECISION,
            is_meme_platform BOOLEAN DEFAULT FALSE,
            is_human_semantic_name BOOLEAN DEFAULT FALSE,
            cadence_score DOUBLE PRECISION,
            directory_meme_hint BOOLEAN DEFAULT FALSE,
            directory_loop_intent_score DOUBLE PRECISION DEFAULT 0.5,
            is_high_value_source BOOLEAN DEFAULT FALSE,
            is_native_gif BOOLEAN DEFAULT FALSE,
            palette_depth DOUBLE PRECISION,
            motion_gini DOUBLE PRECISION,
            block_skew DOUBLE PRECISION,
            temporal_flatness DOUBLE PRECISION,
            loop_closure_score DOUBLE PRECISION,
            motion_periodicity DOUBLE PRECISION,
            temporal_jitter DOUBLE PRECISION,
            loss_tolerance TEXT,
            loop_verdict TEXT,
            labeled_by TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            features vector(31)
        )",
        &[],
    )?;

    // Ensure older databases upgrade their schema
    conn.execute(
        "ALTER TABLE samples DROP COLUMN IF EXISTS features CASCADE",
        &[],
    )?;
    conn.execute("ALTER TABLE samples ADD COLUMN features vector(31)", &[])?;

    conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS loop_closure_score DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS motion_periodicity DOUBLE PRECISION",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS temporal_jitter DOUBLE PRECISION",
        &[],
    )?;

    // The HNSW index for high-performance vector retrieval
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_samples_features_hnsw 
         ON samples USING hnsw (features vector_l2_ops)",
        &[],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS sample_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        &[],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS inference_log (
            id BIGSERIAL PRIMARY KEY,
            file_hash TEXT,
            source_path TEXT,
            duration_secs DOUBLE PRECISION,
            webp_compression_ratio DOUBLE PRECISION,
            tree_probability DOUBLE PRECISION NOT NULL,
            knn_keep_probability DOUBLE PRECISION,
            knn_confidence DOUBLE PRECISION,
            knn_neighbor_count INTEGER,
            final_probability DOUBLE PRECISION NOT NULL,
            final_verdict TEXT NOT NULL,
            decision_reason TEXT NOT NULL,
            layer_exit TEXT NOT NULL,
            signal_snapshot JSONB NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        &[],
    )?;

    // Migration for existing tables
    conn.execute(
        "ALTER TABLE inference_log ADD COLUMN IF NOT EXISTS layer_exit TEXT DEFAULT 'Unknown'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE inference_log ADD COLUMN IF NOT EXISTS signal_snapshot JSONB DEFAULT '{}'",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE inference_log ALTER COLUMN layer_exit SET NOT NULL",
        &[],
    )?;
    conn.execute(
        "ALTER TABLE inference_log ALTER COLUMN signal_snapshot SET NOT NULL",
        &[],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_samples_lookup
         ON samples(loss_tolerance, width, height, duration_secs, has_transparency)",
        &[],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_inference_log_blindspots
         ON inference_log(knn_confidence, duration_secs, webp_compression_ratio)",
        &[],
    )?;

    conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS directory_loop_intent_score DOUBLE PRECISION DEFAULT 0.5",
        &[],
    )?;

    conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS webp_compression_ratio DOUBLE PRECISION",
        &[],
    )?;

    conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS loop_verdict TEXT",
        &[],
    )?;

    emit_stderr("✅ Database Schema Ready.");
    Ok(())
}

fn seed_positive_dataset_if_needed(conn: &mut Client) -> Result<()> {
    if cfg!(test) || std::env::var("MODERN_FORMAT_BOOST_DISABLE_SAMPLE_DB").is_ok() {
        return Ok(());
    }

    let imported = conn
        .query_opt(
            "SELECT value FROM sample_metadata WHERE key = $1",
            &[&IMPORT_KEY],
        )?
        .map(|row| row.get::<_, String>(0))
        .is_some_and(|v| v == "done");

    if imported {
        return Ok(());
    }

    emit_stderr("📥 Importing Default High-Value GIF Training Dataset...");
    let mut tx = conn.transaction()?;

    // Seed default dataset shipped with the binary (PostgreSQL-native SQL)
    let default_sql = include_str!("./sql/default_samples.sql");
    tx.batch_execute(default_sql)?;

    tx.execute(
        "INSERT INTO sample_metadata (key, value) VALUES ($1, 'done')
         ON CONFLICT (key) DO UPDATE SET value = 'done'",
        &[&IMPORT_KEY],
    )?;
    tx.commit()?;

    emit_stderr("✅ Training Dataset successfully imported.");

    // Recalculate stats based on the newly seeded data
    refresh_feature_stats(conn)
}

/// Intermediate representation of a sample's metadata ready for database
/// insertion. Contains all extracted features and classification labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
// Rationale: This struct serves as a comprehensive configuration or state container where individual boolean flags are the most idiomatic and explicit way to represent discrete options.
#[allow(
    clippy::struct_excessive_bools,
    reason = "Data models naturally require multiple boolean flags to map independent configuration features. Grouping them into bitflags would break explicit serde mapping."
)]
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
    /// Whether the file has an embedded ICC color profile.
    has_embedded_icc: bool,
    /// Whether the file uses a complex color profile.
    has_complex_color_profile: bool,
    /// Whether the file has transparency.
    has_transparency: bool,
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
    /// Whether the asset originates from a meme/sticker platform.
    is_meme_platform: bool,
    /// Whether the filename uses human-readable semantic naming.
    is_human_semantic_name: bool,
    /// Score measuring the regularity of frame timing patterns.
    cadence_score: f64,
    /// Score indicating how meme-like the source directory appears.
    directory_loop_intent_score: f64,
    /// Whether the source is considered high-value art.
    is_high_value_source: bool,
    /// Whether the source format is natively GIF.
    is_native_gif: bool,
    /// Depth of the color palette as a normalized score (0-1).
    palette_depth: Option<f64>,
    /// Motion Gini coefficient -- measures how concentrated motion is across frames.
    motion_gini: Option<f64>,
    /// Block skew measurement -- detects geometric distortion.
    block_skew: Option<f64>,
    /// Temporal flatness -- how uniform the temporal features are.
    temporal_flatness: Option<f64>,
    /// WebP compression ratio relative to the original.
    webp_compression_ratio: Option<f64>,
    /// Loop intent classification ("`LoopStrong`", "`LoopWeak`", or "`Uncertain`").
    loop_verdict: String,
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
        let stem = name
            .rsplit_once('.')
            .map_or(name, |(s, _)| s)
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
#[must_use]
fn gather_sample_metadata(path: &Path) -> Option<LoopMeta> {
    let probe = match crate::probe_video(path) {
        Ok(probe) => probe,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "sample probe failed; skipping training sample"
            );
            return None;
        }
    };
    let mut meta = LoopMeta::from_ffprobe_result(&probe, path);
    if let Ok(scan) = scan_gif_headers(path) {
        meta.palette_size = scan.palette_size;
        meta.app_extensions = scan.app_extensions;
        meta.has_transparency = scan.has_transparency;
        meta.frame_payload_variation = scan.frame_payload_variation;
        meta.frame_delay_variation = scan.frame_delay_variation;
        meta.loop_count = scan.loop_count;
        if let Some(duration_secs) = scan.duration_secs {
            meta.duration_secs = Some(duration_secs);
        }
    }

    // Call deep refinement to populate palette_depth, temporal_flatness, etc.
    if let Err(e) = crate::loop_intent::deep_refine_meta(&mut meta, path) {
        tracing::warn!(
            error = %e,
            path = %path.display(),
            "sample metadata refinement failed; storing probe-level features"
        );
    }
    Some(meta)
}

pub fn sample_from_path(
    path: &Path,
    labeled_by: &str,
    label_override: Option<&str>,
) -> Option<SampleInsert> {
    let mut meta = gather_sample_metadata(path)?;

    let (temporal_bpp, spatial_bpp) = bpp_from_meta(&meta);

    let loss_tolerance = if let Some(label) = label_override {
        label.to_string()
    } else {
        determine_loss_tolerance(
            temporal_bpp,
            meta.has_embedded_icc,
            meta.has_complex_color_profile,
            meta.app_extensions.as_deref(),
            path,
            meta.file_name.as_deref(),
        )
    };

    // If manual label is "video", ensure we treat it as a video-like source
    if loss_tolerance == "video" {
        meta.is_native_gif = false;
    }

    let aspect_ratio = if meta.height > 0 {
        Some(f64::from(meta.width) / f64::from(meta.height))
    } else {
        None
    };

    let total_pixels = u64::from(meta.width) * u64::from(meta.height);
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
    let is_meme_platform = meta.is_meme_platform;
    let is_native_gif = meta.is_native_gif;
    let is_high_value_source = loss_tolerance == "low";

    let file_hash = match calculate_blake3_hex(path) {
        Ok(hash) => hash,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "sample hashing failed; skipping training sample"
            );
            return None;
        }
    };

    Some(SampleInsert {
        file_hash,
        source_path: path.display().to_string(),
        file_name: meta.file_name.clone(),
        source_ext: meta.source_extension.clone(),
        width: meta.width,
        height: meta.height,
        duration_secs: meta.duration_secs,
        frame_count: meta.frame_count,
        file_size_bytes: meta.file_size_bytes,
        fps: meta.fps,
        has_embedded_icc: meta.has_embedded_icc,
        has_complex_color_profile: meta.has_complex_color_profile,
        has_transparency: meta.has_transparency,
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
        is_meme_platform,
        is_human_semantic_name,
        cadence_score,
        directory_loop_intent_score,
        is_high_value_source,
        is_native_gif,
        palette_depth: meta.palette_depth,
        motion_gini: meta.motion_gini,
        block_skew: meta.block_skew,
        temporal_flatness: meta.temporal_flatness,
        webp_compression_ratio: meta.webp_compression_ratio,
        loop_verdict: match loss_tolerance.as_str() {
            "high" => "LoopStrong".to_string(),
            "low" => "LoopWeak".to_string(),
            _ => "Uncertain".to_string(),
        },
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

/// Compute a 31-dimensional pgvector encoding for a sample using pre-calculated std deviations.
/// This precisely bakes the weights and normalization terms from the old dynamically computed KNN
/// into an L2-compatible vector, allowing `PostgreSQL`'s HNSW index to do the heavy lifting!
fn sample_row_from_meta(meta: &LoopMeta, temporal_bpp: f64, spatial_bpp: f64) -> SampleRow {
    SampleRow {
        _loss_tolerance: None,
        width: meta.width,
        height: meta.height,
        duration_secs: meta.duration_secs,
        frame_count: meta.frame_count,
        file_size_bytes: meta.file_size_bytes,
        fps: meta.fps,
        temporal_bpp,
        spatial_bpp,
        has_transparency: meta.has_transparency,
        has_embedded_icc: meta.has_embedded_icc,
        has_complex_color_profile: meta.has_complex_color_profile,
        palette_size: meta.palette_size,
        frame_payload_variation: meta.frame_payload_variation,
        frame_delay_variation: meta.frame_delay_variation,
        aspect_ratio: (meta.height > 0).then(|| f64::from(meta.width) / f64::from(meta.height)),
        _total_pixels: Some(u64::from(meta.width) * u64::from(meta.height)),
        loop_frequency: Some(crate::loop_intent::score_loop_frequency(
            meta.duration_secs,
            meta.frame_count,
        )),
        is_meme_platform: meta.is_meme_platform,
        is_human_semantic_name: crate::loop_intent::analyze_filename(
            meta.file_name.as_deref(),
            &[],
        )
        .kind
            == crate::loop_intent::FilenameKind::HumanSemantic,
        cadence_score: Some(crate::loop_intent::score_sparse_cadence(
            meta.duration_secs,
            meta.frame_count,
        )),
        directory_loop_intent_score: Some(crate::loop_intent::score_directory_context(
            meta.parent_directories.as_deref(),
            &[],
        )),
        is_high_value_source: meta.has_embedded_icc || meta.has_complex_color_profile,

        is_native_gif: meta.is_native_gif,
        palette_depth: meta.palette_depth,
        motion_gini: meta.motion_gini,
        block_skew: meta.block_skew,
        temporal_flatness: meta.temporal_flatness,
        loop_closure_score: meta.loop_closure_score,
        motion_periodicity: meta.motion_periodicity,
        temporal_jitter: meta.temporal_jitter,
        webp_compression_ratio: meta.webp_compression_ratio,
        _labeled_by: None,
    }
}

fn bpp_from_meta(meta: &LoopMeta) -> (f64, f64) {
    let pixel_count = (f64::from(meta.width) * f64::from(meta.height)).max(1.0);
    let file_size = crate::numeric_cast::u64_to_f64(meta.file_size_bytes);
    let frame_count = meta.frame_count.map_or_else(
        || {
            tracing::warn!("Training: Missing 'frame_count' for Bits Per Pixel (BPP) calculation; assuming static (1.0) to avoid division by zero");
            1.0
        },
        |fc| crate::numeric_cast::u64_to_f64(fc.max(1)),
    );

    (
        file_size / (pixel_count * frame_count),
        file_size / pixel_count,
    )
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
    let target = sample_row_from_meta(meta, target_temporal_bpp, target_spatial_bpp);
    let target_vector = crate::database_vector::compute_sample_vector(&target, stats_map)
        .expect("KNN: Target search vector computation failed during distance calculation");
    let sample_vector = crate::database_vector::compute_sample_vector(sample, stats_map)
        .expect("KNN: Sample search vector computation failed during distance calculation");
    vector_l2_distance(&target_vector, &sample_vector)
}

fn adaptive_neighbor_count(total: usize) -> Result<usize, String> {
    let count = crate::numeric_cast::u64_to_usize_strict(
        crate::numeric_cast::f64_to_u64_strict(
            (crate::numeric_cast::usize_to_f64(total)).sqrt().round(),
            "adaptive_neighbor_total_rounded",
        )
        .ok_or_else(|| "❌ Failed to calculate adaptive neighbor count".to_string())?,
        "adaptive_neighbor_total",
    )
    .ok_or_else(|| "❌ Adaptive neighbor count overflowed usize".to_string())?;

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
    distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = *distances.get(distances.len() / 4).unwrap_or(&0.0_f64);
    let q3 = *distances.get((distances.len() * 3) / 4).unwrap_or(&0.0_f64);
    let iqr = (q3 - q1).max(0.06);
    let d0 = *distances.first().unwrap_or(&0.0_f64);
    (d0 + iqr * 1.5).max(d0 + 0.08)
}

pub(crate) fn normalize_log_ratio(a: f64, b: f64, scale: f64) -> f64 {
    if a <= 0.0_f64 || b <= 0.0_f64 || scale <= 0.0_f64 {
        return 1.0;
    }
    ((a.ln() - b.ln()).abs() / scale).clamp(0.0, 1.0)
}

fn percentile_value(sorted_values: &[f64], quantile: f64) -> Option<f64> {
    if sorted_values.is_empty() {
        return None;
    }

    let clamped = quantile.clamp(0.0, 1.0);
    let scaled_index =
        clamped * crate::numeric_cast::usize_to_f64(sorted_values.len().saturating_sub(1));
    let lower_index = crate::numeric_cast::f64_to_usize_strict(scaled_index.floor(), "lower_index")
        .unwrap_or_else(|| {
            tracing::warn!(
                "Training: Numerical overflow calculating percentile lower index; defaulting to 0"
            );
            0
        });
    let upper_index = crate::numeric_cast::f64_to_usize_strict(scaled_index.ceil(), "upper_index")
        .unwrap_or_else(|| {
            tracing::warn!(
                "Training: Numerical overflow calculating percentile upper index; defaulting to 0"
            );
            0
        });

    if lower_index == upper_index {
        return sorted_values.get(lower_index).copied();
    }

    let lower = sorted_values.get(lower_index).copied()?;
    let upper = sorted_values.get(upper_index).copied()?;
    Some((upper - lower).mul_add(
        scaled_index - crate::numeric_cast::usize_to_f64(lower_index),
        lower,
    ))
}

fn build_feature_stats(values: &[f64]) -> FeatureStats {
    if values.is_empty() {
        return FeatureStats::default();
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
    sorted.sort_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap_or(std::cmp::Ordering::Equal));

    FeatureStats {
        mean,
        std_dev: variance.sqrt(),
        weight: None,
        p10: percentile_value(&sorted, 0.10),
        p25: percentile_value(&sorted, 0.25),
        p50: percentile_value(&sorted, 0.50),
        p75: percentile_value(&sorted, 0.75),
        p90: percentile_value(&sorted, 0.90),
    }
}

/// Scan a directory tree for media files and ingest them into the database
/// as training samples.
///
/// Walks the given path, extracts sample metadata for each GIF/WebP/APNG/AVIF/MP4/MOV
/// file, computes feature vectors, and inserts them into the `samples` table.
/// Static images (`frame_count` <= 1) are excluded. Returns the number of
/// successfully ingested samples.
///
/// # Errors
/// Returns an error if the database cannot be initialized or the walk fails.
///
/// # Panics
/// Panics if the progress bar template is invalid.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn batch_ingest_samples(dataset_path: &Path, label_override: Option<&str>) -> Result<usize> {
    let mut conn = open_pg_client()?;

    init_schema(&mut conn)?;
    seed_positive_dataset_if_needed(&mut conn)?;

    println!(
        "🔍 Scanning for candidate assets in {}...",
        dataset_path.display()
    );
    let mut candidate_paths = Vec::new();
    for entry in WalkDir::new(dataset_path)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path().to_path_buf();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ["gif", "webp", "apng", "avif", "mp4", "mov"].contains(&ext.as_str()) {
            candidate_paths.push(path);
        }
    }

    if candidate_paths.is_empty() {
        println!("⚠️ No matching assets found in designated path.");
        println!("✅ Successfully initialized PostgreSQL with default seeded samples.");
        return Ok(0);
    }

    let pb = ProgressBar::new(candidate_paths.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .expect("Invalid progress bar template")
            .progress_chars("#>-"),
    );

    println!("🧠 Learning from {} samples...", candidate_paths.len());

    // Process in parallel to speed up ffprobe and GIF header scanning
    let samples: Vec<_> = candidate_paths
        .par_iter()
        .filter_map(|path| {
            let res = sample_from_path(path, "cli_ingest", label_override);
            pb.inc(1);
            if let Some(s) = &res {
                // Exclude static images: only multi-frame content is valuable for loop intent training
                if s.frame_count.is_none_or(|fc| fc <= 1) {
                    return None;
                }
                pb.set_message(format!("Learn: {}", s.file_name.as_deref().unwrap_or("?")));
            }
            res
        })
        .collect();

    pb.finish_with_message("Learning complete.");

    println!("💾 Persisting {} samples to database...", samples.len());

    let initial_feature_map = fetch_feature_map(&mut conn)?;
    let mut tx = conn.transaction()?;
    let mut count = 0;

    let stmt = tx.prepare(
        "INSERT INTO samples (
            file_hash, source_path, file_name, source_ext,
            width, height, duration_secs, frame_count, file_size_bytes, fps,
            has_embedded_icc, has_complex_color_profile, has_transparency,
            palette_size, frame_payload_variation, frame_delay_variation,
            temporal_bpp, spatial_bpp, loss_tolerance, labeled_by, aspect_ratio,
            total_pixels, loop_frequency, is_meme_platform, is_human_semantic_name,
            cadence_score, directory_loop_intent_score, is_high_value_source, is_native_gif,
            palette_depth, motion_gini, block_skew, temporal_flatness, webp_compression_ratio,
            loop_verdict, features
         ) VALUES (
            $1, $2, $3, $4,
            $5, $6, $7, $8, $9, $10,
            $11, $12, $13,
            $14, $15, $16,
            $17, $18, $19, $20, $21,
            $22, $23, $24, $25, $26, $27, $28, $29,
            $30, $31, $32, $33, $34, $35, $36::vector
         )
         ON CONFLICT (file_hash) DO UPDATE SET
            features = EXCLUDED.features,
            source_path = EXCLUDED.source_path,
            file_name = EXCLUDED.file_name,
            source_ext = EXCLUDED.source_ext,
            width = EXCLUDED.width,
            height = EXCLUDED.height,
            duration_secs = EXCLUDED.duration_secs,
            frame_count = EXCLUDED.frame_count,
            file_size_bytes = EXCLUDED.file_size_bytes,
            fps = EXCLUDED.fps,
            has_embedded_icc = EXCLUDED.has_embedded_icc,
            has_complex_color_profile = EXCLUDED.has_complex_color_profile,
            has_transparency = EXCLUDED.has_transparency,
            palette_size = EXCLUDED.palette_size,
            frame_payload_variation = EXCLUDED.frame_payload_variation,
            frame_delay_variation = EXCLUDED.frame_delay_variation,
            temporal_bpp = EXCLUDED.temporal_bpp,
            spatial_bpp = EXCLUDED.spatial_bpp,
            loss_tolerance = EXCLUDED.loss_tolerance,
            labeled_by = EXCLUDED.labeled_by,
            aspect_ratio = EXCLUDED.aspect_ratio,
            total_pixels = EXCLUDED.total_pixels,
            loop_frequency = EXCLUDED.loop_frequency,
            is_meme_platform = EXCLUDED.is_meme_platform,
            is_human_semantic_name = EXCLUDED.is_human_semantic_name,
            cadence_score = EXCLUDED.cadence_score,
            directory_loop_intent_score = EXCLUDED.directory_loop_intent_score,
            is_high_value_source = EXCLUDED.is_high_value_source,
            is_native_gif = EXCLUDED.is_native_gif,
            palette_depth = EXCLUDED.palette_depth,
            motion_gini = EXCLUDED.motion_gini,
            block_skew = EXCLUDED.block_skew,
            temporal_flatness = EXCLUDED.temporal_flatness,
            webp_compression_ratio = EXCLUDED.webp_compression_ratio,
            loop_verdict = EXCLUDED.loop_verdict",
    )?;

    for sample in samples {
        let palette_size_i32 = if let Some(s) = sample.palette_size {
            let Some(res) = crate::numeric_cast::u32_to_i32_strict(s, "db_pal_size") else {
                tracing::warn!(
                    "Database: 'palette_size' ({s}) out of i32 range; skipping sample {}",
                    sample.file_hash
                );
                continue;
            };
            Some(res)
        } else {
            None
        };

        let Some(total_pixels_i64) =
            crate::numeric_cast::u64_to_i64_strict(sample.total_pixels, "db_total_pixels")
        else {
            tracing::warn!(
                "Database: 'total_pixels' out of i64 range; skipping sample {}",
                sample.file_hash
            );
            continue;
        };

        let frame_count_i64 = if let Some(fc) = sample.frame_count {
            let Some(res) = crate::numeric_cast::u64_to_i64_strict(fc, "db_frame_count") else {
                tracing::warn!(
                    "Database: 'frame_count' ({fc}) out of i64 range; skipping sample {}",
                    sample.file_hash
                );
                continue;
            };
            res
        } else {
            tracing::warn!(
                "Database: 'frame_count' metadata missing for sample {}; skipping",
                sample.file_hash
            );
            continue;
        };

        let Some(file_size_i64) =
            crate::numeric_cast::u64_to_i64_strict(sample.file_size_bytes, "db_file_size")
        else {
            tracing::warn!(
                "Database: 'file_size' out of i64 range; skipping sample {}",
                sample.file_hash
            );
            continue;
        };

        let Some(width_i32) = crate::numeric_cast::u32_to_i32_strict(sample.width, "db_width")
        else {
            tracing::warn!(
                "Database: 'width' ({}) out of i32 range; skipping sample {}",
                sample.width,
                sample.file_hash
            );
            continue;
        };

        let Some(height_i32) = crate::numeric_cast::u32_to_i32_strict(sample.height, "db_height")
        else {
            tracing::warn!(
                "Database: 'height' ({}) out of i32 range; skipping sample {}",
                sample.height,
                sample.file_hash
            );
            continue;
        };

        let sample_row = SampleRow::from(sample.clone());
        let Some(vec_data) =
            crate::database_vector::compute_sample_vector(&sample_row, &initial_feature_map)
        else {
            tracing::warn!(
                "Skipping pgvector insertion for {} due to missing features",
                sample.file_hash
            );
            continue;
        };
        let pg_vector = pgvector::Vector::from(vec_data);

        let res = tx.execute(
            &stmt,
            &[
                &sample.file_hash,
                &sample.source_path,
                &sample.file_name,
                &sample.source_ext,
                &width_i32,
                &height_i32,
                &sample.duration_secs,
                &frame_count_i64,
                &file_size_i64,
                &sample.fps,
                &sample.has_embedded_icc,
                &sample.has_complex_color_profile,
                &sample.has_transparency,
                &palette_size_i32,
                &sample.frame_payload_variation,
                &sample.frame_delay_variation,
                &sample.temporal_bpp,
                &sample.spatial_bpp,
                &sample.loss_tolerance,
                &sample.labeled_by,
                &sample.aspect_ratio,
                &total_pixels_i64,
                &sample.loop_frequency,
                &sample.is_meme_platform,
                &sample.is_human_semantic_name,
                &sample.cadence_score,
                &sample.directory_loop_intent_score,
                &sample.is_high_value_source,
                &sample.is_native_gif,
                &sample.palette_depth,
                &sample.motion_gini,
                &sample.block_skew,
                &sample.temporal_flatness,
                &sample.webp_compression_ratio,
                &sample.loop_verdict,
                &pg_vector,
            ],
        );
        if res.is_ok() {
            count += 1;
        }
    }

    tx.commit()?;
    recompute_all_features(&mut conn)?;
    Ok(count)
}

/// Recompute global feature statistics and update the training model.
///
/// Queries all labeled samples, computes per-feature statistics (mean,
/// `std_dev`, percentiles), extracts dynamic keywords from `LoopStrong`
/// filenames, computes discriminative power for each feature, and stores
/// the resulting `FeatureMap` in the metadata table. Also triggers
/// pgvector feature backfill.
///
/// # Errors
/// Returns an error if the database queries fail.
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn refresh_feature_stats(conn: &mut Client) -> Result<()> {
    emit_stderr("🏋️  Recomputing Global KNN Feature Statistics (Training Model)...");

    // Feature Integrity Check: Re-probe samples that were previously broken (e.g. motion_gini = 0.0)
    let broken_rows = conn.query(
        "SELECT file_hash, source_path FROM samples WHERE motion_gini = 0.0 AND frame_count > 1",
        &[],
    )?;
    if !broken_rows.is_empty() {
        emit_stderr(&format!(
            "   🛠️  Found {} samples with outdated feature metrics. Refreshing integrity...",
            broken_rows.len()
        ));
        let mut fixed_count = 0_i32;
        for row in broken_rows {
            let file_hash: String = row.get(0);
            let source_path: Option<String> = row.get(1);
            if let Some(path_str) = source_path {
                let path = Path::new(&path_str);
                if path.exists()
                    && let Some(sample) = sample_from_path(path, "integrity_refresh", None)
                {
                    conn.execute(
                        "UPDATE samples SET 
                                motion_gini = $1, 
                                directory_loop_intent_score = $2,
                                temporal_flatness = $3,
                                palette_depth = $4
                             WHERE file_hash = $5",
                        &[
                            &sample.motion_gini,
                            &sample.directory_loop_intent_score,
                            &sample.temporal_flatness,
                            &sample.palette_depth,
                            &file_hash,
                        ],
                    )?;

                    fixed_count += 1_i32;
                }
            }
        }
        if fixed_count > 0_i32 {
            emit_stderr(&format!(
                "   ✅ Refreshed feature integrity for {fixed_count} labeled samples."
            ));
        }
    }

    let rows = conn.query(
        "SELECT
            width, height, duration_secs, frame_count, file_size_bytes, fps,
            temporal_bpp, spatial_bpp, frame_payload_variation, frame_delay_variation,
            aspect_ratio, loop_frequency, cadence_score, palette_depth, motion_gini,
            block_skew, temporal_flatness, webp_compression_ratio
         FROM samples WHERE loss_tolerance IS NOT NULL AND frame_count > 1",
        &[],
    )?;

    if rows.is_empty() {
        emit_stderr("⚠️  Retraining aborted: No labeled training samples found in database.");
        return Ok(());
    }

    emit_stderr(&format!(
        "   📊 Analyzing {} training samples...",
        rows.len()
    ));

    let all_data: Vec<Vec<f64>> = rows
        .iter()
        .filter_map(|row| {
            let duration = row.get::<_, f64>(2);
            let frame_count = f64::from(crate::numeric_cast::i64_to_u32_strict(
                row.get::<_, i64>(3),
                "db_frame_count",
            )?);
            let fps =
                crate::numeric_cast::option_f64_strict(row.get::<_, Option<f64>>(5), "db_fps")?;

            let density = if duration > 0.05_f64 {
                frame_count / duration
            } else {
                fps
            };
            let gap = if frame_count > 0.0_f64 {
                duration / frame_count
            } else {
                duration
            };

            Some(vec![
                f64::from(row.get::<_, i32>(0)) * f64::from(row.get::<_, i32>(1)), // pixels
                duration,
                frame_count,
                crate::numeric_cast::i64_to_f64(row.get::<_, i64>(4)), // file_size_bytes
                fps,
                density,
                gap,
                row.get::<_, f64>(6), // temporal_bpp
                row.get::<_, f64>(7), // spatial_bpp
                crate::numeric_cast::option_f64_strict(row.get::<_, Option<f64>>(10), "db_aspect")?, // aspect
                row.get::<_, Option<f64>>(11)?, // loop_freq
                row.get::<_, Option<f64>>(12)?, // cadence
                row.get::<_, Option<f64>>(8)?,  // payload_var
                row.get::<_, Option<f64>>(9)?,  // delay_var
                row.get::<_, Option<f64>>(13)?, // p_depth
                row.get::<_, Option<f64>>(14)?, // m_gini
                row.get::<_, Option<f64>>(15)?, // b_skew
                row.get::<_, Option<f64>>(16)?, // t_flat
                crate::numeric_cast::option_f64_strict(
                    row.get::<_, Option<f64>>(17),
                    "db_webp_ratio",
                )?, // webp_ratio
            ])
        })
        .collect();

    if all_data.is_empty() {
        return Ok(());
    }

    let names = vec![
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
    ];

    // 1. Extract dynamic keywords from SampleStrong filenames
    let keyword_rows = conn.query(
        "WITH words AS (
             SELECT unnest(regexp_split_to_array(lower(file_name), '[^a-z0-9]+')) as word 
             FROM samples 
             WHERE loop_verdict = 'LoopStrong' AND file_name IS NOT NULL
         )
         SELECT word FROM words 
         WHERE length(word) > 2 AND word !~ '^[0-9]+$'
         GROUP BY word 
         ORDER BY COUNT(*) DESC 
         LIMIT 50",
        &[],
    )?;
    let top_keywords: Vec<String> = keyword_rows.iter().map(|r| r.get(0)).collect();
    if !top_keywords.is_empty() {
        emit_stderr(&format!(
            "   🔍 Extracted {} dynamic loop triggers from filenames.",
            top_keywords.len()
        ));
    }

    let mut feature_map = FeatureMap {
        top_keywords: top_keywords.clone(),
        ..Default::default()
    };

    for (idx, name) in names.iter().enumerate() {
        let values: Vec<f64> = all_data
            .iter()
            .filter_map(|v| {
                crate::numeric_cast::option_f64_strict(v.get(idx).copied(), "feature_matrix_entry")
            })
            .collect();
        feature_map
            .stats
            .insert(name.to_string(), build_feature_stats(&values));
    }

    // Assign learned dynamic weights based on data-driven discriminative power
    if let Ok(powers) = query_feature_discriminative_power(conn) {
        for power in powers {
            // Map DB column names back to feature_map string keys
            let mapped_name = match power.feature_name.as_str() {
                "duration_secs" => "duration",
                "fps" => "fps",
                "file_size_bytes" => "file_size_bytes",
                "temporal_bpp" => "temporal_bpp",
                "spatial_bpp" => "spatial_bpp",
                "frame_payload_variation" => "payload_var",
                "frame_delay_variation" => "delay_var",
                "palette_depth" => "p_depth",
                "motion_gini" => "m_gini",
                "temporal_flatness" => "t_flat",
                "webp_compression_ratio" => "webp_ratio",
                "cadence_score" => "cadence",
                "loop_frequency" => "loop_freq",
                _ => continue,
            };
            if let Some(stat) = feature_map.stats.get_mut(mapped_name) {
                // Ensure weights are positive and scale gently for the euclidean distance.
                // Clamped to avoid massive dominant features ruining the search radius.
                stat.weight = Some(power.discriminative_power.abs().clamp(0.01, 10.0));
            }
        }
    }

    let json = serde_json::to_string(&feature_map)?;
    conn.execute(
        "INSERT INTO sample_metadata (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = $2",
        &[&STATS_KEY, &json],
    )?;

    // Calculate Global Collection Stats
    let stats_row = conn.query_one(
        "SELECT 
            MIN(duration_secs), AVG(duration_secs), MAX(duration_secs),
            MIN(file_size_bytes), AVG(file_size_bytes)::DOUBLE PRECISION, MAX(file_size_bytes),
            MIN(width), AVG(width)::DOUBLE PRECISION, MAX(width),
            MIN(height), AVG(height)::DOUBLE PRECISION, MAX(height),
            MIN(aspect_ratio), AVG(aspect_ratio), MAX(aspect_ratio)
         FROM samples WHERE is_native_gif = TRUE",
        &[],
    )?;

    let dur_min: f64 = stats_row.get(0);
    let dur_avg: f64 = stats_row.get(1);
    let dur_max: f64 = stats_row.get(2);

    let size_min_i64: i64 = stats_row.get(3);
    let size_avg: f64 = stats_row.get(4);
    let size_max_i64: i64 = stats_row.get(5);

    let w_min_i32: i32 = stats_row.get(6);
    let w_avg: f64 = stats_row.get(7);
    let w_max_i32: i32 = stats_row.get(8);

    let h_min_i32: i32 = stats_row.get(9);
    let h_avg: f64 = stats_row.get(10);
    let h_max_i32: i32 = stats_row.get(11);

    let aspect_min: f64 = stats_row.get(12);
    let aspect_avg: f64 = stats_row.get(13);
    let aspect_max: f64 = stats_row.get(14);

    // Estimate bitrate stats (size / duration)
    let bitrate_row = conn.query_one(
        "SELECT
            MIN(file_size_bytes::DOUBLE PRECISION * 8.0 / NULLIF(duration_secs, 0.0)),
            AVG(file_size_bytes::DOUBLE PRECISION * 8.0 / NULLIF(duration_secs, 0.0))::DOUBLE PRECISION,
            MAX(file_size_bytes::DOUBLE PRECISION * 8.0 / NULLIF(duration_secs, 0.0))
         FROM samples WHERE is_native_gif = TRUE AND duration_secs > 0",
        &[],
    )?;

    let collection_stats = GlobalCollectionStats {
        duration_min: Some(dur_min),
        duration_avg: Some(dur_avg),
        duration_max: Some(dur_max),
        duration_p90: Some(
            feature_map
                .stats
                .get("duration")
                .and_then(|stats| stats.p90)
                .unwrap_or(dur_avg),
        ),

        size_min: crate::numeric_cast::i64_to_f64(size_min_i64.max(0)),
        size_avg,
        size_max: crate::numeric_cast::i64_to_f64(size_max_i64.max(0)),

        bitrate_min: bitrate_row.get(0),
        bitrate_avg: bitrate_row.get(1),
        bitrate_max: bitrate_row.get(2),

        width_min: f64::from(w_min_i32),
        width_avg: w_avg,
        width_max: f64::from(w_max_i32),
        height_min: f64::from(h_min_i32),
        height_avg: h_avg,
        height_max: f64::from(h_max_i32),

        aspect_min,
        aspect_avg,
        aspect_max,
        top_keywords,
    };

    let col_json = serde_json::to_string(&collection_stats)?;

    conn.execute(
        "INSERT INTO sample_metadata (key, value) VALUES ('collection_stats_v1', $1)
         ON CONFLICT (key) DO UPDATE SET value = $1",
        &[&col_json],
    )?;

    recompute_all_features(conn)?;
    Ok(())
}

/// Recompute pgvector feature encodings for all labeled samples.
///
/// Reads each sample's features, computes the 31-dimensional vector
/// using the current `FeatureMap`, and writes it back to the `samples`
/// table for HNSW index usage.
///
/// # Errors
/// Returns an error if the database transaction or query fails.
#[allow(
    clippy::missing_panics_doc,
    reason = "Explicit panic on data corruption is intended and documented inline."
)]
fn map_row_to_sample(row: &postgres::Row) -> Option<SampleRow> {
    let width = crate::numeric_cast::i32_to_u32_strict(row.get::<_, i32>(2), "db_backfill_width")?;
    let height =
        crate::numeric_cast::i32_to_u32_strict(row.get::<_, i32>(3), "db_backfill_height")?;
    let file_size_bytes =
        crate::numeric_cast::i64_to_u64_strict(row.get::<_, i64>(6), "db_backfill_file_size")?;
    let fps =
        crate::numeric_cast::option_f64_strict(row.get::<_, Option<f64>>(7), "db_backfill_fps");

    Some(SampleRow {
        _loss_tolerance: row.get(1),
        width,
        height,
        duration_secs: row.get(4),
        frame_count: crate::numeric_cast::i64_to_u64_strict(
            row.get::<_, i64>(5),
            "db_backfill_frame_count",
        ),
        file_size_bytes,
        fps,
        temporal_bpp: row.get(8),
        spatial_bpp: row.get(9),
        has_transparency: row.get(10),
        has_embedded_icc: row.get(11),
        has_complex_color_profile: row.get(12),
        palette_size: row.get::<_, Option<i32>>(13).and_then(|s| {
            crate::numeric_cast::i32_to_u32_strict(s, "db_backfill_pal").or_else(|| {
                crate::progress_mode::emit_stderr(
                    "☢️ [ANOMALY] DB palette_size corruption! Refusing to forge data.",
                );
                None
            })
        }),
        frame_payload_variation: row.get(14),
        frame_delay_variation: row.get(15),
        aspect_ratio: row.get(16),
        _labeled_by: row.get(17),
        _total_pixels: row.get::<_, Option<i64>>(18).and_then(|s| {
            crate::numeric_cast::i64_to_u64_strict(s, "db_backfill_total_pixels").or_else(|| {
                crate::progress_mode::emit_stderr("☢️ [ANOMALY] DB total_pixels corruption! Refusing to forge data. Information invalidated.");
                None
            })
        }),
        loop_frequency: row.get(19),
        is_meme_platform: row.get(20),
        is_human_semantic_name: row.get(21),
        cadence_score: row.get(22),
        directory_loop_intent_score: row.get::<_, Option<f64>>(23),
        is_high_value_source: row.get(24),
        is_native_gif: row.get(25),
        palette_depth: row.get(26),
        motion_gini: row.get(27),
        block_skew: row.get(28),
        temporal_flatness: row.get(29),
        loop_closure_score: row.get(30),
        motion_periodicity: row.get(31),
        temporal_jitter: row.get(32),
        webp_compression_ratio: row.get::<_, Option<f64>>(33),
    })
}

/// # Errors
/// Returns an error if the database transaction or query fails.
pub fn recompute_all_features(conn: &mut Client) -> Result<()> {
    let feature_map = fetch_feature_map(conn)?;

    // ── pgvector HNSW Migration: Backfill Vectors ──
    tracing::debug!("Backfilling pgvector encodings for all labeled samples");

    let sample_rows = conn.query(
        "SELECT
            file_hash, loss_tolerance, width, height, duration_secs, frame_count, file_size_bytes,
            fps, temporal_bpp, spatial_bpp,
            has_transparency, has_embedded_icc, has_complex_color_profile,
            palette_size, frame_payload_variation, frame_delay_variation,
            aspect_ratio, labeled_by,
            total_pixels, loop_frequency, is_meme_platform, is_human_semantic_name,
            cadence_score, directory_loop_intent_score, is_high_value_source, is_native_gif,
            palette_depth, motion_gini, block_skew, temporal_flatness,
            loop_closure_score, motion_periodicity, temporal_jitter, webp_compression_ratio
         FROM samples WHERE loss_tolerance IS NOT NULL AND frame_count > 1",
        &[],
    )?;

    let mut tx = conn.transaction()?;
    let stmt = tx.prepare("UPDATE samples SET features = $1::vector WHERE file_hash = $2")?;

    let mut updated_count = 0_i32;
    for row in &sample_rows {
        let file_hash: String = row.get(0);
        let Some(sample) = map_row_to_sample(row) else {
            crate::progress_mode::emit_stderr(&format!(
                "☢️ [ANOMALY] Row corruption for {file_hash}. Skipping vector backfill."
            ));
            continue;
        };

        let Some(vec_data) = crate::database_vector::compute_sample_vector(&sample, &feature_map)
        else {
            tracing::warn!(
                "Skipping pgvector backfill for {} due to missing features",
                file_hash
            );
            continue;
        };
        let pg_vector = pgvector::Vector::from(vec_data);
        tx.execute(&stmt, &[&pg_vector, &file_hash])?;
        updated_count += 1_i32;
    }
    tx.commit()?;
    emit_stderr(&format!(
        "   ✅ pgvector backfill complete ({updated_count} samples encoded)."
    ));

    emit_stderr("✅ KNN Model Training Complete: Internal statistics synchronized.");
    Ok(())
}

// ── Level 4: Inference Logging ───────────────────────────────────────────────

/// Build a JSON snapshot of key `LoopMeta` fields for the inference log.
fn build_signal_snapshot(meta: &LoopMeta) -> Value {
    let f64_safe = |v: f64| if v.is_finite() { json!(v) } else { json!(null) };
    let opt_f64_safe = |v: Option<f64>| {
        v.and_then(|x| if x.is_finite() { Some(json!(x)) } else { None })
            .unwrap_or(json!(null))
    };

    json!({
        "duration_secs": meta.duration_secs.map_or_else(|| json!(null), |v| if v.is_finite() { json!(v) } else { json!(0.0) }),
        "width": meta.width,
        "height": meta.height,
        "fps": opt_f64_safe(meta.fps),
        "frame_count": meta.frame_count,
        "file_size_bytes": meta.file_size_bytes,
        "has_audio": meta.has_audio,
        "has_transparency": meta.has_transparency,
        "is_native_gif": meta.is_native_gif,
        "has_embedded_icc": meta.has_embedded_icc,
        "has_complex_color_profile": meta.has_complex_color_profile,
        "is_meme_platform": meta.is_meme_platform,
        "loop_count": meta.loop_count,
        "webp_compression_ratio": opt_f64_safe(meta.webp_compression_ratio),
        "palette_depth": opt_f64_safe(meta.palette_depth),
        "motion_gini": opt_f64_safe(meta.motion_gini),
        "temporal_flatness": opt_f64_safe(meta.temporal_flatness),
        "block_skew": opt_f64_safe(meta.block_skew),
        "frame_payload_variation": opt_f64_safe(meta.frame_payload_variation),
        "frame_delay_variation": opt_f64_safe(meta.frame_delay_variation),
        "directory_loop_intent_score": f64_safe(meta.directory_loop_intent_score),
        "filename_loop_intent_score": f64_safe(meta.filename_loop_intent_score),
        "source_extension": meta.source_extension,
        "container": meta.container,
    })
}

/// Log one inference record to the database for later analysis.
///
/// Fails silently -- never blocks the pipeline. Called after every
/// verdict to build the feedback loop. Stores the meta snapshot,
/// tree/KNN probabilities, final verdict, and layer exit information.
#[allow(
    clippy::missing_panics_doc,
    reason = "Explicit panic on data corruption is intended and documented inline."
)]
pub fn log_inference_record(
    conn: &mut Client,
    meta: &LoopMeta,
    record: &LoopInferenceRecord,
    path: Option<&Path>,
) {
    let file_hash: Option<String> = path.and_then(|p| {
        calculate_blake3_hex(p)
            .map_err(|e| {
                warn!(
                    path = %p.display(),
                    error = %e,
                    "☢️ [ANOMALY] Failed to calculate file hash! Refusing to forge record data."
                );
                e
            })
            .ok()
    });
    let source_path: Option<String> = path.map(|p| p.display().to_string());
    let snapshot = build_signal_snapshot(meta);

    let knn_neighbor_count_i32 = record.knn_neighbor_count.and_then(|s| {
        crate::numeric_cast::usize_to_i32_strict(s, "db_knn_count").or_else(|| {
            crate::progress_mode::emit_stderr(
                "☢️ [ANOMALY] KNN count overflow! Refusing to forge record data.",
            );
            None
        })
    });

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

    // snapshot is Value from build_signal_snapshot(meta)

    let result = conn.execute(
        "INSERT INTO inference_log (
            file_hash, source_path, duration_secs, webp_compression_ratio,
            tree_probability, knn_keep_probability, knn_confidence, knn_neighbor_count,
            final_probability, final_verdict, decision_reason, layer_exit,
            signal_snapshot
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::jsonb)",
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
        ],
    );

    if let Err(e) = result {
        log::warn!(
            "⚠️ Failed to write inference log (non-fatal): {e} | Parameter count: 13 | Exit Layer: {layer_exit}"
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
    // We pivot the numeric columns into (feature_name, value) rows, then compute
    // per-feature statistics grouped by the loss_tolerance label.
    let rows = conn.query(
        "WITH feature_pivoted AS (
            SELECT loss_tolerance, 'duration_secs' AS feature_name, duration_secs AS value FROM samples WHERE loss_tolerance IN ('high', 'video')
            UNION ALL SELECT loss_tolerance, 'fps', fps FROM samples WHERE loss_tolerance IN ('high', 'video') AND fps IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'file_size_bytes', file_size_bytes::DOUBLE PRECISION FROM samples WHERE loss_tolerance IN ('high', 'video')
            UNION ALL SELECT loss_tolerance, 'temporal_bpp', temporal_bpp FROM samples WHERE loss_tolerance IN ('high', 'video')
            UNION ALL SELECT loss_tolerance, 'spatial_bpp', spatial_bpp FROM samples WHERE loss_tolerance IN ('high', 'video')
            UNION ALL SELECT loss_tolerance, 'frame_payload_variation', frame_payload_variation FROM samples WHERE loss_tolerance IN ('high', 'video') AND frame_payload_variation IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'frame_delay_variation', frame_delay_variation FROM samples WHERE loss_tolerance IN ('high', 'video') AND frame_delay_variation IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'palette_depth', palette_depth FROM samples WHERE loss_tolerance IN ('high', 'video') AND palette_depth IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'motion_gini', motion_gini FROM samples WHERE loss_tolerance IN ('high', 'video') AND motion_gini IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'temporal_flatness', temporal_flatness FROM samples WHERE loss_tolerance IN ('high', 'video') AND temporal_flatness IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'webp_compression_ratio', webp_compression_ratio FROM samples WHERE loss_tolerance IN ('high', 'video') AND webp_compression_ratio IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'directory_loop_intent_score', directory_loop_intent_score FROM samples WHERE loss_tolerance IN ('high', 'video') AND directory_loop_intent_score IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'cadence_score', cadence_score FROM samples WHERE loss_tolerance IN ('high', 'video') AND cadence_score IS NOT NULL
            UNION ALL SELECT loss_tolerance, 'loop_frequency', loop_frequency FROM samples WHERE loss_tolerance IN ('high', 'video') AND loop_frequency IS NOT NULL
        )
        SELECT
            feature_name,
            AVG(value) FILTER (WHERE loss_tolerance = 'high') AS mean_loop_strong,
            AVG(value) FILTER (WHERE loss_tolerance = 'video') AS mean_loop_weak,
            CASE WHEN NULLIF(STDDEV(value), 0) IS NOT NULL THEN
                (COALESCE(AVG(value) FILTER (WHERE loss_tolerance = 'high'), 0) -
                 COALESCE(AVG(value) FILTER (WHERE loss_tolerance = 'video'), 0))
                / STDDEV(value)
            ELSE 0 END AS discriminative_power,
            COUNT(*) AS sample_count
        FROM feature_pivoted
        GROUP BY feature_name
        ORDER BY ABS(
            CASE WHEN NULLIF(STDDEV(value), 0) IS NOT NULL THEN
                (COALESCE(AVG(value) FILTER (WHERE loss_tolerance = 'high'), 0) -
                 COALESCE(AVG(value) FILTER (WHERE loss_tolerance = 'video'), 0))
                / STDDEV(value)
            ELSE 0 END
        ) DESC",
        &[],
    )?;

    Ok(rows
        .iter()
        .map(|row| LoopFeatureDiscriminativePower {
            feature_name: row.get(0),
            mean_loop_strong: row.get(1),
            mean_loop_weak: row.get(2),
            discriminative_power: row.get::<_, f64>(3),
            sample_count: row.get(4),
        })
        .collect())
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
    };

    // 1. Infrastructure Checks
    if let Ok(row) = conn.query_one("SELECT version()", &[]) {
        report.pg_version = row.get(0);
    }

    if let Ok(Some(row)) = conn.query_opt(
        "SELECT installed_version FROM pg_available_extensions WHERE name = 'vector'",
        &[],
    ) {
        report.has_vector_extension = true;
        report.vector_extension_version = row.get(0);
    }

    // 2. Table Statistics
    let tables = vec![
        "samples",
        "quality_samples",
        "analysis_records",
        "path_index",
        "inference_log",
        "quality_inference_log",
    ];
    for table in tables {
        let count_query = format!("SELECT COUNT(*) FROM {table}");
        if let Ok(row) = conn.query_one(&count_query, &[]) {
            report.table_counts.insert(table.to_string(), row.get(0));
        }
    }

    // 3. Data Integrity: NaN/Infinity Scan for pgvector columns
    // We scan both the feature search vector columns which are critical for KNN stability.

    // Check 'samples' table
    if let Ok(rows) = conn.query(
        "SELECT file_hash FROM samples WHERE features::text ~ 'NaN|Infinity'",
        &[],
    ) && !rows.is_empty()
    {
        report.corruption_found = true;
        report.corruption_details.push(format!(
            "🔥 Found {} records with NaN/Inf vectors in 'samples' table.",
            rows.len()
        ));
    }

    // Check 'quality_samples' table
    if let Ok(rows) = conn.query(
        "SELECT file_hash FROM quality_samples WHERE features::text ~ 'NaN|Infinity'",
        &[],
    ) && !rows.is_empty()
    {
        report.corruption_found = true;
        report.corruption_details.push(format!(
            "🔥 Found {} records with NaN/Inf vectors in 'quality_samples' table.",
            rows.len()
        ));
    }

    // 4. Maturity Analysis
    let (low, high, video) = get_class_counts(&mut conn);
    let total_samples = low + high + video;
    let min_total = crate::constants::MIN_GIF_SAMPLES_TOTAL;
    let min_per_class = crate::constants::MIN_GIF_SAMPLES_PER_CLASS;

    if total_samples >= min_total && low >= min_per_class && (high + video) >= min_per_class {
        report.maturity_status = "Mature (KNN Active)".to_string();
    } else {
        let needed = min_total.saturating_sub(total_samples);
        report.maturity_status = format!(
            "Immature (Need {} more samples)",
            crate::numeric_cast::i64_to_usize_sat(needed)
        );
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_meta() -> LoopMeta {
        let frames = 24;
        let duration = 2.0_f64;
        let size = 120_000;
        LoopMeta {
            duration_secs: Some(duration),
            duration_tier: Some(crate::loop_intent::DurationTier::from_secs(duration)),
            width: 320,
            height: 320,
            fps: Some(12.0),
            frame_count: Some(frames),
            file_size_bytes: size,
            file_name: None,
            palette_size: Some(64),
            app_extensions: None,
            encoder_software: None,
            is_interlaced: None,
            has_transparency: true,
            transparency_is_real: None,
            is_native_gif: true,
            real_frame_count: None,
            frame_payload_variation: Some(0.4),
            frame_delay_variation: Some(0.6),
            source_extension: Some("gif".to_string()),
            container: Some("gif".to_string()),
            parent_directories: None,
            directory_loop_intent_score: 0.5,
            filename_loop_intent_score: 0.5,
            has_embedded_icc: false,
            has_complex_color_profile: false,
            loop_count: None,
            has_audio: false,
            audio_is_silent: Some(true), // GIFs never have audio
            frame_types: vec![
                'P';
                crate::numeric_cast::u64_to_usize_strict(
                    frames,
                    "gif_frame_count_types"
                )
                .unwrap_or_else(|| {
                    crate::progress_mode::emit_stderr(
                        "☢️ [ANOMALY] GIF frame count overflow! Truncating to 0 for types.",
                    );
                    0
                })
            ],
            pts_deltas: vec![
                duration / crate::numeric_cast::u64_to_f64(frames.max(1));
                crate::numeric_cast::u64_to_usize_strict(frames, "gif_frame_count_pts")
                    .unwrap_or_else(|| {
                        crate::progress_mode::emit_stderr(
                            "☢️ [ANOMALY] GIF frame count overflow! Truncating to 0 for pts.",
                        );
                        0
                    })
            ],
            mv_magnitudes: Vec::new(),
            cached_frame_png: None,
            is_meme_platform: false,
            palette_depth: Some(0.8),
            motion_gini: Some(0.7),
            block_skew: Some(0.6),
            temporal_flatness: Some(0.9),
            loop_closure_score: Some(0.85),
            motion_periodicity: Some(0.75),
            temporal_jitter: Some(0.90),
            pkt_sizes: Vec::new(),
            webp_compression_ratio: Some(0.85),
        }
    }

    #[test]
    fn distance_prefers_similar_samples() {
        let meta = base_meta();
        let near = SampleRow {
            _loss_tolerance: Some("high".to_string()),
            width: 300,
            height: 300,
            duration_secs: Some(2.2),
            frame_count: Some(24),
            file_size_bytes: 125_000,
            fps: Some(12.0),
            temporal_bpp: 0.05,
            spatial_bpp: 1.2,
            has_transparency: true,
            has_embedded_icc: false,
            has_complex_color_profile: false,
            palette_size: Some(64),
            frame_payload_variation: Some(0.35_f64),
            frame_delay_variation: Some(0.55_f64),
            aspect_ratio: Some(1.0_f64),
            _total_pixels: Some(90000),
            loop_frequency: Some(0.8_f64),
            is_meme_platform: true,
            is_human_semantic_name: true,
            cadence_score: Some(0.9_f64),
            directory_loop_intent_score: Some(1.0_f64),
            is_high_value_source: true,
            is_native_gif: true,
            palette_depth: Some(0.8_f64),
            motion_gini: Some(0.7_f64),
            block_skew: Some(0.6_f64),
            temporal_flatness: Some(0.9_f64),
            loop_closure_score: Some(0.88_f64),
            motion_periodicity: Some(0.78_f64),
            temporal_jitter: Some(0.92_f64),
            webp_compression_ratio: Some(0.9_f64),
            _labeled_by: Some("cli_ingest".to_string()),
        };
        let far = SampleRow {
            _loss_tolerance: Some("low".to_string()),
            width: 1920,
            height: 1080,
            duration_secs: Some(20.0),
            frame_count: Some(600),
            file_size_bytes: 20_000_000,
            fps: Some(30.0),
            temporal_bpp: 0.4,
            spatial_bpp: 35.0,
            has_transparency: false,
            has_embedded_icc: true,
            has_complex_color_profile: true,
            palette_size: Some(256),
            frame_payload_variation: Some(0.05_f64),
            frame_delay_variation: Some(0.02_f64),
            aspect_ratio: Some(1.78_f64),
            _total_pixels: Some(2_073_600),
            loop_frequency: Some(0.1_f64),
            is_meme_platform: false,
            is_human_semantic_name: false,
            cadence_score: Some(0.1_f64),
            directory_loop_intent_score: Some(0.5_f64),
            is_high_value_source: false,
            is_native_gif: false,
            palette_depth: Some(0.1_f64),
            motion_gini: Some(0.2_f64),
            block_skew: Some(0.1_f64),
            temporal_flatness: Some(0.1_f64),
            loop_closure_score: Some(0.15_f64),
            motion_periodicity: Some(0.20_f64),
            temporal_jitter: Some(0.18_f64),
            webp_compression_ratio: Some(0.1_f64),
            _labeled_by: Some("cli_ingest".to_string()),
        };
        let (tbpp, sbpp) = bpp_from_meta(&meta);

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
            (resolved_duration_secs(&meta).expect("Test should have valid duration") - 80.0).abs()
                < 0.01_f64
        );
    }

    #[test]
    fn feature_stats_capture_percentiles() {
        let stats = build_feature_stats(&[1.0_f64, 2.0_f64, 3.0_f64, 4.0_f64, 5.0_f64]);
        assert_eq!(stats.p10, Some(1.4_f64));
        assert_eq!(stats.p50, Some(3.0_f64));
        assert_eq!(stats.p90, Some(4.6_f64));
    }

    #[test]
    fn bpp_from_meta_divides_temporal_density_by_frame_count() {
        let mut meta = base_meta();
        meta.width = 1200;
        meta.height = 1200;
        meta.frame_count = Some(36);
        meta.file_size_bytes = 2_391_699;

        let (temporal_bpp, spatial_bpp) = bpp_from_meta(&meta);
        let pixel_count = f64::from(meta.width) * f64::from(meta.height);
        let expected_temporal = crate::numeric_cast::u64_to_f64(meta.file_size_bytes)
            / (pixel_count
                * crate::numeric_cast::u64_to_f64(meta.frame_count.expect("fixture has frames")));
        let legacy_buggy_temporal = crate::numeric_cast::u64_to_f64(meta.file_size_bytes)
            / pixel_count
            * crate::numeric_cast::u64_to_f64(meta.frame_count.expect("fixture has frames"));
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

    #[test]
    fn loop_reference_profile_prefers_dynamic_stats_when_present() {
        let mut feature_map = FeatureMap {
            top_keywords: vec!["meme".to_string()],
            ..FeatureMap::default()
        };
        feature_map.stats.insert(
            "duration".to_string(),
            FeatureStats {
                mean: 6.0,
                std_dev: 2.0,
                weight: None,
                p10: Some(1.0_f64),
                p25: Some(2.0_f64),
                p50: Some(5.0_f64),
                p75: Some(8.0_f64),
                p90: Some(10.0_f64),
            },
        );
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

        let profile = build_loop_reference_profile(GlobalCollectionStats::default(), &feature_map);
        assert_eq!(profile.duration.p25, Some(2.0_f64));
        assert!(crate::float_compare::approx_eq_f64(profile.fps.mean, 14.0));
        assert_eq!(profile.top_keywords, vec!["meme".to_string()]);
    }
}
