use crate::loop_intent::LoopMeta;
use crate::media_meta_utils::scan_gif_headers;
use anyhow::{Context, Result};
use blake3::Hasher;
use indicatif::{ProgressBar, ProgressStyle};
use postgres::{Client, NoTls};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;

const PG_DEFAULT_CONNSTR: &str = "host=localhost dbname=modern_format_boost";
const IMPORT_KEY: &str = "dataset_seeds_import_v4";
const STATS_KEY: &str = "feature_stats_v1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FeatureStats {
    mean: f64,
    std_dev: f64,
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
            p10: value.p10,
            p25: value.p25,
            p50: value.p50,
            p75: value.p75,
            p90: value.p90,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SampleMatch {
    pub exact_label: Option<bool>,
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
        let midpoint = |lhs: f64, rhs: f64| (lhs + rhs) / 2.0;

        Self {
            duration: DistributionStats {
                mean: collection.duration_avg,
                std_dev: ((collection.duration_max - collection.duration_min) / 4.0).max(0.5),
                p10: Some(collection.duration_min),
                p25: Some(midpoint(collection.duration_min, collection.duration_avg)),
                p50: Some(collection.duration_avg),
                p75: Some(midpoint(collection.duration_avg, collection.duration_p90)),
                p90: Some(collection.duration_p90),
            },
            fps: DistributionStats {
                mean: 12.0,
                std_dev: 8.0,
                p10: Some(4.0),
                p25: Some(8.0),
                p50: Some(12.0),
                p75: Some(18.0),
                p90: Some(24.0),
            },
            frame_density: DistributionStats {
                mean: 12.0,
                std_dev: 8.0,
                p10: Some(4.0),
                p25: Some(8.0),
                p50: Some(12.0),
                p75: Some(18.0),
                p90: Some(24.0),
            },
            file_size_bytes: DistributionStats {
                mean: collection.size_avg,
                std_dev: ((collection.size_max - collection.size_min) / 4.0).max(64_000.0),
                p10: Some(collection.size_min),
                p25: Some(midpoint(collection.size_min, collection.size_avg)),
                p50: Some(collection.size_avg),
                p75: Some(midpoint(collection.size_avg, collection.size_max)),
                p90: Some(collection.size_max),
            },
            pixels: DistributionStats {
                mean: pixels_avg,
                std_dev: ((pixels_max - pixels_min) / 4.0).max(16_384.0),
                p10: Some(pixels_min),
                p25: Some(midpoint(pixels_min, pixels_avg)),
                p50: Some(pixels_avg),
                p75: Some(midpoint(pixels_avg, pixels_max)),
                p90: Some(pixels_max),
            },
            temporal_bpp: DistributionStats {
                mean: 0.05,
                std_dev: 0.05,
                p10: Some(0.01),
                p25: Some(0.02),
                p50: Some(0.05),
                p75: Some(0.08),
                p90: Some(0.12),
            },
            spatial_bpp: DistributionStats {
                mean: 4.0,
                std_dev: 3.0,
                p10: Some(1.0),
                p25: Some(2.0),
                p50: Some(4.0),
                p75: Some(6.0),
                p90: Some(10.0),
            },
            payload_variation: DistributionStats {
                mean: 0.5,
                std_dev: 0.2,
                p10: Some(0.2),
                p25: Some(0.35),
                p50: Some(0.5),
                p75: Some(0.65),
                p90: Some(0.8),
            },
            delay_variation: DistributionStats {
                mean: 0.25,
                std_dev: 0.15,
                p10: Some(0.05),
                p25: Some(0.12),
                p50: Some(0.25),
                p75: Some(0.35),
                p90: Some(0.55),
            },
            palette_depth: DistributionStats {
                mean: 0.55,
                std_dev: 0.18,
                p10: Some(0.25),
                p25: Some(0.4),
                p50: Some(0.55),
                p75: Some(0.7),
                p90: Some(0.85),
            },
            motion_gini: DistributionStats {
                mean: 0.55,
                std_dev: 0.18,
                p10: Some(0.2),
                p25: Some(0.4),
                p50: Some(0.55),
                p75: Some(0.7),
                p90: Some(0.85),
            },
            temporal_flatness: DistributionStats {
                mean: 0.55,
                std_dev: 0.18,
                p10: Some(0.2),
                p25: Some(0.4),
                p50: Some(0.55),
                p75: Some(0.7),
                p90: Some(0.85),
            },
            webp_ratio: DistributionStats {
                mean: 10.0,
                std_dev: 4.0,
                p10: Some(4.0),
                p25: Some(7.0),
                p50: Some(10.0),
                p75: Some(13.0),
                p90: Some(16.0),
            },
            cadence: DistributionStats {
                mean: 0.5,
                std_dev: 0.2,
                p10: Some(0.2),
                p25: Some(0.35),
                p50: Some(0.5),
                p75: Some(0.65),
                p90: Some(0.8),
            },
            top_keywords: collection.top_keywords.clone(),
            collection,
        }
    }
}

#[derive(Debug, Clone)]
struct SampleRow {
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
    labeled_by: Option<String>,
}

fn pg_connstr() -> String {
    std::env::var("MFB_PG_CONNSTR").unwrap_or_else(|_| PG_DEFAULT_CONNSTR.to_string())
}

pub fn open_pg_client() -> Result<Client> {
    let connstr = pg_connstr();
    Client::connect(&connstr, NoTls)
        .with_context(|| format!("Failed to connect to PostgreSQL: {connstr}"))
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
        .map(DistributionStats::from)
        .unwrap_or(fallback)
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

    // (Abandon Exact Match per User Request)
    // We no longer exit early on file_hash / blake3 matches.
    // Instead, we proceed to fuzzy matching to find semantically similar samples.

    let target_pixels = f64::from(meta.width) * f64::from(meta.height);
    let target_temporal_bpp =
        meta.file_size_bytes as f64 / ((target_pixels.max(1.0)) * meta.frame_count.max(1) as f64);
    let target_spatial_bpp = meta.file_size_bytes as f64 / target_pixels.max(1.0);

    let feature_stats = fetch_feature_map(&mut conn)?;

    // Corrected Query: Only native GIFs for KNN reference, no exact match bypass.
    let rows = conn.query(
        "SELECT
            loss_tolerance, width, height, duration_secs, frame_count, file_size_bytes,
            fps, temporal_bpp, spatial_bpp,
            has_transparency, has_embedded_icc, has_complex_color_profile,
            palette_size, frame_payload_variation, frame_delay_variation,
            aspect_ratio, labeled_by,
            total_pixels, loop_frequency, is_meme_platform, is_human_semantic_name,
            cadence_score, is_high_value_source, is_native_gif,
            palette_depth, motion_gini, block_skew, temporal_flatness, directory_meme_score,
            webp_compression_ratio
         FROM samples
         WHERE loss_tolerance IS NOT NULL AND is_native_gif = TRUE
         LIMIT 1021",
        &[],
    )?;

    let mut candidates = Vec::new();

    for row in &rows {
        let sample = SampleRow {
            loss_tolerance: row.get::<_, Option<String>>(0),
            width: row.get::<_, i32>(1) as u32,
            height: row.get::<_, i32>(2) as u32,
            duration_secs: row.get(3),
            frame_count: row.get::<_, i64>(4) as u64,
            file_size_bytes: row.get::<_, i64>(5) as u64,
            fps: row.get::<_, Option<f64>>(6).unwrap_or(0.0),
            temporal_bpp: row.get(7),
            spatial_bpp: row.get(8),
            has_transparency: row.get(9),
            has_embedded_icc: row.get(10),
            has_complex_color_profile: row.get(11),
            palette_size: row.get::<_, Option<i32>>(12).map(|v| v as u32),
            frame_payload_variation: row.get(13),
            frame_delay_variation: row.get(14),
            aspect_ratio: row.get(15),
            labeled_by: row.get(16),
            total_pixels: row.get::<_, Option<i64>>(17).map(|v| v as u64),
            loop_frequency: row.get(18),
            is_meme_platform: row.get(19),
            is_human_semantic_name: row.get(20),
            cadence_score: row.get(21),
            is_high_value_source: row.get(22),
            is_native_gif: row.get(23),
            palette_depth: row.get(24),
            motion_gini: row.get(25),
            block_skew: row.get(26),
            temporal_flatness: row.get(27),
            directory_meme_score: row.get::<_, Option<f64>>(28),
            webp_compression_ratio: row.get::<_, Option<f64>>(29),
        };

        let distance = sample_distance(
            meta,
            &sample,
            target_temporal_bpp,
            target_spatial_bpp,
            &feature_stats,
        );
        candidates.push((sample, distance));
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let neighbor_count = adaptive_neighbor_count(candidates.len());
    let neighbors = &candidates[..neighbor_count];

    let min_distance = neighbors.first().map_or(0.0, |(_, d)| *d);
    let radius = dynamic_neighbor_radius(neighbors);

    let mut weighted_keep = 0.0;
    let mut total_weight = 0.0;
    let mut distances = Vec::new();
    let mut loop_durations = Vec::new();

    for (sample, distance) in neighbors {
        if *distance > radius {
            continue;
        }

        let relative_distance = (*distance - min_distance).max(0.0);
        let weight = 1.0 / (1.0 + relative_distance * relative_distance * 3.0);
        let prob = match sample.loss_tolerance.as_deref() {
            Some("high") => 1.0,
            Some("low") => 0.0,
            _ => 0.5,
        };

        if prob >= 0.5 {
            loop_durations.push(sample.duration_secs);
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
        exact_label: None,
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
    // Enable pgvector extension
    conn.execute("CREATE EXTENSION IF NOT EXISTS vector", &[])?;

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
            features vector(18)
        )",
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
        "CREATE INDEX IF NOT EXISTS idx_samples_lookup
         ON samples(loss_tolerance, width, height, duration_secs, has_transparency)",
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

    let mut tx = conn.transaction()?;

    // Seed default dataset shipped with the binary (PostgreSQL-native SQL)
    let default_sql = include_str!("./sql/default_samples.sql");
    tx.batch_execute(default_sql).unwrap_or_else(|e| {
        log::warn!("⚠️ Failed to seed default GIF value dataset: {e}");
    });

    tx.execute(
        "INSERT INTO sample_metadata (key, value) VALUES ($1, 'done')
         ON CONFLICT (key) DO UPDATE SET value = 'done'",
        &[&IMPORT_KEY],
    )?;
    tx.commit()?;

    // Recalculate stats based on the newly seeded data
    let _ = refresh_feature_stats(conn);
    Ok(())
}

struct SampleInsert {
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
fn sample_from_path(path: &Path, labeled_by: &str) -> Option<SampleInsert> {
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

    let loss_tolerance = determine_loss_tolerance(
        temporal_bpp,
        meta.has_embedded_icc,
        meta.has_complex_color_profile,
        meta.app_extensions.as_deref(),
        path,
        meta.file_name.as_deref(),
    );

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

fn sample_distance(
    meta: &LoopMeta,
    sample: &SampleRow,
    target_temporal_bpp: f64,
    target_spatial_bpp: f64,
    stats_map: &FeatureMap,
) -> f64 {
    let target_pixels = (f64::from(meta.width) * f64::from(meta.height)).max(1.0);
    let sample_pixels = (f64::from(sample.width) * f64::from(sample.height)).max(1.0);
    let target_loop_frequency =
        crate::loop_intent::score_loop_frequency(meta.duration_secs, meta.frame_count);
    let target_analysis = crate::loop_intent::analyze_filename(meta.file_name.as_deref(), &[]);
    let target_is_human_semantic_name =
        target_analysis.kind == crate::loop_intent::FilenameKind::HumanSemantic;
    let target_cadence_score =
        crate::loop_intent::score_sparse_cadence(meta.duration_secs, meta.frame_count);
    // Use a continuous directory-context score for target -> sample distance
    let target_directory_meme_score =
        crate::loop_intent::score_directory_context(meta.parent_directories.as_deref(), &[]);
    let target_is_native_gif = meta.source_extension.as_deref() == Some("gif");

    // GIPHY_PLATFORM_MARKERS check moved to loop_intent helpers or inline
    const PLATFORM_MARKERS: &[&str] =
        &["GIPHY", "TENOR", "STICKER", "TELEGRAM", "TIKTOK", "DISCORD"];
    let target_is_meme_platform = meta.app_extensions.as_ref().is_some_and(|exts| {
        exts.iter().any(|e| {
            let up = e.to_uppercase();
            PLATFORM_MARKERS.iter().any(|&m| up.contains(m))
        })
    });

    // Affinity is now a derived calculation
    let target_loop_affinity = (target_loop_frequency * 0.45
        + target_cadence_score * 0.25
        + (if target_is_native_gif { 1.0 } else { 0.55 }) * 0.20
        + 0.5 * 0.10) // default fps score for target
        .clamp(0.0, 1.0);

    let target_is_high_value_source =
        meta.has_embedded_icc || meta.has_complex_color_profile || meta.has_audio;
    let target_frame_density = meta.frame_count as f64 / meta.duration_secs.max(0.05);
    let sample_frame_density = sample.frame_count as f64 / sample.duration_secs.max(0.05);
    let target_frame_gap = meta.duration_secs / meta.frame_count.max(1) as f64;
    let sample_frame_gap = sample.duration_secs / sample.frame_count.max(1) as f64;
    let sample_audio_score = if sample.is_native_gif { 1.0 } else { 0.55 };
    let sample_fps_score =
        (1.0 - normalize_log_ratio(sample.fps.max(1e-3), meta.fps.max(1e-3), 1.2)).clamp(0.0, 1.0);
    let sample_loop_affinity = (sample.loop_frequency.unwrap_or(0.5) * 0.45
        + sample.cadence_score.unwrap_or(0.5) * 0.25
        + sample_audio_score * 0.20
        + sample_fps_score * 0.10)
        .clamp(0.0, 1.0);

    let get_std = |f: &str| stats_map.stats.get(f).map_or(1.0, |s| s.std_dev).max(1e-6);

    // Standardized Euclidean: d = sqrt(sum((x - y)/sigma)^2)
    let d_pix = (target_pixels - sample_pixels) / get_std("pixels");
    let d_dur = (meta.duration_secs - sample.duration_secs) / get_std("duration");
    let d_frm = (meta.frame_count as f64 - sample.frame_count as f64) / get_std("frame_count");
    let d_fsize =
        (meta.file_size_bytes as f64 - sample.file_size_bytes as f64) / get_std("file_size_bytes");
    let d_dens = (target_frame_density - sample_frame_density) / get_std("density");
    let d_gap = (target_frame_gap - sample_frame_gap) / get_std("gap");
    let d_tbpp = (target_temporal_bpp - sample.temporal_bpp) / get_std("temporal_bpp");
    let d_sbpp = (target_spatial_bpp - sample.spatial_bpp) / get_std("spatial_bpp");

    let target_webp_ratio = meta.webp_compression_ratio.unwrap_or(0.0);
    let sample_webp_ratio = sample.webp_compression_ratio.unwrap_or(0.0);
    let d_wratio = (target_webp_ratio - sample_webp_ratio) / get_std("webp_ratio");

    let d_lfreq =
        (target_loop_frequency - sample.loop_frequency.unwrap_or(0.5)) / get_std("loop_freq");
    let d_laffin = (target_loop_affinity - sample_loop_affinity) / get_std("loop_affin");
    let d_cadence =
        (target_cadence_score - sample.cadence_score.unwrap_or(0.5)) / get_std("cadence");

    // Boolean features (categorical) still use fixed penalties in [0, 1] scale
    let bool_dist = |a: bool, b: bool, w: f64| if a == b { 0.0 } else { w };
    let meme_platform_dist = bool_dist(target_is_meme_platform, sample.is_meme_platform, 1.2);
    let name_dist = bool_dist(
        target_is_human_semantic_name,
        sample.is_human_semantic_name,
        0.8,
    );
    // Directory-context distance: compare continuous scores (missing -> neutral 0.5)
    let sample_directory_meme_score = sample.directory_meme_score.unwrap_or(0.5);
    let dir_hint_dist = (target_directory_meme_score - sample_directory_meme_score).abs();
    let native_gif_dist = bool_dist(target_is_native_gif, sample.is_native_gif, 0.6);
    let high_value_dist = bool_dist(
        target_is_high_value_source,
        sample.is_high_value_source,
        1.5,
    );
    let trans_dist = bool_dist(meta.has_transparency, sample.has_transparency, 1.5);

    let color_distance = if meta.has_embedded_icc == sample.has_embedded_icc
        && meta.has_complex_color_profile == sample.has_complex_color_profile
    {
        0.0
    } else {
        1.2
    };

    let d_payload = (meta.frame_payload_variation.unwrap_or(0.5)
        - sample.frame_payload_variation.unwrap_or(0.5))
        / get_std("payload_var");
    let d_delay = (meta.frame_delay_variation.unwrap_or(0.5)
        - sample.frame_delay_variation.unwrap_or(0.5))
        / get_std("delay_var");

    let target_aspect = if meta.height > 0 {
        Some(f64::from(meta.width) / f64::from(meta.height))
    } else {
        None
    };
    let d_aspect =
        (target_aspect.unwrap_or(1.0) - sample.aspect_ratio.unwrap_or(1.0)) / get_std("aspect");
    let d_pal = (meta.palette_size.map_or(256.0, f64::from)
        - sample.palette_size.map_or(256.0, f64::from))
        / 256.0;

    let d_pdepth = (meta.palette_depth.unwrap_or(0.5) - sample.palette_depth.unwrap_or(0.5))
        / get_std("p_depth");
    let d_mgini =
        (meta.motion_gini.unwrap_or(0.5) - sample.motion_gini.unwrap_or(0.5)) / get_std("m_gini");
    let d_bskew =
        (meta.block_skew.unwrap_or(0.5) - sample.block_skew.unwrap_or(0.5)) / get_std("b_skew");
    let d_tflat = (meta.temporal_flatness.unwrap_or(0.5) - sample.temporal_flatness.unwrap_or(0.5))
        / get_std("t_flat");

    let label_penalty = if sample.labeled_by.as_deref() == Some("auto") {
        0.8
    } else {
        0.0
    };

    // Sum of Squares
    let sos = d_pix.powi(2) * 0.4
        + d_dur.powi(2) * 1.5
        + d_frm.powi(2) * 0.3
        + d_fsize.powi(2) * 0.9
        + d_dens.powi(2) * 0.8
        + d_gap.powi(2) * 0.8
        + d_tbpp.powi(2) * 1.2
        + d_sbpp.powi(2) * 0.6
        + d_lfreq.powi(2) * 1.0
        + d_laffin.powi(2) * 1.5
        + d_cadence.powi(2) * 1.0
        + d_payload.powi(2) * 1.1
        + d_delay.powi(2) * 0.9
        + d_aspect.powi(2) * 0.7
        + d_pal.powi(2) * 0.1
        + d_pdepth.powi(2) * 1.4
        + d_mgini.powi(2) * 1.2
        + d_bskew.powi(2) * 1.0
        + d_tflat.powi(2) * 1.3
        + d_wratio.powi(2) * 1.0;

    sos.sqrt()
        + meme_platform_dist
        + name_dist
        + dir_hint_dist
        + native_gif_dist
        + trans_dist
        + color_distance
        + high_value_dist
        + label_penalty
}

#[allow(dead_code)]
fn variation_distance(a: Option<f64>, b: Option<f64>, missing_penalty: f64) -> f64 {
    match (a, b) {
        (Some(lhs), Some(rhs)) => (lhs - rhs).abs(),
        (None, None) => 0.0,
        _ => missing_penalty,
    }
}

fn adaptive_neighbor_count(total: usize) -> usize {
    ((total as f64).sqrt().round() as usize)
        .clamp(6, 24)
        .min(total)
}

fn dynamic_neighbor_radius(neighbors: &[(SampleRow, f64)]) -> f64 {
    let mut distances: Vec<f64> = neighbors.iter().map(|(_, d)| *d).collect();
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
        p10: percentile_value(&sorted, 0.10),
        p25: percentile_value(&sorted, 0.25),
        p50: percentile_value(&sorted, 0.50),
        p75: percentile_value(&sorted, 0.75),
        p90: percentile_value(&sorted, 0.90),
    }
}

pub fn batch_ingest_samples(dataset_path: &Path) -> Result<usize> {
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
        if ["gif", "webp", "apng", "avif"].contains(&ext.as_str()) {
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
            let res = sample_from_path(path, "cli_ingest");
            pb.inc(1);
            if let Some(s) = &res {
                // 排除静态图片：只有多帧内容才具备循环意图训练价值
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
    let rows = conn.query(
        "SELECT
            width, height, duration_secs, frame_count, file_size_bytes, fps,
            temporal_bpp, spatial_bpp, frame_payload_variation, frame_delay_variation,
            aspect_ratio, loop_frequency, cadence_score, palette_depth, motion_gini,
            block_skew, temporal_flatness, webp_compression_ratio
         FROM samples WHERE loss_tolerance IS NOT NULL",
        &[],
    )?;

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
             SELECT unnest(regexp_split_to_array(lower(file_name), '[^a-z0-9一-龥]+')) as word 
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

    Ok(())
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
        let mut feature_map = FeatureMap::default();
        feature_map.top_keywords = vec!["meme".to_string()];
        feature_map.stats.insert(
            "duration".to_string(),
            FeatureStats {
                mean: 6.0,
                std_dev: 2.0,
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
            },
        );

        let profile = build_loop_reference_profile(GlobalCollectionStats::default(), &feature_map);
        assert_eq!(profile.duration.p25, Some(2.0));
        assert_eq!(profile.fps.mean, 14.0);
        assert_eq!(profile.top_keywords, vec!["meme".to_string()]);
    }
}
