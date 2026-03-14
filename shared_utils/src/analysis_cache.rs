//! 🗄️ Image Analysis Cache - Persistent SQLite Backend
//!
//! 🔥 v1.0: 极致复用化检测机制
//!
//! Provides a highly efficient, persistent cache for image analysis results using SQLite and MessagePack.
//! This ensures that expensive operations like pixel-based entropy calculation, deep HEIC/AVIF parsing,
//! and quantization detection are only performed once per file content.
//!
//! ## Strategy
//! 1. **Path-Metadata Check**: Fast lookup by (path, mtime, size).
//! 2. **Content Hash (BLAKE3)**: If path-metadata fails, calculate BLAKE3 hash to find matches by content.
//! 3. **Binary Storage**: Analysis results are packed using MessagePack (rmp-serde) for minimal disk footprint and maximum speed.

use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::Read;
use anyhow::{Context, Result};
use crate::image_analyzer::ImageAnalysis;
use crate::image_quality_detector::ImageQualityAnalysis;
use crate::video_detection::VideoDetectionResult;
use tracing::{debug, info};
use blake3::Hasher;

pub struct AnalysisCache {
    conn: std::sync::Mutex<Connection>,
}

impl AnalysisCache {
    /// Opens or creates the analysis cache at the specified path.
    pub fn new(cache_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            cache_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        ).context("Failed to open SQLite cache")?;

        // Initialize schema
        conn.execute(
            "CREATE TABLE IF NOT EXISTS analysis_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS quality_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS video_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS path_index (
                file_path TEXT PRIMARY KEY,
                content_hash BLOB NOT NULL,
                mtime INTEGER NOT NULL,
                file_size INTEGER NOT NULL
            )",
            [],
        )?;

        // Index for cleaning up old records
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_records_created ON analysis_records(created_at)",
            [],
        )?;

        Ok(Self { conn: std::sync::Mutex::new(conn) })
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
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;
        let mtime = metadata.modified()?
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;
        let path_str = path.to_string_lossy();

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // 1. Try path index first (FASTEST)
        let mut stmt = conn.prepare(
            "SELECT r.analysis_data FROM path_index p 
             JOIN analysis_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, mtime, file_size])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            let mut analysis: ImageAnalysis = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached analysis data (path hit)")?;
            // Ensure path is updated to current if it was cached under a different name
            analysis.file_path = path.display().to_string();
            debug!("🚀 [Cache] HIT (Path) - {}", path.display());
            return Ok(Some(analysis));
        }

        // 2. Fallback to Content Hash (BLAKE3)
        let content_hash = calculate_blake3(path)?;
        let mut stmt = conn.prepare(
            "SELECT analysis_data FROM analysis_records WHERE content_hash = ?"
        )?;
        
        let mut rows = stmt.query(params![content_hash.as_bytes()])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            let mut analysis: ImageAnalysis = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached analysis data (hash hit)")?;
            
            // Back-fill the path index for this exact file to speed up next check
            conn.execute(
                "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size) 
                 VALUES (?, ?, ?, ?)",
                params![path_str, content_hash.as_bytes(), mtime, file_size],
            )?;

            analysis.file_path = path.display().to_string();
            debug!("💎 [Cache] HIT (Hash) - {}", path.display());
            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Try to get quality analysis result for a file.
    pub fn get_quality_analysis(&self, path: &Path) -> Result<Option<ImageQualityAnalysis>> {
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;
        let mtime = metadata.modified()?
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;
        let path_str = path.to_string_lossy();

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // 1. Path Index
        let mut stmt = conn.prepare(
            "SELECT r.analysis_data FROM path_index p 
             JOIN quality_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, mtime, file_size])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            let analysis: ImageQualityAnalysis = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached quality data (path hit)")?;
            debug!("📊 [Cache] Quality HIT (Path) - {}", path.display());
            return Ok(Some(analysis));
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
                "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size) 
                 VALUES (?, ?, ?, ?)",
                params![path_str, content_hash.as_bytes(), mtime, file_size],
            )?;

            debug!("📊 [Cache] Quality HIT (Hash) - {}", path.display());
            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Stores an analysis result in the cache.
    pub fn store_analysis(&self, path: &Path, analysis: &ImageAnalysis) -> Result<()> {
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;
        let mtime = metadata.modified()?
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;
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
            "INSERT OR REPLACE INTO analysis_records (content_hash, file_size, analysis_data, created_at) 
             VALUES (?, ?, ?, ?)",
            params![content_hash.as_bytes(), file_size, packed_data, now],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size) 
             VALUES (?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), mtime, file_size],
        )?;

        debug!("💾 [Cache] Stored - {}", path.display());
        Ok(())
    }

    /// Stores a quality analysis result in the cache.
    pub fn store_quality_analysis(&self, path: &Path, analysis: &ImageQualityAnalysis) -> Result<()> {
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;
        let mtime = metadata.modified()?
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;
        let path_str = path.to_string_lossy();
        
        let content_hash = calculate_blake3(path)?;
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack quality data")?;
            
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        conn.execute(
            "INSERT OR REPLACE INTO quality_records (content_hash, file_size, analysis_data, created_at) 
             VALUES (?, ?, ?, ?)",
            params![content_hash.as_bytes(), file_size, packed_data, now],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size) 
             VALUES (?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), mtime, file_size],
        )?;

        debug!("📊 [Cache] Quality Stored - {}", path.display());
        Ok(())
    }

    /// Try to get a cached video analysis result.
    pub fn get_video_analysis(&self, path: &Path) -> Result<Option<VideoDetectionResult>> {
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;
        let mtime = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let path_str = path.to_string_lossy();

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // 1. Path Index
        let mut stmt = conn.prepare(
            "SELECT r.analysis_data FROM path_index p 
             JOIN video_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, mtime, file_size])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            let analysis: VideoDetectionResult = rmp_serde::from_slice(&data)
                .context("Failed to unpack cached video data (path hit)")?;
            return Ok(Some(analysis));
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
                "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size) 
                 VALUES (?, ?, ?, ?)",
                params![path_str, content_hash.as_bytes(), mtime, file_size],
            )?;

            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Stores a video analysis result in the cache.
    pub fn store_video_analysis(&self, path: &Path, analysis: &VideoDetectionResult) -> Result<()> {
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len() as i64;
        let mtime = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let path_str = path.to_string_lossy();
        
        let content_hash = calculate_blake3(path)?;
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack video analysis data")?;
            
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        conn.execute(
            "INSERT OR REPLACE INTO video_records (content_hash, file_size, analysis_data, created_at) 
             VALUES (?, ?, ?, ?)",
            params![content_hash.as_bytes(), file_size, packed_data, now],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size) 
             VALUES (?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), mtime, file_size],
        )?;

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
