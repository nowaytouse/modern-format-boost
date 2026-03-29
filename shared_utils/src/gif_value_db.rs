use crate::gif_meme_score::{gif_meta_from_probe_with_path, scan_gif_headers, GifMeta};
use anyhow::{Context, Result};
use blake3::Hasher;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rusqlite::{params, Connection, OpenFlags};
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DB_FILE_NAME: &str = "gif_value_samples_v3.db";
const IMPORT_KEY: &str = "dataset_seeds_import_v3";

#[derive(Debug, Clone)]
pub struct SampleMatch {
    pub exact_label: Option<bool>,
    pub keep_probability: Option<f64>,
    pub neighbor_count: usize,
    pub mean_distance: Option<f64>,
}

#[derive(Debug, Clone)]
struct SampleRow {
    loss_tolerance: Option<String>,
    width: u32,
    height: u32,
    duration_secs: f64,
    frame_count: u64,
    temporal_bpp: f64,
    spatial_bpp: f64,
    has_transparency: bool,
    has_embedded_icc: bool,
    has_complex_color_profile: bool,
    palette_size: Option<u32>,
    frame_payload_variation: Option<f64>,
    frame_delay_variation: Option<f64>,
    aspect_ratio: Option<f64>,
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
            }));
        }
    }

    let target_pixels = f64::from(meta.width) * f64::from(meta.height);
    let target_temporal_bpp =
        meta.file_size_bytes as f64 / ((target_pixels.max(1.0)) * meta.frame_count.max(1) as f64);
    let target_spatial_bpp = meta.file_size_bytes as f64 / target_pixels.max(1.0);

    let min_pixels = (target_pixels * 0.20).max(1024.0) as i64;
    let max_pixels = (target_pixels * 5.0).max(min_pixels as f64 + 1.0) as i64;
    let min_duration = (meta.duration_secs - 8.0).max(0.0);
    let max_duration = meta.duration_secs + 8.0;

    let mut stmt = conn.prepare(
        "SELECT
            loss_tolerance, width, height, duration_secs, frame_count,
            temporal_bpp, spatial_bpp,
            has_transparency, has_embedded_icc, has_complex_color_profile,
            palette_size, frame_payload_variation, frame_delay_variation,
            aspect_ratio, labeled_by
         FROM samples
         WHERE loss_tolerance IS NOT NULL
           AND (width * height) BETWEEN ?1 AND ?2
           AND duration_secs BETWEEN ?3 AND ?4
         LIMIT 96",
    )?;

    let rows = stmt.query_map(
        params![min_pixels, max_pixels, min_duration, max_duration],
        |row| {
            Ok(SampleRow {
                loss_tolerance: row.get::<_, Option<String>>(0)?,
                width: row.get(1)?,
                height: row.get(2)?,
                duration_secs: row.get(3)?,
                frame_count: row.get::<_, i64>(4)? as u64,
                temporal_bpp: row.get(5)?,
                spatial_bpp: row.get(6)?,
                has_transparency: row.get::<_, i64>(7)? == 1,
                has_embedded_icc: row.get::<_, i64>(8)? == 1,
                has_complex_color_profile: row.get::<_, i64>(9)? == 1,
                palette_size: row.get(10)?,
                frame_payload_variation: row.get(11)?,
                frame_delay_variation: row.get(12)?,
                aspect_ratio: row.get(13)?,
                labeled_by: row.get(14)?,
            })
        },
    )?;

    let mut weighted_keep = 0.0;
    let mut total_weight = 0.0;
    let mut distances = Vec::new();

    for sample in rows.flatten() {
        let distance = sample_distance(meta, &sample, target_temporal_bpp, target_spatial_bpp);
        if distance > 1.35 {
            continue;
        }
        let weight = 1.0 / (1.0 + distance * distance);

        let prob = match sample.loss_tolerance.as_deref() {
            Some("high") => 1.0,
            Some("low") => 0.0,
            _ => 0.5,
        };

        weighted_keep += prob * weight;
        total_weight += weight;
        distances.push(distance);
    }

    if distances.is_empty() {
        return Ok(None);
    }

    let keep_probability = weighted_keep / total_weight.max(1e-6);
    let mean_distance = distances.iter().sum::<f64>() / distances.len() as f64;

    Ok(Some(SampleMatch {
        exact_label: None,
        keep_probability: Some(keep_probability),
        neighbor_count: distances.len(),
        mean_distance: Some(mean_distance),
    }))
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
            loss_tolerance TEXT,
            labeled_by TEXT,
            created_at TEXT DEFAULT (datetime('now'))
        )",
        [],
    )?;

    // Migration: add aspect_ratio column if it was added after initial schema creation
    let _ = conn.execute("ALTER TABLE samples ADD COLUMN aspect_ratio REAL", []);

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
    if let Ok((pal, exts, has_transparency, variation, delay_variation)) = scan_gif_headers(path) {
        meta.palette_size = pal;
        meta.app_extensions = exts;
        meta.has_transparency = has_transparency;
        meta.frame_payload_variation = variation;
        meta.frame_delay_variation = delay_variation;
    }

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
) -> f64 {
    let target_pixels = (f64::from(meta.width) * f64::from(meta.height)).max(1.0);
    let sample_pixels = (f64::from(sample.width) * f64::from(sample.height)).max(1.0);
    let pixel_distance = (target_pixels.log2() - sample_pixels.log2()).abs() / 3.0;
    let duration_distance = (meta.duration_secs - sample.duration_secs).abs() / 8.0;
    let frame_distance = (meta.frame_count as f64 - sample.frame_count as f64).abs()
        / meta.frame_count.max(1) as f64;
    let temporal_bpp_distance = (target_temporal_bpp - sample.temporal_bpp).abs() / 0.08;
    let spatial_bpp_distance = (target_spatial_bpp - sample.spatial_bpp).abs() / 15.0;
    let transparency_distance = if meta.has_transparency == sample.has_transparency {
        0.0
    } else {
        1.5
    };
    let color_distance = if meta.has_embedded_icc == sample.has_embedded_icc
        && meta.has_complex_color_profile == sample.has_complex_color_profile
    {
        0.0
    } else {
        1.2
    };
    let palette_distance = match (meta.palette_size, sample.palette_size) {
        (Some(a), Some(b)) => (f64::from(a) - f64::from(b)).abs() / 256.0,
        _ => 0.2,
    };
    let payload_variation_distance = variation_distance(
        meta.frame_payload_variation,
        sample.frame_payload_variation,
        0.5,
    );
    let delay_variation_distance = variation_distance(
        meta.frame_delay_variation,
        sample.frame_delay_variation,
        0.35,
    );

    let target_aspect = if meta.height > 0 {
        Some(f64::from(meta.width) / f64::from(meta.height))
    } else {
        None
    };
    let aspect_distance = match (target_aspect, sample.aspect_ratio) {
        (Some(a), Some(b)) => {
            // Normalize: typical ratios span 0.3 (portrait 9:16) to 3.0 (ultra-wide)
            // Penalty is largest when comparing square meme (1.0) vs widescreen video (1.78)
            (a - b).abs() / 1.5
        }
        _ => 0.15,
    };

    let label_penalty = if sample.labeled_by.as_deref() == Some("auto") {
        0.8 // Hefty penalty for auto-labeled heuristics to prevent an echo chamber
    } else {
        0.0 // True manual cli_ingest samples get zero penalty
    };

    pixel_distance * 0.8
        + duration_distance * 0.7
        + frame_distance * 0.4
        + temporal_bpp_distance * 1.2
        + spatial_bpp_distance * 0.6
        + transparency_distance
        + color_distance
        + palette_distance * 0.4
        + payload_variation_distance * 0.7
        + delay_variation_distance * 0.8
        + aspect_distance * 0.9
        + label_penalty
}

fn variation_distance(a: Option<f64>, b: Option<f64>, missing_penalty: f64) -> f64 {
    match (a, b) {
        (Some(lhs), Some(rhs)) => (lhs - rhs).abs(),
        (None, None) => 0.0,
        _ => missing_penalty,
    }
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
                temporal_bpp, spatial_bpp, loss_tolerance, labeled_by, aspect_ratio
             ) VALUES (
                ?1, ?2, ?3, ?4,
                ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13,
                ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21
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
            ]);
            if res.is_ok() {
                count += 1;
            }
        }
    }

    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_meta() -> GifMeta {
        GifMeta {
            duration_secs: 2.0,
            width: 320,
            height: 320,
            fps: 12.0,
            frame_count: 24,
            file_size_bytes: 120_000,
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
            temporal_bpp: 0.05,
            spatial_bpp: 1.2,
            has_transparency: true,
            has_embedded_icc: false,
            has_complex_color_profile: false,
            palette_size: Some(64),
            frame_payload_variation: Some(0.35),
            frame_delay_variation: Some(0.55),
            aspect_ratio: Some(1.0),
            labeled_by: Some("cli_ingest".to_string()),
        };
        let far = SampleRow {
            loss_tolerance: Some("low".to_string()),
            width: 1920,
            height: 1080,
            duration_secs: 20.0,
            frame_count: 600,
            temporal_bpp: 0.4,
            spatial_bpp: 35.0,
            has_transparency: false,
            has_embedded_icc: true,
            has_complex_color_profile: true,
            palette_size: Some(256),
            frame_payload_variation: Some(0.05),
            frame_delay_variation: Some(0.02),
            aspect_ratio: Some(1.78),
            labeled_by: Some("cli_ingest".to_string()),
        };
        let pixel_count = f64::from(meta.width) * f64::from(meta.height);
        let tbpp = meta.file_size_bytes as f64 / (pixel_count * meta.frame_count as f64);
        let sbpp = meta.file_size_bytes as f64 / pixel_count;

        assert!(
            sample_distance(&meta, &near, tbpp, sbpp) < sample_distance(&meta, &far, tbpp, sbpp)
        );
    }
}
