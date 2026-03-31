use crate::gif_meme_score::{gif_meta_from_probe_with_path, scan_gif_headers, GifMeta};
use anyhow::{Context, Result};
use blake3::Hasher;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rusqlite::{params, Connection, OpenFlags};
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DB_FILE_NAME: &str = "gif_value_samples_v4.db";
const IMPORT_KEY: &str = "dataset_seeds_import_v4";
const STATS_KEY: &str = "feature_stats_v1";

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FeatureStats {
    mean: f64,
    std_dev: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FeatureMap {
    stats: std::collections::HashMap<String, FeatureStats>,
}

#[derive(Debug, Clone)]
pub struct SampleMatch {
    pub exact_label: Option<bool>,
    pub keep_probability: Option<f64>,
    pub neighbor_count: usize,
    pub mean_distance: Option<f64>,
    pub std_dev_distance: Option<f64>,
    pub min_distance: Option<f64>,
    pub p25_distance: Option<f64>,
    pub p75_distance: Option<f64>,
}

#[derive(Debug, Clone)]
struct SampleRow {
    loss_tolerance: Option<String>,
    width: u32,
    height: u32,
    duration_secs: f64,
    frame_count: u64,
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
    directory_meme_hint: bool,
    is_high_value_source: bool,
    is_native_gif: bool,
    palette_depth: Option<f64>,
    motion_gini: Option<f64>,
    block_skew: Option<f64>,
    temporal_flatness: Option<f64>,
    labeled_by: Option<String>,
}

pub fn lookup_similar_samples(meta: &GifMeta, path: Option<&Path>) -> Option<SampleMatch> {
    lookup_similar_samples_inner(meta, path).ok().flatten()
}

fn lookup_similar_samples_inner(
    meta: &GifMeta,
    path: Option<&Path>,
) -> Result<Option<SampleMatch>> {
    let db_path = sample_db_path()?;
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
    )
    .with_context(|| format!("Failed to open gif value db: {}", db_path.display()))?;

    init_schema(&conn)?;
    seed_positive_dataset_if_needed(&conn)?;

    if let Some(path) = path {
        let file_hash = calculate_blake3_hex(path)?;
        if let Ok(Some(tol)) = conn.query_row(
            "SELECT loss_tolerance FROM samples WHERE file_hash = ?1 LIMIT 1",
            params![file_hash],
            |row| row.get::<_, Option<String>>(0),
        ) {
            let prob = match tol.as_str() {
                "high" => 1.0,
                "low" => 0.0,
                _ => 0.5,
            };
            return Ok(Some(SampleMatch {
                exact_label: Some(prob > 0.5),
                keep_probability: Some(prob),
                neighbor_count: 1,
                mean_distance: Some(0.0),
                std_dev_distance: Some(0.0),
                min_distance: Some(0.0),
                p25_distance: Some(0.0),
                p75_distance: Some(0.0),
            }));
        }
    }

    let target_pixels = f64::from(meta.width) * f64::from(meta.height);
    let target_temporal_bpp =
        meta.file_size_bytes as f64 / ((target_pixels.max(1.0)) * meta.frame_count.max(1) as f64);
    let target_spatial_bpp = meta.file_size_bytes as f64 / target_pixels.max(1.0);

    let feature_stats: FeatureMap = conn
        .query_row(
            "SELECT value FROM sample_metadata WHERE key = ?1",
            params![STATS_KEY],
            |row| {
                let s: String = row.get(0)?;
                Ok(serde_json::from_str(&s).unwrap_or_default())
            },
        )
        .unwrap_or_default();

    let mut stmt = conn.prepare(
        "SELECT
            loss_tolerance, width, height, duration_secs, frame_count,
            fps, temporal_bpp, spatial_bpp,
            has_transparency, has_embedded_icc, has_complex_color_profile,
            palette_size, frame_payload_variation, frame_delay_variation,
            aspect_ratio, labeled_by,
            total_pixels, loop_frequency, is_meme_platform, is_human_semantic_name,
            cadence_score, directory_meme_hint, is_high_value_source, is_native_gif,
            palette_depth, motion_gini, block_skew, temporal_flatness
         FROM samples
         WHERE loss_tolerance IS NOT NULL
         LIMIT 1024",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(SampleRow {
            loss_tolerance: row.get::<_, Option<String>>(0)?,
            width: row.get(1)?,
            height: row.get(2)?,
            duration_secs: row.get(3)?,
            frame_count: row.get::<_, i64>(4)? as u64,
            fps: row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
            temporal_bpp: row.get(6)?,
            spatial_bpp: row.get(7)?,
            has_transparency: row.get::<_, i64>(8)? == 1,
            has_embedded_icc: row.get::<_, i64>(9)? == 1,
            has_complex_color_profile: row.get::<_, i64>(10)? == 1,
            palette_size: row.get(11)?,
            frame_payload_variation: row.get(12)?,
            frame_delay_variation: row.get(13)?,
            aspect_ratio: row.get(14)?,
            total_pixels: row.get::<_, Option<i64>>(16)?.map(|v| v as u64),
            loop_frequency: row.get(17)?,
            is_meme_platform: row.get::<_, i64>(18)? == 1,
            is_human_semantic_name: row.get::<_, i64>(19)? == 1,
            cadence_score: row.get(20)?,
            directory_meme_hint: row.get::<_, i64>(21)? == 1,
            is_high_value_source: row.get::<_, i64>(22)? == 1,
            is_native_gif: row.get::<_, i64>(23)? == 1,
            palette_depth: row.get(24)?,
            motion_gini: row.get(25)?,
            block_skew: row.get(26)?,
            temporal_flatness: row.get(27)?,
            labeled_by: row.get(15)?,
        })
    })?;

    let mut candidates = Vec::new();

    for sample in rows.flatten() {
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

    // Sort for percentiles
    distances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = distances.len();
    let min_distance = distances.first().copied();
    let p25_distance = distances.get(n / 4).copied();
    let p75_distance = distances.get(3 * n / 4).copied();

    Ok(Some(SampleMatch {
        exact_label: None,
        keep_probability: Some(keep_probability),
        neighbor_count: distances.len(),
        mean_distance: Some(mean_distance),
        std_dev_distance: Some(std_dev_distance),
        min_distance,
        p25_distance,
        p75_distance,
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
fn resolved_duration_secs(meta: &GifMeta) -> f64 {
    if meta.duration_secs > 0.11 {
        meta.duration_secs
    } else if meta.frame_count > 1 && meta.fps > 0.1 {
        meta.frame_count as f64 / meta.fps
    } else {
        meta.frame_count.max(1) as f64 / 12.0
    }
}

#[must_use]
pub fn is_lossless_exploration_safe(meta: &GifMeta, path: Option<&Path>) -> bool {
    let mut current_meta = meta.clone();
    if let Some(p) = path {
        let _ = crate::gif_meme_score::deep_refine_meta(&mut current_meta, p);
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

fn sample_db_path() -> Result<PathBuf> {
    let mut path = crate::common_utils::get_user_project_cache_dir()?;
    path.push(DB_FILE_NAME);
    Ok(path)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS samples (
            file_hash TEXT PRIMARY KEY,
            source_path TEXT,
            file_name TEXT,
            source_ext TEXT,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            duration_secs REAL NOT NULL,
            frame_count INTEGER NOT NULL,
            file_size_bytes INTEGER NOT NULL,
            fps REAL,
            has_embedded_icc INTEGER NOT NULL DEFAULT 0,
            has_complex_color_profile INTEGER NOT NULL DEFAULT 0,
            has_transparency INTEGER NOT NULL DEFAULT 0,
            palette_size INTEGER,
            frame_payload_variation REAL,
            frame_delay_variation REAL,
            temporal_bpp REAL NOT NULL,
            spatial_bpp REAL NOT NULL,
            aspect_ratio REAL,
            total_pixels INTEGER,
            loop_frequency REAL,
            is_meme_platform INTEGER DEFAULT 0,
            is_human_semantic_name INTEGER DEFAULT 0,
            cadence_score REAL,
            directory_meme_hint INTEGER DEFAULT 0,
            is_high_value_source INTEGER DEFAULT 0,
            is_native_gif INTEGER DEFAULT 0,
            palette_depth REAL,
            motion_gini REAL,
            block_skew REAL,
            temporal_flatness REAL,
            loss_tolerance TEXT,
            labeled_by TEXT,
            created_at TEXT DEFAULT (datetime('now'))
        )",
        [],
    )?;

    // Migration: ensure columns exist for v4 schema if incremental upgrade occurs
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN aspect_ratio REAL", []);
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN total_pixels INTEGER", []);
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN loop_frequency REAL", []);
    let _ = conn.execute(
        "ALTER TABLE samples ADD COLUMN is_meme_platform INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE samples ADD COLUMN is_human_semantic_name INTEGER",
        [],
    );
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN cadence_score REAL", []);
    let _ = conn.execute(
        "ALTER TABLE samples ADD COLUMN directory_meme_hint INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE samples ADD COLUMN is_high_value_source INTEGER",
        [],
    );
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN is_native_gif INTEGER", []);
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN palette_depth REAL", []);
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN motion_gini REAL", []);
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN block_skew REAL", []);
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN temporal_flatness REAL", []);

    conn.execute(
        "CREATE TABLE IF NOT EXISTS sample_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_samples_lookup
         ON samples(loss_tolerance, width, height, duration_secs, has_transparency)",
        [],
    )?;

    Ok(())
}

fn seed_positive_dataset_if_needed(conn: &Connection) -> Result<()> {
    if cfg!(test) || std::env::var("MODERN_FORMAT_BOOST_DISABLE_SAMPLE_DB").is_ok() {
        return Ok(());
    }

    let imported = conn
        .query_row(
            "SELECT value FROM sample_metadata WHERE key = ?1",
            params![IMPORT_KEY],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .is_some_and(|v| v == "done");

    if imported {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;

    // Seed default dataset shipped with the binary
    let default_sql = include_str!("default_samples.sql");
    tx.execute_batch(default_sql).unwrap_or_else(|e| {
        log::warn!("⚠️ Failed to seed default GIF value dataset: {e}");
    });

    tx.execute(
        "INSERT OR REPLACE INTO sample_metadata (key, value) VALUES (?1, 'done')",
        params![IMPORT_KEY],
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
    directory_meme_hint: bool,
    is_high_value_source: bool,
    is_native_gif: bool,
    palette_depth: Option<f64>,
    motion_gini: Option<f64>,
    block_skew: Option<f64>,
    temporal_flatness: Option<f64>,
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
        "作品",
        "作者",
        "画师",
        "插画",
        "收藏",
        "原作",
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
        "meme", "sticker", "emoji", "reaction", "表情", "贴纸", "斗图", "梗",
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
    let file_size = std::fs::metadata(path).ok()?.len();
    let probe = crate::probe_video(path).ok()?;
    let mut meta = gif_meta_from_probe_with_path(&probe, file_size, path)?;
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
    let _ = crate::gif_meme_score::deep_refine_meta(&mut meta, path);

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
        crate::gif_meme_score::score_loop_frequency(meta.duration_secs, meta.frame_count);
    let analysis = crate::gif_meme_score::analyze_filename(meta.file_name.as_deref());
    let is_human_semantic_name =
        analysis.kind == crate::gif_meme_score::FilenameKind::HumanSemantic;
    let cadence_score =
        crate::gif_meme_score::score_sparse_cadence(meta.duration_secs, meta.frame_count);
    let directory_meme_hint =
        crate::gif_meme_score::score_directory_context(meta.parent_directories.as_deref()) > 0.5;
    let is_native_gif = meta.source_extension.as_deref() == Some("gif");
    let is_high_value_source = loss_tolerance == "low";

    let is_meme_platform = meta.app_extensions.as_ref().is_some_and(|exts| {
        exts.iter().any(|e| {
            crate::gif_meme_score::MEME_PLATFORM_PREFIXES
                .iter()
                .any(|p| e.starts_with(p))
        })
    });

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
        loss_tolerance,
        labeled_by: labeled_by.to_string(),
        aspect_ratio,
        total_pixels,
        loop_frequency,
        is_meme_platform,
        is_human_semantic_name,
        cadence_score,
        directory_meme_hint,
        is_high_value_source,
        is_native_gif,
        palette_depth: meta.palette_depth,
        motion_gini: meta.motion_gini,
        block_skew: meta.block_skew,
        temporal_flatness: meta.temporal_flatness,
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
    meta: &GifMeta,
    sample: &SampleRow,
    target_temporal_bpp: f64,
    target_spatial_bpp: f64,
    stats_map: &FeatureMap,
) -> f64 {
    let target_pixels = (f64::from(meta.width) * f64::from(meta.height)).max(1.0);
    let sample_pixels = (f64::from(sample.width) * f64::from(sample.height)).max(1.0);
    let target_loop_frequency =
        crate::gif_meme_score::score_loop_frequency(meta.duration_secs, meta.frame_count);
    let target_loop_affinity = crate::gif_meme_score::score_loop_affinity(meta);
    let target_analysis = crate::gif_meme_score::analyze_filename(meta.file_name.as_deref());
    let target_is_human_semantic_name =
        target_analysis.kind == crate::gif_meme_score::FilenameKind::HumanSemantic;
    let target_cadence_score =
        crate::gif_meme_score::score_sparse_cadence(meta.duration_secs, meta.frame_count);
    let target_directory_meme_hint =
        crate::gif_meme_score::score_directory_context(meta.parent_directories.as_deref()) > 0.5;
    let target_is_native_gif = meta.source_extension.as_deref() == Some("gif");
    let target_is_meme_platform = meta.app_extensions.as_ref().is_some_and(|exts| {
        exts.iter().any(|e| {
            crate::gif_meme_score::MEME_PLATFORM_PREFIXES
                .iter()
                .any(|p| e.starts_with(p))
        })
    });

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
    let d_dens = (target_frame_density - sample_frame_density) / get_std("density");
    let d_gap = (target_frame_gap - sample_frame_gap) / get_std("gap");
    let d_tbpp = (target_temporal_bpp - sample.temporal_bpp) / get_std("temporal_bpp");
    let d_sbpp = (target_spatial_bpp - sample.spatial_bpp) / get_std("spatial_bpp");

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
    let dir_hint_dist = bool_dist(target_directory_meme_hint, sample.directory_meme_hint, 1.0);
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
        + d_tflat.powi(2) * 1.3;

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

pub fn batch_ingest_samples(dataset_path: &Path) -> Result<usize> {
    let db_path = sample_db_path()?;
    let mut conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
    )?;

    init_schema(&conn)?;

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
        if ["gif", "webp", "mp4", "mov"].contains(&ext.as_str()) {
            candidate_paths.push(path);
        }
    }

    if candidate_paths.is_empty() {
        println!("⚠️ No matching assets found in designated path.");
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
                pb.set_message(format!("Learn: {}", s.file_name.as_deref().unwrap_or("?")));
            }
            res
        })
        .collect();

    pb.finish_with_message("Learning complete.");

    println!("💾 Persisting {} samples to database...", samples.len());
    let tx = conn.transaction()?;
    let mut count = 0;

    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO samples (
                file_hash, source_path, file_name, source_ext,
                width, height, duration_secs, frame_count, file_size_bytes, fps,
                has_embedded_icc, has_complex_color_profile, has_transparency,
                palette_size, frame_payload_variation, frame_delay_variation,
                temporal_bpp, spatial_bpp, loss_tolerance, labeled_by, aspect_ratio,
                total_pixels, loop_frequency, is_meme_platform, is_human_semantic_name,
                cadence_score, directory_meme_hint, is_high_value_source, is_native_gif,
                palette_depth, motion_gini, block_skew, temporal_flatness
             ) VALUES (
                ?1, ?2, ?3, ?4,
                ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13,
                ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21,
                ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
                ?30, ?31, ?32, ?33
             )",
        )?;

        for sample in samples {
            let res = stmt.execute(params![
                sample.file_hash,
                sample.source_path,
                sample.file_name,
                sample.source_ext,
                sample.width,
                sample.height,
                sample.duration_secs,
                i64::try_from(sample.frame_count).unwrap_or(i64::MAX),
                i64::try_from(sample.file_size_bytes).unwrap_or(i64::MAX),
                sample.fps,
                i64::from(sample.has_embedded_icc),
                i64::from(sample.has_complex_color_profile),
                i64::from(sample.has_transparency),
                sample.palette_size,
                sample.frame_payload_variation,
                sample.frame_delay_variation,
                sample.temporal_bpp,
                sample.spatial_bpp,
                sample.loss_tolerance,
                sample.labeled_by,
                sample.aspect_ratio,
                i64::try_from(sample.total_pixels).unwrap_or(0),
                sample.loop_frequency,
                i64::from(sample.is_meme_platform),
                i64::from(sample.is_human_semantic_name),
                sample.cadence_score,
                i64::from(sample.directory_meme_hint),
                i64::from(sample.is_high_value_source),
                i64::from(sample.is_native_gif),
                sample.palette_depth,
                sample.motion_gini,
                sample.block_skew,
                sample.temporal_flatness,
            ]);
            if res.is_ok() {
                count += 1;
            }
        }
    }

    tx.commit()?;
    refresh_feature_stats(&conn)?;
    Ok(count)
}

fn refresh_feature_stats(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT 
            width, height, duration_secs, frame_count, fps, 
            temporal_bpp, spatial_bpp, palette_size,
            frame_payload_variation, frame_delay_variation,
            aspect_ratio, loop_frequency, cadence_score,
            palette_depth, motion_gini, block_skew, temporal_flatness
         FROM samples WHERE loss_tolerance IS NOT NULL",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(vec![
            row.get::<_, i64>(0)? as f64 * row.get::<_, i64>(1)? as f64, // pixels
            row.get::<_, f64>(2)?,                                       // duration
            row.get::<_, i64>(3)? as f64,                                // frame_count
            row.get::<_, f64>(4)?, // fps (frame density proxy)
            row.get::<_, f64>(5)?, // tbpp
            row.get::<_, f64>(6)?, // sbpp
            row.get::<_, Option<f64>>(10)?.unwrap_or(1.0), // aspect
            row.get::<_, Option<f64>>(11)?.unwrap_or(0.5), // loop_freq
            row.get::<_, Option<f64>>(12)?.unwrap_or(0.5), // cadence
            row.get::<_, Option<f64>>(8)?.unwrap_or(0.5), // payload_var
            row.get::<_, Option<f64>>(9)?.unwrap_or(0.5), // delay_var
            row.get::<_, Option<f64>>(13)?.unwrap_or(0.5), // p_depth
            row.get::<_, Option<f64>>(14)?.unwrap_or(0.5), // m_gini
            row.get::<_, Option<f64>>(15)?.unwrap_or(0.5), // b_skew
            row.get::<_, Option<f64>>(16)?.unwrap_or(0.5), // t_flat
        ])
    })?;

    let all_data: Vec<Vec<f64>> = rows.flatten().collect();
    if all_data.is_empty() {
        return Ok(());
    }

    let names = vec![
        "pixels",
        "duration",
        "frame_count",
        "density",
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
    ];

    let mut feature_map = FeatureMap::default();
    let n = all_data.len() as f64;

    for (idx, name) in names.iter().enumerate() {
        let values: Vec<f64> = all_data
            .iter()
            .map(|v| v.get(idx).copied().unwrap_or(0.0))
            .collect();
        let mean = values.iter().sum::<f64>() / n;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        feature_map.stats.insert(
            name.to_string(),
            FeatureStats {
                mean,
                std_dev: variance.sqrt(),
            },
        );
    }

    let json = serde_json::to_string(&feature_map)?;
    conn.execute(
        "INSERT OR REPLACE INTO sample_metadata (key, value) VALUES (?1, ?2)",
        params![STATS_KEY, json],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_meta() -> GifMeta {
        let frames = 24;
        let duration = 2.0;
        let size = 120_000;
        GifMeta {
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
            parent_directories: None,
            has_embedded_icc: false,
            has_complex_color_profile: false,
            loop_count: None,
            has_audio: false,
            frame_types: vec!['P'; frames as usize],
            pts_deltas: vec![duration / frames as f64; frames as usize],
            mv_magnitudes: Vec::new(),
            palette_depth: None,
            motion_gini: None,
            block_skew: None,
            temporal_flatness: None,
            pkt_sizes: vec![size / frames; frames as usize],
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
            directory_meme_hint: true,
            is_high_value_source: true,
            is_native_gif: true,
            palette_depth: Some(0.8),
            motion_gini: Some(0.7),
            block_skew: Some(0.6),
            temporal_flatness: Some(0.9),
            labeled_by: Some("cli_ingest".to_string()),
        };
        let far = SampleRow {
            loss_tolerance: Some("low".to_string()),
            width: 1920,
            height: 1080,
            duration_secs: 20.0,
            frame_count: 600,
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
            directory_meme_hint: false,
            is_high_value_source: false,
            is_native_gif: false,
            palette_depth: Some(0.1),
            motion_gini: Some(0.2),
            block_skew: Some(0.1),
            temporal_flatness: Some(0.1),
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
}
