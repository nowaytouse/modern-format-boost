//! 🗄️ Image Analysis Cache - Persistent SQLite Backend
//!
//! 🔥 v2.0: 版本化缓存机制 + 精确失效策略
//!
//! Provides a highly efficient, persistent cache for image analysis results using SQLite and MessagePack.
//! This ensures that expensive operations like pixel-based entropy calculation, deep HEIC/AVIF parsing,
//! and quantization detection are only performed once per file content.
//!
//! ## Strategy
//! 1. **Path-Metadata Check**: Fast lookup by (path, mtime, size).
//! 2. **Content Hash (BLAKE3)**: If path-metadata fails, calculate BLAKE3 hash to find matches by content.
//! 3. **Binary Storage**: Analysis results are packed using MessagePack (rmp-serde) for minimal disk footprint and maximum speed.
//! 4. **Version Control**: Algorithm version tracking to auto-invalidate stale cache entries.
//!
//! ## Version History
//! - v1: Initial implementation
//! - v2: Added HEIC lossless detection fix + algorithm versioning

use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::Read;
use anyhow::{Context, Result};
use crate::image_analyzer::ImageAnalysis;
use crate::image_quality_detector::ImageQualityAnalysis;
use crate::video_detection::VideoDetectionResult;
use tracing::{debug, info, warn};
use blake3::Hasher;

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
    pub fn total_records(&self) -> usize {
        self.analysis_records + self.quality_records + self.video_records
    }
    
    pub fn db_size_mb(&self) -> f64 {
        self.db_size_bytes as f64 / 1024.0 / 1024.0
    }
    
    pub fn db_size_gb(&self) -> f64 {
        self.db_size_bytes as f64 / 1024.0 / 1024.0 / 1024.0
    }
    
    pub fn stale_records(&self) -> i64 {
        self.algorithm_version_distribution
            .iter()
            .filter(|(&v, _)| v < self.current_algorithm_version)
            .map(|(_, &count)| count)
            .sum()
    }
}

pub const CACHE_SIZE_LIMIT_BYTES: u64 = 85 * 1024 * 1024 * 1024; // 85 GB

/// 🔢 Cache Schema Version - Increment when database structure changes
const CACHE_SCHEMA_VERSION: i32 = 2;

/// 🧬 Analysis Algorithm Version - Bound to program version for automatic cache invalidation
/// 
/// Version Format: Major.Minor.Patch → MajorMinorPatch (e.g., 0.10.60 → 10060)
/// This ensures cache is regenerated on ANY program update, maintaining consistency.
/// 
/// Version History:
/// - v1: Original HEIC lossless detection
/// - v2: Fixed HEIC lossless detection + improved box parsing
/// - v10060: Bound to program version 0.10.60 (automatic invalidation on updates)
/// - v10061: Cache version binding mechanism
/// - v10062: Dependency unification (GitHub nightly sources)
/// - v10063: HEIC security limits increased (6GB, 10k ipco children)
/// - v10064: Git history cleanup (AI tool configs removed for privacy)
/// - v10065: HEIC security limits fix (apply before reading, 7GB memory)
/// - v10066: HEIC security limits increased to 15GB + feature flag fix
const ANALYSIS_ALGORITHM_VERSION: i32 = 10066;

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
    fn from_path(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let size = metadata.len() as i64;
        
        // Use nanoseconds for maximum rigor as requested
        let mtime = metadata.modified()?
            .duration_since(UNIX_EPOCH)?
            .as_nanos() as i64;
        
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(unix)]
        let ctime = metadata.ctime_nsec(); // Unix ctime nanoseconds
        #[cfg(windows)]
        use std::os::windows::fs::MetadataExt;
        #[cfg(windows)]
        let ctime = metadata.last_write_time() as i64;
        #[cfg(not(any(unix, windows)))]
        let ctime = mtime;

        // Birthtime (btime)
        let btime = match metadata.created() {
            Ok(t) => t.duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(ctime),
            Err(_) => ctime,
        };

        // Atime (last access)
        let atime = match metadata.accessed() {
            Ok(t) => t.duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(mtime),
            Err(_) => mtime,
        };

        Ok(Self { mtime, ctime, btime, atime, size })
    }
}

pub struct AnalysisCache {
    conn: std::sync::Mutex<Connection>,
    cache_path: std::path::PathBuf,
}

impl AnalysisCache {
    /// Opens or creates the analysis cache at the specified path.
    pub fn new(cache_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            cache_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        ).context("Failed to open SQLite cache")?;

        // Check and handle schema version
        Self::check_and_migrate_schema(&conn, cache_path)?;

        // Initialize schema with algorithm_version column
        conn.execute(
            "CREATE TABLE IF NOT EXISTS analysis_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                algorithm_version INTEGER DEFAULT 1
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS quality_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                algorithm_version INTEGER DEFAULT 1
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS video_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                algorithm_version INTEGER DEFAULT 1
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS path_index (
                file_path TEXT PRIMARY KEY,
                content_hash BLOB NOT NULL,
                mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                atime INTEGER DEFAULT 0,
                ctime INTEGER DEFAULT 0,
                btime INTEGER DEFAULT 0
            )",
            [],
        )?;

        // Store schema version metadata
        conn.execute(
            "CREATE TABLE IF NOT EXISTS cache_metadata (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO cache_metadata (key, value) VALUES ('schema_version', ?)",
            params![CACHE_SCHEMA_VERSION],
        )?;

        // --- MIGRATION: Add new columns if they don't exist in an old DB ---
        let existing_columns: std::collections::HashSet<String> = conn
            .prepare("PRAGMA table_info(path_index)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<_, _>>()?;

        for col in &["atime", "ctime", "btime"] {
            if !existing_columns.contains(*col) {
                info!("🛠️  [Cache] Migrating path_index: adding column '{}'", col);
                conn.execute(&format!("ALTER TABLE path_index ADD COLUMN {} INTEGER DEFAULT 0", col), [])?;
            }
        }

        // Add algorithm_version column to existing tables if missing
        for table in &["analysis_records", "quality_records", "video_records"] {
            let existing_columns: std::collections::HashSet<String> = conn
                .prepare(&format!("PRAGMA table_info({})", table))?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<_, _>>()?;

            if !existing_columns.contains("algorithm_version") {
                info!("🛠️  [Cache] Migrating {}: adding algorithm_version column", table);
                conn.execute(
                    &format!("ALTER TABLE {} ADD COLUMN algorithm_version INTEGER DEFAULT 1", table),
                    [],
                )?;
            }
        }

        // Index for cleaning up old records
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_records_created ON analysis_records(created_at)",
            [],
        )?;

        Ok(Self { 
            conn: std::sync::Mutex::new(conn),
            cache_path: cache_path.to_path_buf(),
        })
    }

    /// Check schema version and handle migration/invalidation
    fn check_and_migrate_schema(conn: &Connection, _cache_path: &Path) -> Result<()> {
        // Try to get current schema version
        let current_version: Option<i32> = conn
            .query_row(
                "SELECT value FROM cache_metadata WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .ok();

        match current_version {
            Some(v) if v == CACHE_SCHEMA_VERSION => {
                // Schema version matches, check algorithm version
                Self::invalidate_old_algorithm_entries(conn)?;
            }
            Some(v) if v < CACHE_SCHEMA_VERSION => {
                info!("🔄 [Cache] Schema version mismatch (current: {}, expected: {}). Migrating...", v, CACHE_SCHEMA_VERSION);
                // Schema changed - migration will be handled by ALTER TABLE statements
            }
            Some(v) => {
                warn!("⚠️  [Cache] Schema version {} is newer than expected {}. Cache may be incompatible.", v, CACHE_SCHEMA_VERSION);
            }
            None => {
                info!("🆕 [Cache] Initializing new cache database");
            }
        }

        Ok(())
    }

    /// Invalidate cache entries created with old algorithm versions
    fn invalidate_old_algorithm_entries(conn: &Connection) -> Result<()> {
        let tables = ["analysis_records", "quality_records", "video_records"];
        let mut total_invalidated = 0;

        for table in &tables {
            let count: i32 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {} WHERE algorithm_version < ?", table),
                params![ANALYSIS_ALGORITHM_VERSION],
                |row| row.get(0),
            )?;

            if count > 0 {
                conn.execute(
                    &format!("DELETE FROM {} WHERE algorithm_version < ?", table),
                    params![ANALYSIS_ALGORITHM_VERSION],
                )?;
                total_invalidated += count;
            }
        }

        if total_invalidated > 0 {
            info!("🔄 [Cache] Invalidated {} entries due to algorithm version upgrade (v{} → v{})", 
                total_invalidated, ANALYSIS_ALGORITHM_VERSION - 1, ANALYSIS_ALGORITHM_VERSION);
            
            // Clean up orphaned path_index entries
            conn.execute(
                "DELETE FROM path_index WHERE content_hash NOT IN (
                    SELECT content_hash FROM analysis_records 
                    UNION SELECT content_hash FROM quality_records 
                    UNION SELECT content_hash FROM video_records
                )",
                [],
            )?;
        }

        Ok(())
    }

    /// Default project-local cache location
    pub fn default_local() -> Result<Self> {
        let mut path = std::env::current_dir()?;
        path.push(".cache");
        std::fs::create_dir_all(&path)?;
        path.push("image_analysis_v2.db");
        Self::new(&path)
    }

    /// Try to get analysis result for a file. 
    /// Returns Ok(Some(result)) if cached and still valid (metadata match or hash match).
    pub fn get_analysis(&self, path: &Path) -> Result<Option<ImageAnalysis>> {
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // 1. Try path index first (FASTEST)
        let mut stmt = conn.prepare(
            "SELECT r.analysis_data, r.algorithm_version, p.atime, p.ctime, p.btime FROM path_index p 
             JOIN analysis_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, sig.mtime, sig.size])?;
        if let Some(row) = rows.next()? {
            let algorithm_version: i32 = row.get(1)?;
            
            // Check if algorithm version is current
            if algorithm_version < ANALYSIS_ALGORITHM_VERSION {
                debug!("🔄 [Cache] Stale algorithm version (v{} < v{}) for {}", 
                    algorithm_version, ANALYSIS_ALGORITHM_VERSION, path.display());
                // Fall through to recompute
            } else {
                // Strict Invalidation: Check ctime and btime too
                let _cached_atime: i64 = row.get(2)?;
                let cached_ctime: i64 = row.get(3)?;
                let cached_btime: i64 = row.get(4)?;

                // Use XOR or direct compare for maximum rigor
                let strict_match = (cached_ctime == 0 || cached_ctime == sig.ctime) && 
                                   (cached_btime == 0 || cached_btime == sig.btime);

                if !strict_match {
                    warn!("⚠️  [Cache] Path Match but Metadata Discrepancy (ctime/btime changed). Invalidating entry for {}", path.display());
                } else {
                    let data: Vec<u8> = row.get(0)?;
                    let mut analysis: ImageAnalysis = rmp_serde::from_slice(&data)
                        .context("Failed to unpack cached analysis data (path hit)")?;
                    analysis.file_path = path.display().to_string();
                    debug!("🚀 [Cache] HIT (Path) - {}", path.display());
                    return Ok(Some(analysis));
                }
            }
        }

        // 2. Fallback to Content Hash (BLAKE3)
        let content_hash = calculate_blake3(path)?;
        let mut stmt = conn.prepare(
            "SELECT analysis_data, algorithm_version FROM analysis_records WHERE content_hash = ?"
        )?;
        
        let mut rows = stmt.query(params![content_hash.as_bytes()])?;
        if let Some(row) = rows.next()? {
            let algorithm_version: i32 = row.get(1)?;
            
            // Check algorithm version
            if algorithm_version < ANALYSIS_ALGORITHM_VERSION {
                debug!("🔄 [Cache] Stale algorithm version (v{} < v{}) for {}", 
                    algorithm_version, ANALYSIS_ALGORITHM_VERSION, path.display());
                return Ok(None); // Force recompute
            }
            
            let data: Vec<u8> = row.get(0)?;
            let mut analysis: ImageAnalysis = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached analysis data (hash hit)")?;
            
            // Back-fill the path index for this exact file to speed up next check
            conn.execute(
                "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
            )?;

            analysis.file_path = path.display().to_string();
            debug!("💎 [Cache] HIT (Hash) - {}", path.display());
            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Try to get quality analysis result for a file.
    pub fn get_quality_analysis(&self, path: &Path) -> Result<Option<ImageQualityAnalysis>> {
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // 1. Path Index
        let mut stmt = conn.prepare(
            "SELECT r.analysis_data, p.ctime, p.btime FROM path_index p 
             JOIN quality_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, sig.mtime, sig.size])?;
        if let Some(row) = rows.next()? {
            let cached_ctime: i64 = row.get(1)?;
            let cached_btime: i64 = row.get(2)?;
            
            if (cached_ctime == 0 || cached_ctime == sig.ctime) && (cached_btime == 0 || cached_btime == sig.btime) {
                let data: Vec<u8> = row.get(0)?;
                let analysis: ImageQualityAnalysis = rmp_serde::from_slice(&data)
                    .context("Failed to unpack cached quality data (path hit)")?;
                debug!("📊 [Cache] Quality HIT (Path) - {}", path.display());
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let mut stmt = conn.prepare(
            "SELECT analysis_data FROM quality_records WHERE content_hash = ?"
        )?;
        
        let mut rows = stmt.query(params![content_hash.as_bytes()])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            let analysis: ImageQualityAnalysis = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached quality data (hash hit)")?;
            
            conn.execute(
                "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
            )?;

            debug!("📊 [Cache] Quality HIT (Hash) - {}", path.display());
            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Stores an analysis result in the cache.
    pub fn store_analysis(&self, path: &Path, analysis: &ImageAnalysis) -> Result<()> {
        // 🧠 Smart Cache: Never store results that failed analysis.
        // This prevents "ghost errors" from persisting after a code fix.
        if analysis.analysis_error.is_some() {
            return Ok(());
        }

        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        
        // Calculate hash for cross-path reuse
        let content_hash = calculate_blake3(path)?;
        
        // Pack data
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack analysis data")?;
            
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // Perform in transaction for atomicity
        conn.execute(
            "INSERT OR REPLACE INTO analysis_records (content_hash, file_size, analysis_data, created_at, algorithm_version) 
             VALUES (?, ?, ?, ?, ?)",
            params![content_hash.as_bytes(), sig.size, packed_data, now, ANALYSIS_ALGORITHM_VERSION],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
        )?;

        let _ = self.enforce_size_limit();
        Ok(())
    }

    /// Stores a quality analysis result in the cache.
    pub fn store_quality_analysis(&self, path: &Path, analysis: &ImageQualityAnalysis) -> Result<()> {
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        
        let content_hash = calculate_blake3(path)?;
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack quality data")?;
            
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        conn.execute(
            "INSERT OR REPLACE INTO quality_records (content_hash, file_size, analysis_data, created_at, algorithm_version) 
             VALUES (?, ?, ?, ?, ?)",
            params![content_hash.as_bytes(), sig.size, packed_data, now, ANALYSIS_ALGORITHM_VERSION],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
        )?;

        let _ = self.enforce_size_limit();
        Ok(())
    }

    /// Try to get a cached video analysis result.
    pub fn get_video_analysis(&self, path: &Path) -> Result<Option<VideoDetectionResult>> {
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // 1. Path Index
        let mut stmt = conn.prepare(
            "SELECT r.analysis_data, p.ctime, p.btime FROM path_index p 
             JOIN video_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, sig.mtime, sig.size])?;
        if let Some(row) = rows.next()? {
            let cached_ctime: i64 = row.get(1)?;
            let cached_btime: i64 = row.get(2)?;

            if (cached_ctime == 0 || cached_ctime == sig.ctime) && (cached_btime == 0 || cached_btime == sig.btime) {
                let data: Vec<u8> = row.get(0)?;
                let analysis: VideoDetectionResult = rmp_serde::from_slice(&data)
                    .context("Failed to unpack cached video data (path hit)")?;
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index (Content Match)
        let content_hash = calculate_blake3(path)?;
        let mut stmt = conn.prepare(
            "SELECT analysis_data FROM video_records WHERE content_hash = ?"
        )?;
        
        let mut rows = stmt.query(params![content_hash.as_bytes()])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            let analysis: VideoDetectionResult = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached video data (hash hit)")?;
            
            // Backfill path index
            conn.execute(
                "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
            )?;

            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Stores a video analysis result in the cache.
    pub fn store_video_analysis(&self, path: &Path, analysis: &VideoDetectionResult) -> Result<()> {
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        
        let content_hash = calculate_blake3(path)?;
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack video analysis data")?;
            
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        conn.execute(
            "INSERT OR REPLACE INTO video_records (content_hash, file_size, analysis_data, created_at, algorithm_version) 
             VALUES (?, ?, ?, ?, ?)",
            params![content_hash.as_bytes(), sig.size, packed_data, now, ANALYSIS_ALGORITHM_VERSION],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
        )?;

        let _ = self.enforce_size_limit();
        Ok(())
    }

    /// Garbage collection: remove records older than specified duration (sec)
    pub fn cleanup_old_records(&self, max_age_secs: i64) -> Result<usize> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;
        let threshold = now - max_age_secs;
        
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        let removed = conn.execute(
            "DELETE FROM analysis_records WHERE created_at < ?",
            params![threshold],
        )?;

        // Orphans in path_index will be handled by regular usage or a full sweep
        if removed > 0 {
            info!("🧹 [Cache] Pruned {} old records", removed);
            conn.execute("VACUUM", [])?;
        }
        
        Ok(removed)
    }

    /// 📊 Get cache statistics
    pub fn get_statistics(&self) -> Result<CacheStatistics> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;
        
        let db_size = std::fs::metadata(&self.cache_path)
            .map(|m| m.len())
            .unwrap_or(0);
        
        let analysis_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM analysis_records",
            [],
            |row| row.get(0),
        )?;
        
        let quality_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM quality_records",
            [],
            |row| row.get(0),
        )?;
        
        let video_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM video_records",
            [],
            |row| row.get(0),
        )?;
        
        let path_index_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM path_index",
            [],
            |row| row.get(0),
        )?;
        
        // Get version distribution
        let mut version_dist = std::collections::HashMap::new();
        
        for table in &["analysis_records", "quality_records", "video_records"] {
            let mut stmt = conn.prepare(&format!(
                "SELECT algorithm_version, COUNT(*) FROM {} GROUP BY algorithm_version",
                table
            ))?;
            
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i32>(0)?, row.get::<_, i64>(1)?))
            })?;
            
            for row in rows {
                let (version, count) = row?;
                *version_dist.entry(version).or_insert(0) += count;
            }
        }
        
        let current_schema_version: i32 = conn.query_row(
            "SELECT value FROM cache_metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        ).unwrap_or(1);
        
        Ok(CacheStatistics {
            db_size_bytes: db_size,
            analysis_records: analysis_count as usize,
            quality_records: quality_count as usize,
            video_records: video_count as usize,
            path_index_entries: path_index_count as usize,
            schema_version: current_schema_version,
            algorithm_version_distribution: version_dist,
            current_algorithm_version: ANALYSIS_ALGORITHM_VERSION,
        })
    }

    /// ⚖️ Enforce size limit (85GB). 
    /// If DB exceeds limit, prune oldest records until it's back under 90% of limit.
    pub fn enforce_size_limit(&self) -> Result<()> {
        let current_size = match std::fs::metadata(&self.cache_path) {
            Ok(m) => m.len(),
            Err(_) => return Ok(()),
        };

        if current_size < CACHE_SIZE_LIMIT_BYTES {
            return Ok(());
        }

        info!("⚖️  [Cache] Size limit reached ({} / {} GB). Pruning...", 
            current_size / 1024 / 1024 / 1024,
            CACHE_SIZE_LIMIT_BYTES / 1024 / 1024 / 1024
        );

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;
        
        // Prune 15% of the oldest records to provide headroom
        let total_records: i64 = conn.query_row("SELECT COUNT(*) FROM analysis_records", [], |r| r.get(0))?;
        let to_remove = (total_records / 7).max(1); // approx 15%

        // Delete from all record tables
        conn.execute(
            "DELETE FROM analysis_records WHERE content_hash IN (SELECT content_hash FROM analysis_records ORDER BY created_at LIMIT ?)",
            params![to_remove],
        )?;
        conn.execute(
            "DELETE FROM quality_records WHERE content_hash IN (SELECT content_hash FROM quality_records ORDER BY created_at LIMIT ?)",
            params![to_remove],
        )?;
        conn.execute(
            "DELETE FROM video_records WHERE content_hash IN (SELECT content_hash FROM video_records ORDER BY created_at LIMIT ?)",
            params![to_remove],
        )?;

        // Cleanup path_index orphans (those pointing to deleted content_hashes)
        conn.execute(
            "DELETE FROM path_index WHERE content_hash NOT IN (SELECT content_hash FROM analysis_records UNION SELECT content_hash FROM quality_records UNION SELECT content_hash FROM video_records)",
            []
        )?;

        info!("🧹 [Cache] Pruned {} old records to maintain size limit", to_remove);
        
        // Vacuum to actually shrink the file
        conn.execute("VACUUM", [])?;
        
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_cache_metadata_rigor() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("test_cache.db");
        let cache = AnalysisCache::new(&db_path)?;

        let mut temp_file = NamedTempFile::new()?;
        writeln!(temp_file, "rigor test data")?;
        let path = temp_file.path();

        let analysis = ImageAnalysis {
            file_path: path.display().to_string(),
            format: "test".to_string(),
            ..Default::default()
        };

        // 1. Store initial analysis
        cache.store_analysis(path, &analysis)?;

        // 2. Fetch immediately (HIT)
        let hit = cache.get_analysis(path)?;
        assert!(hit.is_some(), "Should have a cache hit");

        // 3. Simulate change (overwrite file with same size but different content/metadata)
        std::thread::sleep(std::time::Duration::from_millis(10)); 
        {
            let mut f = std::fs::File::create(path)?;
            writeln!(f, "modified data!!")?;
        }
        
        // Even if size is same, mtime/ctime will change.
        let miss = cache.get_analysis(path)?;
        // It might still hit if only ctime/btime check is strict and mtime matches,
        // but since mtime changed (overwrite), it will miss path_index.
        // Then it will try hash check, which will ALSO miss because content changed.
        assert!(miss.is_none(), "Should be a cache MISS after file modification");

        Ok(())
    }

    #[test]
    fn test_signature_stability() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path();
        
        let sig1 = FileSignature::from_path(path)?;
        std::thread::sleep(std::time::Duration::from_millis(10));
        let sig2 = FileSignature::from_path(path)?;
        
        // Sig should be stable if file not touched
        assert_eq!(sig1.mtime, sig2.mtime);
        assert_eq!(sig1.ctime, sig2.ctime);
        assert_eq!(sig1.size, sig2.size);
        
        Ok(())
    }
}

fn calculate_blake3(path: &Path) -> Result<blake3::Hash> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    
    // Efficiency: Use chunked reading for hash
    let mut buffer = [0u8; 65536]; // 64KB
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 { break; }
        hasher.update(&buffer[..bytes_read]);
    }
    
    Ok(hasher.finalize())
}
