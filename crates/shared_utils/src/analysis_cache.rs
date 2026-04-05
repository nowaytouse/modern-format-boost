//! 🗄️ Image Analysis Cache - PostgreSQL Backend
//!
//! 🔥 v3.0: Enhanced cache with content fingerprint + integrity verification
//!
//! Provides a highly efficient, persistent cache for image analysis results using PostgreSQL and `MessagePack`.
//! This ensures that expensive operations like pixel-based entropy calculation, deep HEIC/AVIF parsing,
//! and quantization detection are only performed once per file content.

use crate::image_analyzer::ImageAnalysis;
use crate::image_quality_detector::ImageQualityAnalysis;
use crate::video_detection::VideoDetectionResult;
use anyhow::{Context, Result};
use blake3::Hasher;
use postgres::Client;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

// Import unified version management
use crate::version::{cache_algorithm_version, CACHE_SCHEMA_VERSION};

const PG_DEFAULT_CONNSTR: &str = "host=localhost dbname=modern_format_boost";

/// 📊 Cache Statistics
#[derive(Debug, Clone)]
pub struct CacheStatistics {
    pub db_size_bytes: u64,
    pub analysis_records: usize,
    pub quality_records: usize,
    pub video_records: usize,
    pub path_index_entries: usize,
    pub schema_version: i32,
    pub algorithm_version_distribution: std::collections::HashMap<i32, i64>,
    pub current_algorithm_version: i32,
}

impl CacheStatistics {
    #[must_use]
    pub const fn total_records(&self) -> usize {
        self.analysis_records + self.quality_records + self.video_records
    }

    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn db_size_mb(&self) -> f64 {
        self.db_size_bytes as f64 / 1024.0 / 1024.0
    }

    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn db_size_gb(&self) -> f64 {
        self.db_size_bytes as f64 / 1024.0 / 1024.0 / 1024.0
    }

    #[must_use]
    pub fn stale_records(&self) -> i64 {
        self.algorithm_version_distribution
            .iter()
            .filter(|(&v, _)| v < self.current_algorithm_version)
            .map(|(_, &count)| count)
            .sum()
    }
}

pub const CACHE_SIZE_LIMIT_BYTES: u64 = 85 * 1024 * 1024 * 1024; // 85 GB

fn open_pg_client() -> Result<Client> {
    crate::database::open_pg_client()
}

/// 🏷️ File Signature for robust change detection
#[derive(Debug, Clone, PartialEq)]
struct FileSignature {
    mtime: i64,
    ctime: i64,
    btime: i64,
    atime: i64,
    size: i64,
}

impl FileSignature {
    pub fn from_path(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path)?;
        let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);

        let mtime = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .try_into()
            .unwrap_or(i64::MAX);

        #[cfg(unix)]
        let ctime = metadata.ctime_nsec();
        #[cfg(windows)]
        use std::os::windows::fs::MetadataExt;
        #[cfg(windows)]
        let ctime = metadata.last_write_time() as i64;
        #[cfg(not(any(unix, windows)))]
        let ctime = mtime;

        let btime = metadata.created().map_or(ctime, |t| {
            t.duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos().try_into().unwrap_or(ctime))
                .unwrap_or(ctime)
        });

        let atime = metadata.accessed().map_or(mtime, |t| {
            t.duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos().try_into().unwrap_or(mtime))
                .unwrap_or(mtime)
        });

        Ok(Self {
            mtime,
            ctime,
            btime,
            atime,
            size,
        })
    }
}

pub struct AnalysisCache {
    // Connection is opened per-operation now for better reliability in concurrent environments
    // but the struct holds the connstr implicitly via pg_connstr()
}

impl AnalysisCache {
    pub fn new() -> Result<Self> {
        let mut client = open_pg_client()?;
        Self::init_schema(&mut client)?;
        Ok(Self {})
    }

    fn init_schema(client: &mut Client) -> Result<()> {
        let schema_sql = include_str!("analysis_cache_pg.sql");
        client
            .batch_execute(schema_sql)
            .context("Failed to initialize Postgres cache schema")?;

        // Initialize metadata if empty
        let current_ver: Option<i32> = client
            .query_opt(
                "SELECT value FROM cache_metadata WHERE key = 'schema_version'",
                &[],
            )?
            .map(|row| row.get(0));

        if current_ver.is_none() {
            client.execute(
                "INSERT INTO cache_metadata (key, value) VALUES ('schema_version', $1)",
                &[&CACHE_SCHEMA_VERSION],
            )?;
        }

        Self::invalidate_old_algorithm_entries(client)?;
        Ok(())
    }

    fn invalidate_old_algorithm_entries(client: &mut Client) -> Result<()> {
        let tables = ["analysis_records", "quality_records", "video_records"];
        let mut total_invalidated = 0;
        let current_version = cache_algorithm_version();

        for table in &tables {
            let count: i64 = client
                .execute(
                    &format!("DELETE FROM {table} WHERE algorithm_version < $1"),
                    &[&current_version],
                )
                .map(|n| n as i64)?;

            total_invalidated += count;
        }

        if total_invalidated > 0 {
            info!(
                "🔄 [Cache] Invalidated {} entries due to algorithm version upgrade",
                total_invalidated
            );

            client.execute(
                "DELETE FROM path_index WHERE content_hash NOT IN (
                    SELECT content_hash FROM analysis_records 
                    UNION SELECT content_hash FROM quality_records 
                    UNION SELECT content_hash FROM video_records
                )",
                &[],
            )?;
        }

        Ok(())
    }

    pub fn default_local() -> Result<Self> {
        Self::new()
    }

    pub fn get_analysis(&self, path: &Path) -> Result<Option<ImageAnalysis>> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        // 1. Path Index
        let row = client.query_opt(
            "SELECT r.analysis_data, r.algorithm_version, r.data_checksum, p.ctime, p.btime FROM path_index p 
             JOIN analysis_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = $1 AND p.mtime = $2 AND p.file_size = $3",
            &[&path_str.to_string(), &sig.mtime, &sig.size],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(1);
            if algorithm_version >= cache_algorithm_version() {
                let cached_ctime: i64 = row.get(3);
                let cached_btime: i64 = row.get(4);

                if (cached_ctime == 0 || cached_ctime == sig.ctime)
                    && (cached_btime == 0 || cached_btime == sig.btime)
                {
                    let data: Vec<u8> = row.get(0);
                    if let Some(stored_checksum) = row.get::<_, Option<i64>>(2) {
                        if calculate_checksum(&data) != (stored_checksum as u32) {
                            warn!(
                                "⚠️  [Cache] Checksum mismatch for {}. Invalidating.",
                                path.display()
                            );
                            return Ok(None);
                        }
                    }

                    let mut analysis: ImageAnalysis = rmp_serde::from_slice(&data)
                        .context("Failed to unpack cached analysis data (path hit)")?;
                    analysis.file_path = path.display().to_string();
                    debug!("🚀 [Cache] HIT (Path) - {}", path.display());
                    return Ok(Some(analysis));
                }
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let row = client.query_opt(
            "SELECT analysis_data, algorithm_version, data_checksum FROM analysis_records WHERE content_hash = $1",
            &[&content_hash.as_bytes().as_slice()],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(1);
            if algorithm_version >= cache_algorithm_version() {
                let data: Vec<u8> = row.get(0);
                if let Some(stored_checksum) = row.get::<_, Option<i64>>(2) {
                    if calculate_checksum(&data) != (stored_checksum as u32) {
                        warn!(
                            "⚠️  [Cache] Checksum mismatch for {}. Invalidating.",
                            path.display()
                        );
                        return Ok(None);
                    }
                }

                let mut analysis: ImageAnalysis = rmp_serde::from_slice(&data)
                    .context("Failed to unpack cached analysis data (hash hit)")?;

                // Backfill path index
                client.execute(
                    "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
                     VALUES ($1, $2, $3, $4, $5, $6, $7)
                     ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = EXCLUDED.ctime, btime = EXCLUDED.btime",
                    &[&path_str.to_string(), &content_hash.as_bytes().as_slice(), &sig.mtime, &sig.size, &sig.atime, &sig.ctime, &sig.btime],
                )?;

                analysis.file_path = path.display().to_string();
                debug!("💎 [Cache] HIT (Hash) - {}", path.display());
                return Ok(Some(analysis));
            }
        }

        Ok(None)
    }

    pub fn get_quality_analysis(&self, path: &Path) -> Result<Option<ImageQualityAnalysis>> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        // 1. Path Index
        let row = client.query_opt(
            "SELECT r.analysis_data, r.data_checksum, p.ctime, p.btime FROM path_index p 
             JOIN quality_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = $1 AND p.mtime = $2 AND p.file_size = $3",
            &[&path_str.to_string(), &sig.mtime, &sig.size],
        )?;

        if let Some(row) = row {
            let cached_ctime: i64 = row.get(2);
            let cached_btime: i64 = row.get(3);

            if (cached_ctime == 0 || cached_ctime == sig.ctime)
                && (cached_btime == 0 || cached_btime == sig.btime)
            {
                let data: Vec<u8> = row.get(0);
                if let Some(stored_checksum) = row.get::<_, Option<i64>>(1) {
                    if calculate_checksum(&data) != (stored_checksum as u32) {
                        warn!("⚠️  [Cache] Quality checksum mismatch (Path).");
                        return Ok(None);
                    }
                }

                let analysis: ImageQualityAnalysis = rmp_serde::from_slice(&data)
                    .context("Failed to unpack cached quality data (path hit)")?;
                debug!("📊 [Cache] Quality HIT (Path) - {}", path.display());
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let row = client.query_opt(
            "SELECT analysis_data, data_checksum FROM quality_records WHERE content_hash = $1",
            &[&content_hash.as_bytes().as_slice()],
        )?;

        if let Some(row) = row {
            let data: Vec<u8> = row.get(0);
            if let Some(stored_checksum) = row.get::<_, Option<i64>>(1) {
                if calculate_checksum(&data) != (stored_checksum as u32) {
                    warn!("⚠️  [Cache] Quality checksum mismatch (Hash).");
                    return Ok(None);
                }
            }

            let analysis: ImageQualityAnalysis = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached quality data (hash hit)")?;

            // Backfill path index
            client.execute(
                "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = EXCLUDED.ctime, btime = EXCLUDED.btime",
                &[&path_str.to_string(), &content_hash.as_bytes().as_slice(), &sig.mtime, &sig.size, &sig.atime, &sig.ctime, &sig.btime],
            )?;

            debug!("📊 [Cache] Quality HIT (Hash) - {}", path.display());
            return Ok(Some(analysis));
        }

        Ok(None)
    }

    pub fn store_analysis(&self, path: &Path, analysis: &ImageAnalysis) -> Result<()> {
        if analysis.analysis_error.is_some() {
            return Ok(());
        }

        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        let content_hash = calculate_blake3(path)?;
        let content_fingerprint = calculate_content_fingerprint(path)?;
        let packed_data = rmp_serde::to_vec(analysis).context("Failed to pack analysis data")?;
        let checksum = calculate_checksum(&packed_data);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        let mut tx = client.transaction()?;
        tx.execute(
            "INSERT INTO analysis_records (content_hash, file_size, analysis_data, created_at, algorithm_version, content_fingerprint_hash, data_checksum)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (content_hash) DO UPDATE SET file_size = EXCLUDED.file_size, analysis_data = EXCLUDED.analysis_data, created_at = EXCLUDED.created_at, algorithm_version = EXCLUDED.algorithm_version, content_fingerprint_hash = EXCLUDED.content_fingerprint_hash, data_checksum = EXCLUDED.data_checksum",
            &[&content_hash.as_bytes().as_slice(), &sig.size, &packed_data, &now, &cache_algorithm_version(), &content_fingerprint.as_slice(), &(i64::from(checksum))],
        )?;
        tx.execute(
            "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = EXCLUDED.ctime, btime = EXCLUDED.btime",
            &[&path_str.to_string(), &content_hash.as_bytes().as_slice(), &sig.mtime, &sig.size, &sig.atime, &sig.ctime, &sig.btime],
        )?;
        tx.commit()?;

        debug!("💾 [Cache] Stored analysis for {}", path.display());
        Ok(())
    }

    pub fn store_quality_analysis(
        &self,
        path: &Path,
        analysis: &ImageQualityAnalysis,
    ) -> Result<()> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        let content_hash = calculate_blake3(path)?;
        let content_fingerprint = calculate_content_fingerprint(path)?;
        let packed_data = rmp_serde::to_vec(analysis).context("Failed to pack quality data")?;
        let checksum = calculate_checksum(&packed_data);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        let mut tx = client.transaction()?;
        tx.execute(
            "INSERT INTO quality_records (content_hash, file_size, analysis_data, created_at, algorithm_version, content_fingerprint_hash, data_checksum)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (content_hash) DO UPDATE SET file_size = EXCLUDED.file_size, analysis_data = EXCLUDED.analysis_data, created_at = EXCLUDED.created_at, algorithm_version = EXCLUDED.algorithm_version, content_fingerprint_hash = EXCLUDED.content_fingerprint_hash, data_checksum = EXCLUDED.data_checksum",
            &[&content_hash.as_bytes().as_slice(), &sig.size, &packed_data, &now, &cache_algorithm_version(), &content_fingerprint.as_slice(), &(i64::from(checksum))],
        )?;
        tx.execute(
            "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = EXCLUDED.ctime, btime = EXCLUDED.btime",
            &[&path_str.to_string(), &content_hash.as_bytes().as_slice(), &sig.mtime, &sig.size, &sig.atime, &sig.ctime, &sig.btime],
        )?;
        tx.commit()?;

        debug!("💾 [Cache] Stored quality analysis for {}", path.display());
        Ok(())
    }

    pub fn get_video_analysis(&self, path: &Path) -> Result<Option<VideoDetectionResult>> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        // 1. Path Index
        let row = client.query_opt(
            "SELECT r.analysis_data, r.data_checksum, p.ctime, p.btime FROM path_index p 
             JOIN video_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = $1 AND p.mtime = $2 AND p.file_size = $3",
            &[&path_str.to_string(), &sig.mtime, &sig.size],
        )?;

        if let Some(row) = row {
            let cached_ctime: i64 = row.get(2);
            let cached_btime: i64 = row.get(3);

            if (cached_ctime == 0 || cached_ctime == sig.ctime)
                && (cached_btime == 0 || cached_btime == sig.btime)
            {
                let data: Vec<u8> = row.get(0);
                if let Some(stored_checksum) = row.get::<_, Option<i64>>(1) {
                    if calculate_checksum(&data) != (stored_checksum as u32) {
                        warn!("⚠️  [Cache] Video checksum mismatch (Path).");
                        return Ok(None);
                    }
                }

                let analysis: VideoDetectionResult = rmp_serde::from_slice(&data)
                    .context("Failed to unpack cached video data (path hit)")?;
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let row = client.query_opt(
            "SELECT analysis_data, data_checksum FROM video_records WHERE content_hash = $1",
            &[&content_hash.as_bytes().as_slice()],
        )?;

        if let Some(row) = row {
            let data: Vec<u8> = row.get(0);
            if let Some(stored_checksum) = row.get::<_, Option<i64>>(1) {
                if calculate_checksum(&data) != (stored_checksum as u32) {
                    warn!("⚠️  [Cache] Video checksum mismatch (Hash).");
                    return Ok(None);
                }
            }

            let analysis: VideoDetectionResult = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached video data (hash hit)")?;

            // Backfill path index
            client.execute(
                "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = EXCLUDED.ctime, btime = EXCLUDED.btime",
                &[&path_str.to_string(), &content_hash.as_bytes().as_slice(), &sig.mtime, &sig.size, &sig.atime, &sig.ctime, &sig.btime],
            )?;

            return Ok(Some(analysis));
        }

        Ok(None)
    }

    pub fn store_video_analysis(&self, path: &Path, analysis: &VideoDetectionResult) -> Result<()> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        let content_hash = calculate_blake3(path)?;
        let content_fingerprint = calculate_content_fingerprint(path)?;
        let packed_data = rmp_serde::to_vec(analysis).context("Failed to pack video data")?;
        let checksum = calculate_checksum(&packed_data);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        let mut tx = client.transaction()?;
        tx.execute(
            "INSERT INTO video_records (content_hash, file_size, analysis_data, created_at, algorithm_version, content_fingerprint_hash, data_checksum)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (content_hash) DO UPDATE SET file_size = EXCLUDED.file_size, analysis_data = EXCLUDED.analysis_data, created_at = EXCLUDED.created_at, algorithm_version = EXCLUDED.algorithm_version, content_fingerprint_hash = EXCLUDED.content_fingerprint_hash, data_checksum = EXCLUDED.data_checksum",
            &[&content_hash.as_bytes().as_slice(), &sig.size, &packed_data, &now, &cache_algorithm_version(), &content_fingerprint.as_slice(), &(i64::from(checksum))],
        )?;
        tx.execute(
            "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = EXCLUDED.ctime, btime = EXCLUDED.btime",
            &[&path_str.to_string(), &content_hash.as_bytes().as_slice(), &sig.mtime, &sig.size, &sig.atime, &sig.ctime, &sig.btime],
        )?;
        tx.commit()?;

        debug!("💾 [Cache] Stored video analysis for {}", path.display());
        Ok(())
    }

    pub fn cleanup_old_records(&self, max_age_secs: i64) -> Result<usize> {
        let mut client = open_pg_client()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let threshold = now - max_age_secs;

        let removed = client.execute(
            "DELETE FROM analysis_records WHERE created_at < $1",
            &[&threshold],
        )? as usize;

        if removed > 0 {
            info!("🧹 [Cache] Pruned {} old records", removed);
        }
        Ok(removed)
    }

    pub fn get_statistics(&self) -> Result<CacheStatistics> {
        let mut client = open_pg_client()?;

        let analysis_count: i64 = client
            .query_one("SELECT COUNT(*) FROM analysis_records", &[])?
            .get(0);
        let quality_count: i64 = client
            .query_one("SELECT COUNT(*) FROM quality_records", &[])?
            .get(0);
        let video_count: i64 = client
            .query_one("SELECT COUNT(*) FROM video_records", &[])?
            .get(0);
        let path_index_count: i64 = client
            .query_one("SELECT COUNT(*) FROM path_index", &[])?
            .get(0);

        let mut version_dist = std::collections::HashMap::new();
        for table in &["analysis_records", "quality_records", "video_records"] {
            let rows = client.query(
                &format!(
                    "SELECT algorithm_version, COUNT(*) FROM {table} GROUP BY algorithm_version"
                ),
                &[],
            )?;
            for row in rows {
                let v: i32 = row.get(0);
                let c: i64 = row.get(1);
                *version_dist.entry(v).or_insert(0) += c;
            }
        }

        let schema_version: i32 = client
            .query_one(
                "SELECT value FROM cache_metadata WHERE key = 'schema_version'",
                &[],
            )?
            .get(0);

        Ok(CacheStatistics {
            db_size_bytes: 0, // In Postgres, tracking actual disk size is complex per-table
            analysis_records: analysis_count as usize,
            quality_records: quality_count as usize,
            video_records: video_count as usize,
            path_index_entries: path_index_count as usize,
            schema_version,
            algorithm_version_distribution: version_dist,
            current_algorithm_version: cache_algorithm_version(),
        })
    }

    pub fn enforce_size_limit(&self) -> Result<()> {
        // Size enforcement in shared Postgres is handled differently (usually by policy or quota)
        // or we can implement a row-count based pruning here if needed.
        // For now, we rely on cleanup_old_records.
        Ok(())
    }
}

fn calculate_blake3(path: &Path) -> Result<blake3::Hash> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    #[allow(clippy::large_stack_arrays)]
    let mut buffer = [0u8; 65536];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(hasher.finalize())
}

fn calculate_content_fingerprint(path: &Path) -> Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    #[allow(clippy::large_stack_arrays)]
    let mut buffer = [0u8; 65536];
    let bytes_read = file.read(&mut buffer)?;
    hasher.update(&buffer[..bytes_read]);
    Ok(*hasher.finalize().as_bytes())
}

fn calculate_checksum(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize()
}
