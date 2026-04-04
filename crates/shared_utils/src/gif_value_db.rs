use crate::loop_intent::LoopMeta;
use crate::media_meta_utils::scan_gif_headers;
use crate::progress_mode::emit_stderr;
use anyhow::{Context, Result};
use blake3::Hasher;
use indicatif::{ProgressBar, ProgressStyle};
use postgres::{Client, NoTls};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;

const PG_DEFAULT_CONNSTR: &str = "host=localhost dbname=modern_format_boost";
const IMPORT_KEY: &str = "dataset_seeds_import_v4";
const STATS_KEY: &str = "feature_stats_v1";

static DB_WARN_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FeatureStats {
    mean: f64,
    std_dev: f64,
    #[serde(default)]
    weight: Option<f64>,
    #[serde(default)]
    p10: Option<f64>,
    #[serde(default)]
    p25: Option<f64>,
    #[serde(default)]
    p50: Option<f64>,
    #[serde(default)]
    p75: Option<f64>,
    #[serde(default)]
    p90: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FeatureMap {
    stats: std::collections::HashMap<String, FeatureStats>,
    top_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DistributionStats {
    pub mean: f64,
    pub std_dev: f64,
    #[serde(default)]
    pub weight: Option<f64>,
    #[serde(default)]
    pub p10: Option<f64>,
    #[serde(default)]
    pub p25: Option<f64>,
    #[serde(default)]
    pub p50: Option<f64>,
    #[serde(default)]
    pub p75: Option<f64>,
    #[serde(default)]
    pub p90: Option<f64>,
}

impl DistributionStats {
    #[must_use]
    pub fn z_score(&self, value: f64) -> f64 {
        if self.std_dev > 1e-6 {
            (value - self.mean) / self.std_dev
        } else {
            0.0
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelStatus {
    /// Manually verified / High confidence: Looping intent.
    LoopStrong,
    /// Manually verified / High confidence: Non-looping (video) intent.
    LoopWeak,
    /// Edge case / Low confidence label.
    Uncertain,
    /// Not yet labeled by a human or model.
    NotLabeled,
}

#[derive(Debug, Clone)]
pub struct SampleMatch {
    pub exact_label: LabelStatus,
    pub keep_probability: Option<f64>,
    /// Confidence score in [0, 1]: how tightly clustered the KNN neighbors are.
    /// confidence = 1.0 - (std_dev_distance / mean_distance), clamped to [0, 1].
    /// High confidence (>0.75) means neighbors are homogeneous; safe to trust keep_probability.
    pub confidence: f64,
    pub neighbor_count: usize,
    pub mean_distance: Option<f64>,
    pub std_dev_distance: Option<f64>,
    pub min_distance: Option<f64>,
    pub p25_distance: Option<f64>,
    pub p75_distance: Option<f64>,
    /// Dynamic baseline: P90 duration of neighbors with high loss tolerance.
    pub p90_duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopInferenceRecord {
    pub tree_probability: f64,
    pub knn_keep_probability: Option<f64>,
    pub knn_confidence: Option<f64>,
    pub knn_neighbor_count: Option<usize>,
    pub final_probability: f64,
    pub final_verdict: String,
    pub decision_reason: String,
    pub layer_exit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopFeatureDiscriminativePower {
    pub feature_name: String,
    pub mean_loop_strong: Option<f64>,
    pub mean_loop_weak: Option<f64>,
    pub discriminative_power: f64,
    pub sample_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceBlindSpot {
    pub duration_bucket: f64,
    pub webp_bucket: f64,
    pub avg_knn_confidence: f64,
    pub avg_tree_probability: Option<f64>,
    pub avg_final_probability: Option<f64>,
    pub avg_neighbor_count: Option<f64>,
    pub sample_count: i64,
    pub example_layer_exit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalCollectionStats {
    pub duration_min: f64,
    pub duration_avg: f64,
    pub duration_max: f64,
    pub duration_p90: f64,

    pub size_min: f64,
    pub size_avg: f64,
    pub size_max: f64,

    pub bitrate_min: f64,
    pub bitrate_avg: f64,
    pub bitrate_max: f64,

    pub width_min: u32,
    pub width_avg: f64,
    pub width_max: u32,

    pub height_min: u32,
    pub height_avg: f64,
    pub height_max: u32,

    pub aspect_min: f64,
    pub aspect_avg: f64,
    pub aspect_max: f64,
    pub top_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopReferenceProfile {
    pub collection: GlobalCollectionStats,
    pub duration: DistributionStats,
    pub fps: DistributionStats,
    pub frame_density: DistributionStats,
    pub file_size_bytes: DistributionStats,
    pub pixels: DistributionStats,
    pub temporal_bpp: DistributionStats,
    pub spatial_bpp: DistributionStats,
    pub payload_variation: DistributionStats,
    pub delay_variation: DistributionStats,
    pub palette_depth: DistributionStats,
    pub motion_gini: DistributionStats,
    pub temporal_flatness: DistributionStats,
    pub webp_ratio: DistributionStats,
    pub cadence: DistributionStats,
    pub top_keywords: Vec<String>,
}

impl Default for GlobalCollectionStats {
    fn default() -> Self {
        use crate::constants::{
            DEFAULT_LOOP_BASELINE_DURATION_SECS, MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS,
        };
        Self {
            duration_min: 0.1,
            duration_avg: DEFAULT_LOOP_BASELINE_DURATION_SECS,
            duration_max: 30.0,
            duration_p90: MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS,

            size_min: 1000.0,
            size_avg: 1_000_000.0,
            size_max: 5_000_000.0,

            bitrate_min: 10000.0,
            bitrate_avg: 500000.0,
            bitrate_max: 2000000.0,

            width_min: 32,
            width_avg: 512.0,
            width_max: 1280,

            height_min: 32,
            height_avg: 512.0,
            height_max: 1280,

            aspect_min: 0.5,
            aspect_avg: 1.0,
            aspect_max: 2.0,
            top_keywords: Vec::new(),
        }
    }
}

impl Default for LoopReferenceProfile {
    fn default() -> Self {
        let collection = GlobalCollectionStats::default();
        let pixels_min = f64::from(collection.width_min) * f64::from(collection.height_min);
        let pixels_avg = collection.width_avg * collection.height_avg;
        let pixels_max = f64::from(collection.width_max) * f64::from(collection.height_max);

        Self {
            duration: DistributionStats {
                mean: collection.duration_avg,
                std_dev: ((collection.duration_max - collection.duration_min) / 4.0).max(0.5),
                p10: Some(collection.duration_min),
                p25: Some(f64::midpoint(
                    collection.duration_min,
                    collection.duration_avg,
                )),
                p50: Some(collection.duration_avg),
                p75: Some(f64::midpoint(
                    collection.duration_avg,
                    collection.duration_p90,
                )),
                p90: Some(collection.duration_p90),
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

#[derive(Debug, Clone)]
struct SampleRow {
    #[allow(dead_code)]
    loss_tolerance: Option<String>,
    width: u32,
    height: u32,
    duration_secs: f64,
    frame_count: u64,
    file_size_bytes: u64,
    fps: f64,
    temporal_bpp: f64,
    spatial_bpp: f64,
    has_transparency: bool,
    has_embedded_icc: bool,
    has_complex_color_profile: bool,
    palette_size: Option<u32>,
    frame_payload_variation: Option<f64>,
    frame_delay_variation: Option<f64>,
    aspect_ratio: Option<f64>,
    #[allow(dead_code)]
    total_pixels: Option<u64>,
    loop_frequency: Option<f64>,
    is_meme_platform: bool,
    is_human_semantic_name: bool,
    cadence_score: Option<f64>,
    directory_meme_score: Option<f64>,
    is_high_value_source: bool,
    is_native_gif: bool,
    palette_depth: Option<f64>,
    motion_gini: Option<f64>,
    block_skew: Option<f64>,
    temporal_flatness: Option<f64>,
    webp_compression_ratio: Option<f64>,
    #[allow(dead_code)]
    labeled_by: Option<String>,
}

fn pg_connstr() -> String {
    std::env::var("MFB_PG_CONNSTR").unwrap_or_else(|_| PG_DEFAULT_CONNSTR.to_string())
}

pub fn open_pg_client() -> Result<Client> {
    let connstr = pg_connstr();
    match Client::connect(&connstr, NoTls) {
        Ok(client) => Ok(client),
        Err(e) => {
            if !DB_WARN_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let msg = format!("⚠️  Database Unavailable: {e}");
                crate::progress_mode::emit_stderr(&msg);
                crate::progress_mode::emit_stderr("💡 System running in [LEGACY LIMITED MODE] (Heuristic Tree only, no KNN/Learning).");
                crate::progress_mode::emit_stderr(
                    "💡 To enable full intelligence, run: 'sh scripts/manage_db.sh setup'",
                );
            }
            Err(e).with_context(|| format!("Failed to connect to PostgreSQL: {connstr}"))
        }
    }
}

/// One-time status report for the database.
pub fn report_db_status() {
    if let Ok(_conn) = open_pg_client() {
        crate::progress_mode::emit_stderr(
            "🐘 Database [PostgreSQL]: CONNECTED (Full Learning Mode Active)",
        );
    }
}

pub fn lookup_similar_samples(meta: &LoopMeta, path: Option<&Path>) -> Option<SampleMatch> {
    lookup_similar_samples_inner(meta, path).ok().flatten()
}

pub fn fetch_global_collection_stats(conn: &mut Client) -> Result<GlobalCollectionStats> {
    let row = conn.query_opt(
        "SELECT value FROM sample_metadata WHERE key = 'collection_stats_v1'",
        &[],
    )?;

    if let Some(row) = row {
        let json: String = row.get(0);
        Ok(serde_json::from_str(&json).unwrap_or_default())
    } else {
        Ok(GlobalCollectionStats::default())
    }
}

pub fn fetch_loop_reference_profile(conn: &mut Client) -> Result<LoopReferenceProfile> {
    let collection = fetch_global_collection_stats(conn).unwrap_or_default();
    let feature_map = fetch_feature_map(conn)?;
    Ok(build_loop_reference_profile(collection, &feature_map))
}

fn fetch_feature_map(conn: &mut Client) -> Result<FeatureMap> {
    Ok(conn
        .query_opt(
            "SELECT value FROM sample_metadata WHERE key = $1",
            &[&STATS_KEY],
        )?
        .map(|row| {
            let value: String = row.get(0);
            serde_json::from_str(&value).unwrap_or_default()
        })
        .unwrap_or_default())
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

/// Validates whether the GIF database has enough diverse samples to merit KNN lookup.
fn check_gif_db_maturity(conn: &mut Client) -> bool {
    let Ok(rows) = conn.query(
        "SELECT loss_tolerance, count(*) FROM samples WHERE loss_tolerance IN ('high', 'video') GROUP BY loss_tolerance",
        &[],
    ) else {
        return false;
    };

    let mut high_count: i64 = 0;
    let mut video_count: i64 = 0;
    for row in rows {
        let class: String = row.get(0);
        let count: i64 = row.get(1);
        if class == "high" {
            high_count = count;
        } else if class == "video" {
            video_count = count;
        }
    }

    let total = high_count + video_count;
    total >= crate::constants::MIN_GIF_SAMPLES_TOTAL
        && high_count >= crate::constants::MIN_GIF_SAMPLES_PER_CLASS
        && video_count >= crate::constants::MIN_GIF_SAMPLES_PER_CLASS
}

fn lookup_similar_samples_inner(
    meta: &LoopMeta,
    _path: Option<&Path>,
) -> Result<Option<SampleMatch>> {
    let mut conn = match open_pg_client() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("⚠️ PostgreSQL connection failed (graceful fallback): {e}");
            log::warn!("💡 Suggestion: Run 'sh scripts/manage_db.sh setup' to initialize and start the local database service.");
            return Ok(None);
        }
    };

    init_schema(&mut conn)?;
    seed_positive_dataset_if_needed(&mut conn)?;

    if !check_gif_db_maturity(&mut conn) {
        log::info!(
            "🔬 GIF Database is immature (needs >={} total, >={} per class). Bypassing KNN.",
            crate::constants::MIN_GIF_SAMPLES_TOTAL,
            crate::constants::MIN_GIF_SAMPLES_PER_CLASS
        );
        return Ok(None);
    }

    // Map the incoming LoopMeta into a SampleRow to compute its HNSW search vector
    let target_temporal_bpp = meta.file_size_bytes as f64
        / (((f64::from(meta.width) * f64::from(meta.height)).max(1.0))
            * meta.frame_count.max(1) as f64);
    let target_spatial_bpp =
        meta.file_size_bytes as f64 / (f64::from(meta.width) * f64::from(meta.height)).max(1.0);

    let target_sample = sample_row_from_meta(meta, target_temporal_bpp, target_spatial_bpp);

    let feature_stats = fetch_feature_map(&mut conn)?;
    let target_vector = compute_sample_vector(&target_sample, &feature_stats);
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

    if rows.is_empty() {
        return Ok(None);
    }

    let candidates: Vec<(LabelStatus, f64, f64)> = rows
        .iter()
        .map(|row| {
            let label_str: Option<String> = row.get(0);
            let label = match label_str.as_deref() {
                Some("high") => LabelStatus::LoopStrong,
                Some("video") => LabelStatus::LoopWeak,
                Some("uncertain") => LabelStatus::Uncertain,
                _ => LabelStatus::NotLabeled,
            };
            (
                label,               // label status
                row.get::<_, f64>(1), // duration_secs
                row.get::<_, f64>(2), // dist
            )
        })
        .collect();

    let neighbor_count = adaptive_neighbor_count(candidates.len());
    let neighbors = &candidates[..neighbor_count];

    let min_distance = neighbors.first().map_or(0.0, |(_, _, d)| *d);
    let radius = dynamic_neighbor_radius(neighbors);

    let mut weighted_keep = 0.0;
    let mut total_weight = 0.0;
    let mut distances = Vec::new();
    let mut loop_durations = Vec::new();

    for (label, duration_secs, distance) in neighbors {
        if *distance > radius {
            continue;
        }
 
        let relative_distance = (*distance - min_distance).max(0.0);
        let weight = 1.0 / (1.0 + relative_distance * relative_distance * 3.0);
        let prob = match label {
            LabelStatus::LoopStrong => 1.0,  // Loop intent (Meme/Sticker/Video sticker)
            LabelStatus::LoopWeak => 0.0,    // Non-loop intent (Clip/Record/Long Video)
            _ => 0.5,                        // Uncertain/Fallback
        };

        if prob >= 0.5 {
            loop_durations.push(*duration_secs);
        }

        weighted_keep += prob * weight;
        total_weight += weight;
        distances.push(*distance);
    }

    if distances.is_empty() {
        return Ok(None);
    }

    let keep_probability = weighted_keep / total_weight.max(1e-6);
    let mean_distance = distances.iter().sum::<f64>() / distances.len() as f64;

    let variance = distances
        .iter()
        .map(|d| {
            let diff = d - mean_distance;
            diff * diff
        })
        .sum::<f64>()
        / distances.len() as f64;
    let std_dev_distance = variance.sqrt();

    // Confidence: how tightly clustered the neighbors are.
    // High std_dev relative to mean → low confidence (mixed signals).
    let confidence = if mean_distance > 1e-6 {
        (1.0 - (std_dev_distance / mean_distance)).clamp(0.0, 1.0)
    } else {
        // All neighbors at distance ≈0 → exact match level confidence
        1.0
    };

    // Sort for percentiles
    distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = distances.len();
    let min_distance = distances.first().copied();
    let p25_distance = distances.get(n / 4).copied();
    let p75_distance = distances.get(3 * n / 4).copied();

    let p90_duration = if !loop_durations.is_empty() {
        loop_durations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = (loop_durations.len() as f64 * 0.90).floor() as usize;
        Some(loop_durations[idx.min(loop_durations.len() - 1)])
    } else {
        None
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
        (limit_high + (t * (limit_meme - limit_high))) as f32
    }
}

#[must_use]
fn resolved_duration_secs(meta: &LoopMeta) -> f64 {
    if meta.duration_secs > 0.11 {
        meta.duration_secs
    } else if meta.frame_count > 1 && meta.fps > 0.1 {
        meta.frame_count as f64 / meta.fps
    } else {
        meta.frame_count.max(1) as f64 / 12.0
    }
}

#[must_use]
pub fn is_lossless_exploration_safe(meta: &LoopMeta, path: Option<&Path>) -> bool {
    let mut current_meta = meta.clone();
    if let Some(p) = path {
        let _ = crate::loop_intent::deep_refine_meta(&mut current_meta, p);
    }
    current_meta.duration_secs = resolved_duration_secs(&current_meta);

    let sample_match = lookup_similar_samples(&current_meta, path);
    let keep_prob = sample_match
        .as_ref()
        .and_then(|m| m.keep_probability)
        .unwrap_or(0.5);

    // Dynamic threshold:
    // keep_prob close to 1.0 (Meme / High Tolerance) -> 120s limit
    // keep_prob close to 0.0 (Art / High Value)  -> 30s limit
    let threshold = lossless_duration_limit_for_keep_prob(keep_prob);

    let is_safe = current_meta.duration_secs < f64::from(threshold);

    if !is_safe {
        crate::log_eprintln!(
            "   ⚠️  Lossless-first (CRF 0.00) skip: duration {:.1}s exceeds dynamic limit {:.1}s (Value Prob: {:.2})",
            current_meta.duration_secs, threshold, keep_prob
        );
    }

    is_safe
}

pub fn init_schema(conn: &mut Client) -> Result<()> {
    emit_stderr("🐘 Initializing Database Schema (PostgreSQL + pgvector)...");
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
            duration_secs DOUBLE PRECISION NOT NULL,
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
            directory_meme_score DOUBLE PRECISION DEFAULT 0.5,
            is_high_value_source BOOLEAN DEFAULT FALSE,
            is_native_gif BOOLEAN DEFAULT FALSE,
            palette_depth DOUBLE PRECISION,
            motion_gini DOUBLE PRECISION,
            block_skew DOUBLE PRECISION,
            temporal_flatness DOUBLE PRECISION,
            loss_tolerance TEXT,
            loop_verdict TEXT,
            labeled_by TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            features vector(28)
        )",
        &[],
    )?;

    // Ensure older databases upgrade their schema
    let _ = conn.execute(
        "ALTER TABLE samples DROP COLUMN IF EXISTS features CASCADE",
        &[],
    );
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN features vector(28)", &[]);

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
            duration_secs DOUBLE PRECISION NOT NULL,
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

    let _ = conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS directory_meme_score DOUBLE PRECISION DEFAULT 0.5",
        &[],
    );

    let _ = conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS webp_compression_ratio DOUBLE PRECISION",
        &[],
    );

    let _ = conn.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS loop_verdict TEXT",
        &[],
    );

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
    tx.batch_execute(default_sql).unwrap_or_else(|e| {
        emit_stderr(&format!(
            "⚠️  Failed to seed default GIF value dataset: {e}"
        ));
    });

    tx.execute(
        "INSERT INTO sample_metadata (key, value) VALUES ($1, 'done')
         ON CONFLICT (key) DO UPDATE SET value = 'done'",
        &[&IMPORT_KEY],
    )?;
    tx.commit()?;

    emit_stderr("✅ Training Dataset successfully imported.");

    // Recalculate stats based on the newly seeded data
    let _ = refresh_feature_stats(conn);
    Ok(())
}

pub struct SampleInsert {
    file_hash: String,
    source_path: String,
    file_name: Option<String>,
    source_ext: Option<String>,
    width: u32,
    height: u32,
    duration_secs: f64,
    frame_count: u64,
    file_size_bytes: u64,
    fps: f64,
    has_embedded_icc: bool,
    has_complex_color_profile: bool,
    has_transparency: bool,
    palette_size: Option<u32>,
    frame_payload_variation: Option<f64>,
    frame_delay_variation: Option<f64>,
    temporal_bpp: f64,
    spatial_bpp: f64,
    loss_tolerance: String,
    labeled_by: String,
    aspect_ratio: Option<f64>,
    total_pixels: u64,
    loop_frequency: f64,
    is_meme_platform: bool,
    is_human_semantic_name: bool,
    cadence_score: f64,
    directory_meme_score: f64,
    is_high_value_source: bool,
    is_native_gif: bool,
    palette_depth: Option<f64>,
    motion_gini: Option<f64>,
    block_skew: Option<f64>,
    temporal_flatness: Option<f64>,
    webp_compression_ratio: Option<f64>,
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
    if temporal_bpp < 0.03 {
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

pub fn sample_from_path(
    path: &Path,
    labeled_by: &str,
    label_override: Option<&str>,
) -> Option<SampleInsert> {
    let probe = crate::probe_video(path).ok()?;
    let mut meta = LoopMeta::from_ffprobe_result(&probe, path);
    if let Ok((pal, exts, has_transparency, variation, delay_variation, loop_count, total_dur)) =
        scan_gif_headers(path)
    {
        meta.palette_size = pal;
        meta.app_extensions = exts;
        meta.has_transparency = has_transparency;
        meta.frame_payload_variation = variation;
        meta.frame_delay_variation = delay_variation;
        meta.loop_count = loop_count;
        if let Some(d) = total_dur {
            meta.duration_secs = d;
        }
    }

    // Call deep refinement to populate palette_depth, temporal_flatness, etc.
    let _ = crate::loop_intent::deep_refine_meta(&mut meta, path);

    let pixel_count = f64::from(meta.width) * f64::from(meta.height);
    let temporal_bpp =
        meta.file_size_bytes as f64 / (pixel_count.max(1.0) * meta.frame_count.max(1) as f64);
    let spatial_bpp = meta.file_size_bytes as f64 / pixel_count.max(1.0);

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
    let (is_human_semantic_name, directory_meme_score, cadence_score) = {
        let analysis = crate::loop_intent::analyze_filename(meta.file_name.as_deref(), &[]);
        (
            analysis.kind == crate::loop_intent::FilenameKind::HumanSemantic,
            crate::loop_intent::score_directory_context(meta.parent_directories.as_deref(), &[]),
            crate::loop_intent::score_sparse_cadence(meta.duration_secs, meta.frame_count),
        )
    };
    let is_meme_platform = meta.is_meme_platform;
    let is_native_gif = meta.source_extension.as_deref() == Some("gif");
    let is_high_value_source = loss_tolerance == "low";

    Some(SampleInsert {
        file_hash: calculate_blake3_hex(path).ok()?,
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
        directory_meme_score,
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

fn calculate_blake3_hex(path: &Path) -> Result<String> {
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

/// Compute a 28-dimensional pgvector encoding for a sample using pre-calculated std deviations.
/// This precisely bakes the weights and normalization terms from the old dynamically computed KNN
/// into an L2-compatible vector, allowing PostgreSQL's HNSW index to do the heavy lifting!
fn compute_sample_vector(sample: &SampleRow, stats_map: &FeatureMap) -> Vec<f32> {
    let sample_pixels = (f64::from(sample.width) * f64::from(sample.height)).max(1.0);

    let sample_frame_density = sample.frame_count as f64 / sample.duration_secs.max(0.05);
    let sample_frame_gap = sample.duration_secs / sample.frame_count.max(1) as f64;

    let sample_audio_score = if sample.is_native_gif { 1.0 } else { 0.55 };
    // We normalize fps against a baseline 30fps for the database encoded vector. Target queries will normalize identically.
    let baseline_fps = 30.0;
    let sample_fps_score = (1.0
        - crate::gif_value_db::normalize_log_ratio(sample.fps.max(1e-3), baseline_fps, 1.2))
    .clamp(0.0, 1.0);
    let sample_loop_affinity = (sample.loop_frequency.unwrap_or(0.5) * 0.45
        + sample.cadence_score.unwrap_or(0.5) * 0.25
        + sample_audio_score * 0.20
        + sample_fps_score * 0.10)
        .clamp(0.0, 1.0);

    let get_std = |f: &str| stats_map.stats.get(f).map_or(1.0, |s| s.std_dev).max(1e-6);
    let get_w = |f: &str| {
        stats_map
            .stats
            .get(f)
            .and_then(|s| s.weight)
            .unwrap_or(1.0)
            .max(0.01)
    };

    // Continuous standardizations (Scaled by sqrt(weight) since target_query <-> sample squares it)
    let v_pix = sample_pixels / get_std("pixels") * get_w("pixels").sqrt();
    let v_dur = sample.duration_secs / get_std("duration") * get_w("duration").sqrt();
    let v_frm = sample.frame_count as f64 / get_std("frame_count") * get_w("frame_count").sqrt();
    let v_fsize = sample.file_size_bytes as f64 / get_std("file_size_bytes")
        * get_w("file_size_bytes").sqrt();
    let v_dens = sample_frame_density / get_std("density") * get_w("density").sqrt();
    let v_gap = sample_frame_gap / get_std("gap") * get_w("gap").sqrt();
    let v_tbpp = sample.temporal_bpp / get_std("temporal_bpp") * get_w("temporal_bpp").sqrt();
    let v_sbpp = sample.spatial_bpp / get_std("spatial_bpp") * get_w("spatial_bpp").sqrt();

    let sample_webp_ratio = sample.webp_compression_ratio.unwrap_or(0.0);
    let v_wratio = sample_webp_ratio / get_std("webp_ratio") * get_w("webp_ratio").sqrt();

    let v_lfreq =
        sample.loop_frequency.unwrap_or(0.5) / get_std("loop_freq") * get_w("loop_freq").sqrt();
    let v_laffin = sample_loop_affinity / get_std("loop_affin") * get_w("loop_affin").sqrt();
    let v_cadence =
        sample.cadence_score.unwrap_or(0.5) / get_std("cadence") * get_w("cadence").sqrt();

    let v_payload = sample.frame_payload_variation.unwrap_or(0.5) / get_std("payload_var")
        * get_w("payload_var").sqrt();
    let v_delay = sample.frame_delay_variation.unwrap_or(0.5) / get_std("delay_var")
        * get_w("delay_var").sqrt();

    let v_aspect = sample.aspect_ratio.unwrap_or(1.0) / get_std("aspect") * get_w("aspect").sqrt();
    let v_pal = (sample.palette_size.map_or(256.0, f64::from) / 256.0) * get_w("p_depth").sqrt();

    let v_pdepth =
        sample.palette_depth.unwrap_or(0.5) / get_std("p_depth") * get_w("p_depth").sqrt();
    let v_mgini = sample.motion_gini.unwrap_or(0.5) / get_std("m_gini") * get_w("m_gini").sqrt();
    let v_bskew = sample.block_skew.unwrap_or(0.5) / get_std("b_skew") * get_w("b_skew").sqrt();
    let v_tflat =
        sample.temporal_flatness.unwrap_or(0.5) / get_std("t_flat") * get_w("t_flat").sqrt();

    // Directory context
    let v_dir = sample.directory_meme_score.unwrap_or(0.5) * get_w("dir_meme").sqrt();

    // Categorical variables (weight mapped so diff^2 = penalty weight)
    // If w = penalty weight, v = sqrt(w)/2. If diff is `sqrt(w)`, squared diff is `w`.
    // Wait: If true is w/2 and false is -w/2, diff is w. Squared diff is w^2!
    // To get a penalty of W added to the SUM OF SQUARES, we need diff^2 = W. Thus diff = sqrt(W).
    // So true mapped to sqrt(W)/2, false to -sqrt(W)/2.
    let cat = |val: bool, w: f64| if val { w.sqrt() / 2.0 } else { -w.sqrt() / 2.0 };

    let v_meme = cat(sample.is_meme_platform, 1.2);
    let v_name = cat(sample.is_human_semantic_name, 0.8);
    let v_native = cat(sample.is_native_gif, 0.6);
    let v_hv = cat(sample.is_high_value_source, 1.5);
    let v_trans = cat(sample.has_transparency, 1.5);
    let v_icc = cat(sample.has_embedded_icc, 1.2 / 2.0);
    let v_complex = cat(sample.has_complex_color_profile, 1.2 / 2.0);

    vec![
        v_pix as f32,
        v_dur as f32,
        v_frm as f32,
        v_fsize as f32,
        v_dens as f32,
        v_gap as f32,
        v_tbpp as f32,
        v_sbpp as f32,
        v_wratio as f32,
        v_lfreq as f32,
        v_laffin as f32,
        v_cadence as f32,
        v_payload as f32,
        v_delay as f32,
        v_aspect as f32,
        v_pal as f32,
        v_pdepth as f32,
        v_mgini as f32,
        v_bskew as f32,
        v_tflat as f32,
        v_dir as f32,
        v_meme as f32,
        v_name as f32,
        v_native as f32,
        v_hv as f32,
        v_trans as f32,
        v_icc as f32,
        v_complex as f32,
    ]
}

fn sample_row_from_meta(meta: &LoopMeta, temporal_bpp: f64, spatial_bpp: f64) -> SampleRow {
    SampleRow {
        loss_tolerance: None,
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
        total_pixels: Some(u64::from(meta.width) * u64::from(meta.height)),
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
        directory_meme_score: Some(crate::loop_intent::score_directory_context(
            meta.parent_directories.as_deref(),
            &[],
        )),
        is_high_value_source: meta.has_embedded_icc
            || meta.has_complex_color_profile
            || meta.has_audio,
        is_native_gif: meta.source_extension.as_deref() == Some("gif"),
        palette_depth: meta.palette_depth,
        motion_gini: meta.motion_gini,
        block_skew: meta.block_skew,
        temporal_flatness: meta.temporal_flatness,
        webp_compression_ratio: meta.webp_compression_ratio,
        labeled_by: None,
    }
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
    let target_vector = compute_sample_vector(&target, stats_map);
    let sample_vector = compute_sample_vector(sample, stats_map);
    vector_l2_distance(&target_vector, &sample_vector)
}

fn adaptive_neighbor_count(total: usize) -> usize {
    ((total as f64).sqrt().round() as usize)
        .clamp(6, 24)
        .min(total)
}

fn dynamic_neighbor_radius(neighbors: &[(LabelStatus, f64, f64)]) -> f64 {
    let mut distances: Vec<f64> = neighbors.iter().map(|(_, _, d)| *d).collect();
    distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = distances[distances.len() / 4];
    let q3 = distances[(distances.len() * 3) / 4];
    let iqr = (q3 - q1).max(0.06);
    (distances[0] + iqr * 1.5).max(distances[0] + 0.08)
}

fn normalize_log_ratio(a: f64, b: f64, scale: f64) -> f64 {
    if a <= 0.0 || b <= 0.0 || scale <= 0.0 {
        return 1.0;
    }
    ((a.ln() - b.ln()).abs() / scale).clamp(0.0, 1.0)
}

#[allow(dead_code)]
fn relative_distance(a: f64, b: f64) -> f64 {
    (a - b).abs() / a.abs().max(b.abs()).max(1.0)
}

fn percentile_value(sorted_values: &[f64], quantile: f64) -> Option<f64> {
    if sorted_values.is_empty() {
        return None;
    }

    let clamped = quantile.clamp(0.0, 1.0);
    let scaled_index = clamped * (sorted_values.len().saturating_sub(1)) as f64;
    let lower_index = scaled_index.floor() as usize;
    let upper_index = scaled_index.ceil() as usize;

    if lower_index == upper_index {
        return sorted_values.get(lower_index).copied();
    }

    let lower = sorted_values.get(lower_index).copied()?;
    let upper = sorted_values.get(upper_index).copied()?;
    Some(lower + (upper - lower) * (scaled_index - lower_index as f64))
}

fn build_feature_stats(values: &[f64]) -> FeatureStats {
    if values.is_empty() {
        return FeatureStats::default();
    }

    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;

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
            .expect("Valid template")
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
                if s.frame_count <= 1 {
                    return None;
                }
                pb.set_message(format!("Learn: {}", s.file_name.as_deref().unwrap_or("?")));
            }
            res
        })
        .collect();

    pb.finish_with_message("Learning complete.");

    println!("💾 Persisting {} samples to database...", samples.len());
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
            cadence_score, directory_meme_score, is_high_value_source, is_native_gif,
            palette_depth, motion_gini, block_skew, temporal_flatness, webp_compression_ratio,
            loop_verdict
         ) VALUES (
            $1, $2, $3, $4,
            $5, $6, $7, $8, $9, $10,
            $11, $12, $13,
            $14, $15, $16,
            $17, $18, $19, $20, $21,
            $22, $23, $24, $25, $26, $27, $28, $29,
            $30, $31, $32, $33, $34, $35
         )
         ON CONFLICT (file_hash) DO UPDATE SET
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
            directory_meme_score = EXCLUDED.directory_meme_score,
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
        let palette_size_i32 = sample.palette_size.map(|v| v as i32);
        let total_pixels_i64 = sample.total_pixels as i64;
        let frame_count_i64 = sample.frame_count as i64;
        let file_size_i64 = sample.file_size_bytes as i64;
        let width_i32 = sample.width as i32;
        let height_i32 = sample.height as i32;

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
                &sample.directory_meme_score,
                &sample.is_high_value_source,
                &sample.is_native_gif,
                &sample.palette_depth,
                &sample.motion_gini,
                &sample.block_skew,
                &sample.temporal_flatness,
                &sample.webp_compression_ratio,
                &sample.loop_verdict,
            ],
        );
        if res.is_ok() {
            count += 1;
        }
    }

    tx.commit()?;
    refresh_feature_stats(&mut conn)?;
    Ok(count)
}

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
        let mut fixed_count = 0;
        for row in broken_rows {
            let file_hash: String = row.get(0);
            let source_path: Option<String> = row.get(1);
            if let Some(path_str) = source_path {
                let path = Path::new(&path_str);
                if path.exists() {
                    if let Some(sample) = sample_from_path(path, "integrity_refresh", None) {
                        let _ = conn.execute(
                            "UPDATE samples SET 
                                motion_gini = $1, 
                                directory_meme_score = $2,
                                temporal_flatness = $3,
                                palette_depth = $4
                             WHERE file_hash = $5",
                            &[
                                &sample.motion_gini,
                                &sample.directory_meme_score,
                                &sample.temporal_flatness,
                                &sample.palette_depth,
                                &file_hash,
                            ],
                        );
                        fixed_count += 1;
                    }
                }
            }
        }
        if fixed_count > 0 {
            emit_stderr(&format!(
                "   ✅ Refreshed feature integrity for {} labeled samples.",
                fixed_count
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
        .map(|row| {
            let duration = row.get::<_, f64>(2);
            let frame_count = row.get::<_, i64>(3) as f64;
            let fps = row.get::<_, Option<f64>>(5).unwrap_or(0.0);
            let density = if duration > 0.05 {
                frame_count / duration
            } else {
                fps
            };
            let gap = if frame_count > 0.0 {
                duration / frame_count
            } else {
                duration
            };

            vec![
                f64::from(row.get::<_, i32>(0)) * f64::from(row.get::<_, i32>(1)), // pixels
                duration,
                frame_count,
                row.get::<_, i64>(4) as f64, // file_size_bytes
                fps,
                density,
                gap,
                row.get::<_, f64>(6),                         // temporal_bpp
                row.get::<_, f64>(7),                         // spatial_bpp
                row.get::<_, Option<f64>>(10).unwrap_or(1.0), // aspect
                row.get::<_, Option<f64>>(11).unwrap_or(0.5), // loop_freq
                row.get::<_, Option<f64>>(12).unwrap_or(0.5), // cadence
                row.get::<_, Option<f64>>(8).unwrap_or(0.5),  // payload_var
                row.get::<_, Option<f64>>(9).unwrap_or(0.5),  // delay_var
                row.get::<_, Option<f64>>(13).unwrap_or(0.5), // p_depth
                row.get::<_, Option<f64>>(14).unwrap_or(0.5), // m_gini
                row.get::<_, Option<f64>>(15).unwrap_or(0.5), // b_skew
                row.get::<_, Option<f64>>(16).unwrap_or(0.5), // t_flat
                row.get::<_, Option<f64>>(17).unwrap_or(1.0), // webp_ratio
            ]
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

    let mut feature_map = FeatureMap::default();
    feature_map.top_keywords = top_keywords.clone();

    for (idx, name) in names.iter().enumerate() {
        let values: Vec<f64> = all_data
            .iter()
            .map(|v| v.get(idx).copied().unwrap_or(0.0))
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
        duration_min: dur_min,
        duration_avg: dur_avg,
        duration_max: dur_max,
        duration_p90: feature_map
            .stats
            .get("duration")
            .and_then(|stats| stats.p90)
            .unwrap_or(dur_avg),

        size_min: size_min_i64 as f64,
        size_avg,
        size_max: size_max_i64 as f64,

        bitrate_min: bitrate_row.get(0),
        bitrate_avg: bitrate_row.get(1),
        bitrate_max: bitrate_row.get(2),

        width_min: w_min_i32 as u32,
        width_avg: w_avg,
        width_max: w_max_i32 as u32,

        height_min: h_min_i32 as u32,
        height_avg: h_avg,
        height_max: h_max_i32 as u32,

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

    // ── pgvector HNSW Migration: Backfill Vectors ──
    emit_stderr("   ⚙️  Backfilling pgvector encodings for all labeled samples...");

    let sample_rows = conn.query(
        "SELECT
            file_hash, loss_tolerance, width, height, duration_secs, frame_count, file_size_bytes,
            fps, temporal_bpp, spatial_bpp,
            has_transparency, has_embedded_icc, has_complex_color_profile,
            palette_size, frame_payload_variation, frame_delay_variation,
            aspect_ratio, labeled_by,
            total_pixels, loop_frequency, is_meme_platform, is_human_semantic_name,
            cadence_score, directory_meme_score, is_high_value_source, is_native_gif,
            palette_depth, motion_gini, block_skew, temporal_flatness, webp_compression_ratio
         FROM samples WHERE loss_tolerance IS NOT NULL AND frame_count > 1",
        &[],
    )?;

    let mut tx = conn.transaction()?;
    let stmt = tx.prepare("UPDATE samples SET features = $1::vector WHERE file_hash = $2")?;

    let mut updated_count = 0;
    for row in &sample_rows {
        let file_hash: String = row.get(0);
        let sample = SampleRow {
            loss_tolerance: row.get(1),
            width: row.get::<_, i32>(2) as u32,
            height: row.get::<_, i32>(3) as u32,
            duration_secs: row.get(4),
            frame_count: row.get::<_, i64>(5) as u64,
            file_size_bytes: row.get::<_, i64>(6) as u64,
            fps: row.get::<_, Option<f64>>(7).unwrap_or(0.0),
            temporal_bpp: row.get(8),
            spatial_bpp: row.get(9),
            has_transparency: row.get(10),
            has_embedded_icc: row.get(11),
            has_complex_color_profile: row.get(12),
            palette_size: row.get::<_, Option<i32>>(13).map(|v| v as u32),
            frame_payload_variation: row.get(14),
            frame_delay_variation: row.get(15),
            aspect_ratio: row.get(16),
            labeled_by: row.get(17),
            total_pixels: row.get::<_, Option<i64>>(18).map(|v| v as u64),
            loop_frequency: row.get(19),
            is_meme_platform: row.get(20),
            is_human_semantic_name: row.get(21),
            cadence_score: row.get(22),
            directory_meme_score: row.get::<_, Option<f64>>(23),
            is_high_value_source: row.get(24),
            is_native_gif: row.get(25),
            palette_depth: row.get(26),
            motion_gini: row.get(27),
            block_skew: row.get(28),
            temporal_flatness: row.get(29),
            webp_compression_ratio: row.get::<_, Option<f64>>(30),
        };

        let vec_data = compute_sample_vector(&sample, &feature_map);
        let pg_vector = pgvector::Vector::from(vec_data);
        tx.execute(&stmt, &[&pg_vector, &file_hash])?;
        updated_count += 1;
    }
    tx.commit()?;
    emit_stderr(&format!(
        "   ✅ pgvector backfill complete ({} samples encoded).",
        updated_count
    ));

    emit_stderr("✅ KNN Model Training Complete: Internal statistics synchronized.");
    Ok(())
}

// ── Level 4: Inference Logging ───────────────────────────────────────────────

/// Build a JSON snapshot of key LoopMeta fields for the inference log.
fn build_signal_snapshot(meta: &LoopMeta) -> Value {
    json!({
        "duration_secs": meta.duration_secs,
        "width": meta.width,
        "height": meta.height,
        "fps": meta.fps,
        "frame_count": meta.frame_count,
        "file_size_bytes": meta.file_size_bytes,
        "has_audio": meta.has_audio,
        "has_transparency": meta.has_transparency,
        "is_native_gif": meta.is_native_gif,
        "has_embedded_icc": meta.has_embedded_icc,
        "has_complex_color_profile": meta.has_complex_color_profile,
        "is_meme_platform": meta.is_meme_platform,
        "loop_count": meta.loop_count,
        "webp_compression_ratio": meta.webp_compression_ratio,
        "palette_depth": meta.palette_depth,
        "motion_gini": meta.motion_gini,
        "temporal_flatness": meta.temporal_flatness,
        "block_skew": meta.block_skew,
        "frame_payload_variation": meta.frame_payload_variation,
        "frame_delay_variation": meta.frame_delay_variation,
        "directory_meme_score": meta.directory_meme_score,
        "filename_meme_score": meta.filename_meme_score,
        "source_extension": meta.source_extension,
        "container": meta.container,
    })
}

/// Log one inference record to the database. Fails silently — never blocks the pipeline.
///
/// Called by `assess_loop_intent_from_meta` after every verdict to build the feedback loop.
pub fn log_inference_record(
    conn: &mut Client,
    meta: &LoopMeta,
    record: &LoopInferenceRecord,
    path: Option<&Path>,
) {
    let file_hash: Option<String> = path.and_then(|p| calculate_blake3_hex(p).ok());
    let source_path: Option<String> = path.map(|p| p.display().to_string());
    let snapshot = build_signal_snapshot(meta);
    let snapshot_str = serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string());

    let knn_neighbor_count_i32 = record.knn_neighbor_count.map(|n| n as i32);

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
            &meta.duration_secs,
            &meta.webp_compression_ratio,
            &record.tree_probability,
            &record.knn_keep_probability,
            &record.knn_confidence,
            &knn_neighbor_count_i32,
            &record.final_probability,
            &record.final_verdict,
            &record.decision_reason,
            &record.layer_exit,
            &snapshot_str,
        ],
    );

    if let Err(e) = result {
        log::warn!("⚠️ Failed to write inference log (non-fatal): {e}");
    }
}

// ── Level 1: Feature Discriminative Power Analysis ───────────────────────────

/// Query which features have real discriminative power between loop_strong and loop_weak.
///
/// Returns features sorted by absolute discriminative power descending.
/// `discriminative_power = (mean_loop_strong - mean_loop_weak) / stddev`
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
            UNION ALL SELECT loss_tolerance, 'directory_meme_score', directory_meme_score FROM samples WHERE loss_tolerance IN ('high', 'video') AND directory_meme_score IS NOT NULL
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
            discriminative_power: row.get::<_, Option<f64>>(3).unwrap_or(0.0),
            sample_count: row.get(4),
        })
        .collect())
}

// ── Level 3: Blind Spot Discovery ────────────────────────────────────────────

/// Discover feature-space regions where the system is most uncertain.
///
/// Buckets inference logs by duration (5s) and WebP ratio (3 units) and finds
/// regions with average KNN confidence below the threshold.
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
    pub total_records: i64,
    pub verdict_counts: Vec<(String, i64)>,
    pub layer_exit_counts: Vec<(String, i64)>,
    pub avg_tree_probability: Option<f64>,
    pub avg_knn_confidence: Option<f64>,
    pub avg_final_probability: Option<f64>,
    pub layer7_fallback_count: i64,
}

/// Get a summary of all inference log records.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base_meta() -> LoopMeta {
        let frames = 24;
        let duration = 2.0;
        let size = 120_000;
        LoopMeta {
            duration_secs: duration,
            width: 320,
            height: 320,
            fps: 12.0,
            frame_count: frames,
            file_size_bytes: size,
            file_name: None,
            palette_size: Some(64),
            app_extensions: None,
            has_transparency: true,
            is_native_gif: true,
            frame_payload_variation: Some(0.4),
            frame_delay_variation: Some(0.6),
            source_extension: Some("gif".to_string()),
            container: Some("gif".to_string()),
            parent_directories: None,
            directory_meme_score: 0.5,
            filename_meme_score: 0.5,
            has_embedded_icc: false,
            has_complex_color_profile: false,
            loop_count: None,
            has_audio: false,
            frame_types: vec!['P'; frames as usize],
            pts_deltas: vec![duration / frames as f64; frames as usize],
            mv_magnitudes: Vec::new(),
            cached_frame_png: None,
            is_meme_platform: false,
            palette_depth: Some(0.8),
            motion_gini: Some(0.7),
            block_skew: Some(0.6),
            temporal_flatness: Some(0.9),
            pkt_sizes: Vec::new(),
            webp_compression_ratio: None,
        }
    }

    #[test]
    fn distance_prefers_similar_samples() {
        let meta = base_meta();
        let near = SampleRow {
            loss_tolerance: Some("high".to_string()),
            width: 300,
            height: 300,
            duration_secs: 2.2,
            frame_count: 24,
            file_size_bytes: 125_000,
            fps: 12.0,
            temporal_bpp: 0.05,
            spatial_bpp: 1.2,
            has_transparency: true,
            has_embedded_icc: false,
            has_complex_color_profile: false,
            palette_size: Some(64),
            frame_payload_variation: Some(0.35),
            frame_delay_variation: Some(0.55),
            aspect_ratio: Some(1.0),
            total_pixels: Some(90000),
            loop_frequency: Some(0.8),
            is_meme_platform: true,
            is_human_semantic_name: true,
            cadence_score: Some(0.9),
            directory_meme_score: Some(1.0),
            is_high_value_source: true,
            is_native_gif: true,
            palette_depth: Some(0.8),
            motion_gini: Some(0.7),
            block_skew: Some(0.6),
            temporal_flatness: Some(0.9),
            webp_compression_ratio: Some(0.9),
            labeled_by: Some("cli_ingest".to_string()),
        };
        let far = SampleRow {
            loss_tolerance: Some("low".to_string()),
            width: 1920,
            height: 1080,
            duration_secs: 20.0,
            frame_count: 600,
            file_size_bytes: 20_000_000,
            fps: 30.0,
            temporal_bpp: 0.4,
            spatial_bpp: 35.0,
            has_transparency: false,
            has_embedded_icc: true,
            has_complex_color_profile: true,
            palette_size: Some(256),
            frame_payload_variation: Some(0.05),
            frame_delay_variation: Some(0.02),
            aspect_ratio: Some(1.78),
            total_pixels: Some(2073600),
            loop_frequency: Some(0.1),
            is_meme_platform: false,
            is_human_semantic_name: false,
            cadence_score: Some(0.1),
            directory_meme_score: Some(0.5),
            is_high_value_source: false,
            is_native_gif: false,
            palette_depth: Some(0.1),
            motion_gini: Some(0.2),
            block_skew: Some(0.1),
            temporal_flatness: Some(0.1),
            webp_compression_ratio: Some(0.1),
            labeled_by: Some("cli_ingest".to_string()),
        };
        let pixel_count = f64::from(meta.width) * f64::from(meta.height);
        let tbpp = meta.file_size_bytes as f64 / (pixel_count * meta.frame_count as f64);
        let sbpp = meta.file_size_bytes as f64 / pixel_count;

        let stats = FeatureMap::default();

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
        meta.duration_secs = 0.0;
        meta.frame_count = 800;
        meta.fps = 10.0;
        assert!((resolved_duration_secs(&meta) - 80.0).abs() < 0.01);
    }

    #[test]
    fn feature_stats_capture_percentiles() {
        let stats = build_feature_stats(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(stats.p10, Some(1.4));
        assert_eq!(stats.p50, Some(3.0));
        assert_eq!(stats.p90, Some(4.6));
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
                p10: Some(1.0),
                p25: Some(2.0),
                p50: Some(5.0),
                p75: Some(8.0),
                p90: Some(10.0),
            },
        );
        feature_map.stats.insert(
            "fps".to_string(),
            FeatureStats {
                mean: 14.0,
                std_dev: 3.0,
                p10: Some(8.0),
                p25: Some(10.0),
                p50: Some(14.0),
                p75: Some(18.0),
                p90: Some(22.0),
                weight: None,
            },
        );

        let profile = build_loop_reference_profile(GlobalCollectionStats::default(), &feature_map);
        assert_eq!(profile.duration.p25, Some(2.0));
        assert_eq!(profile.fps.mean, 14.0);
        assert_eq!(profile.top_keywords, vec!["meme".to_string()]);
    }
}
