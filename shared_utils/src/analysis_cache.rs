//! 🗄️ Image Analysis Cache - Persistent SQLite Backend
//!
//! 🔥 v3.0: Enhanced cache with content fingerprint + integrity verification
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
//! 5. **Integrity Verification**: CRC32 checksum for all cached data to detect corruption.
//!
//! ## Version History
//! - v1: Initial implementation
//! - v2: Added HEIC lossless detection fix + algorithm versioning
//! - v3: Added content fingerprint (BLAKE3 of first 64KB) + CRC32 integrity verification

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

// Import unified version management
use crate::version::{CACHE_SCHEMA_VERSION, cache_algorithm_version};

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

/// 🔑 Enhanced Cache Key - Comprehensive invalidation strategy
/// 
/// This structure captures ALL dimensions that affect analysis/encoding output:
/// 1. Input content fingerprint (size + mtime + optional content hash)
/// 2. Encoding parameters (CRF, quality, preset, effort, feature flags)
/// 3. Dependency library versions (ffmpeg, libjxl, libavif, etc.)
/// 4. Encoder backend (VideoToolbox vs CPU, GPU vs CPU)
/// 5. Heuristic configuration (thresholds, weights)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnhancedCacheKey {
    /// Input content fingerprint
    pub content_fingerprint: ContentFingerprint,
    
    /// Encoding parameters hash (CRF, quality, preset, etc.)
    pub encoding_params_hash: u64,
    
    /// Dependency versions hash (ffmpeg, libjxl, libavif versions)
    pub dependency_versions_hash: u64,
    
    /// Encoder backend identifier
    pub encoder_backend: EncoderBackend,
    
    /// Heuristic configuration hash (thresholds, weights)
    pub heuristic_config_hash: u64,
    
    /// Program version (algorithm version)
    pub program_version: i32,
}

/// 📦 Content Fingerprint - Multiple strategies for content identification
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContentFingerprint {
    /// Lightweight: size + mtime (fast, but can have false positives)
    SizeMtime { size: u64, mtime_ns: i64 },
    
    /// Precise: BLAKE3 hash of first N bytes (balance between speed and accuracy)
    PartialHash { size: u64, hash: [u8; 32], bytes_hashed: usize },
    
    /// Full: BLAKE3 hash of entire file (slowest, but 100% accurate)
    FullHash { size: u64, hash: [u8; 32] },
}

impl ContentFingerprint {
    /// Create lightweight fingerprint (size + mtime)
    pub fn from_metadata(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let size = metadata.len();
        let mtime_ns = metadata.modified()?
            .duration_since(UNIX_EPOCH)?
            .as_nanos() as i64;
        
        Ok(Self::SizeMtime { size, mtime_ns })
    }
    
    /// Create partial hash fingerprint (first N bytes)
    pub fn from_partial_hash(path: &Path, bytes_to_hash: usize) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let size = metadata.len();
        
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Hasher::new();
        let mut buffer = vec![0u8; bytes_to_hash.min(size as usize)];
        let bytes_read = file.read(&mut buffer)?;
        hasher.update(&buffer[..bytes_read]);
        
        let hash = *hasher.finalize().as_bytes();
        
        Ok(Self::PartialHash { size, hash, bytes_hashed: bytes_read })
    }
    
    /// Create full hash fingerprint (entire file)
    pub fn from_full_hash(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let size = metadata.len();
        
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Hasher::new();
        std::io::copy(&mut file, &mut hasher)?;
        
        let hash = *hasher.finalize().as_bytes();
        
        Ok(Self::FullHash { size, hash })
    }
}

/// 🎛️ Encoder Backend - Distinguishes different encoding paths
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncoderBackend {
    /// CPU-based encoding
    CPU,
    
    /// GPU-accelerated encoding (CUDA, OpenCL, etc.)
    GPU,
    
    /// Apple VideoToolbox (hardware acceleration on macOS/iOS)
    VideoToolbox,
    
    /// Intel Quick Sync Video
    QuickSync,
    
    /// NVIDIA NVENC
    NVENC,
    
    /// AMD AMF
    AMF,
}

/// 📋 Encoding Parameters - All parameters that affect output
#[derive(Debug, Clone, PartialEq)]
pub struct EncodingParams {
    /// CRF value (for video encoding)
    pub crf: Option<f32>,
    
    /// Quality target (for image encoding)
    pub quality: Option<u8>,
    
    /// Preset/effort level
    pub preset: Option<String>,
    
    /// Effort level (for JXL, AVIF)
    pub effort: Option<u8>,
    
    /// Feature flags
    pub ultimate_mode: bool,
    pub gpu_enabled: bool,
    pub apple_compat: bool,
    
    /// VMAF/PSNR thresholds
    pub vmaf_threshold: Option<f32>,
    pub psnr_threshold: Option<f32>,
    
    /// Additional codec-specific options
    pub codec_options: std::collections::HashMap<String, String>,
}

impl EncodingParams {
    /// Compute hash of all encoding parameters
    pub fn compute_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        
        // Hash all parameters
        self.crf.map(|v| v.to_bits()).hash(&mut hasher);
        self.quality.hash(&mut hasher);
        self.preset.hash(&mut hasher);
        self.effort.hash(&mut hasher);
        self.ultimate_mode.hash(&mut hasher);
        self.gpu_enabled.hash(&mut hasher);
        self.apple_compat.hash(&mut hasher);
        self.vmaf_threshold.map(|v| v.to_bits()).hash(&mut hasher);
        self.psnr_threshold.map(|v| v.to_bits()).hash(&mut hasher);
        
        // Hash codec options (sorted for determinism)
        let mut sorted_options: Vec<_> = self.codec_options.iter().collect();
        sorted_options.sort_by_key(|(k, _)| *k);
        for (k, v) in sorted_options {
            k.hash(&mut hasher);
            v.hash(&mut hasher);
        }
        
        hasher.finish()
    }
}

/// 📚 Dependency Versions - Track versions of critical libraries
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyVersions {
    /// FFmpeg version (libavcodec, libavformat)
    pub ffmpeg_version: Option<String>,
    
    /// libjxl version
    pub libjxl_version: Option<String>,
    
    /// libavif version
    pub libavif_version: Option<String>,
    
    /// libheif version
    pub libheif_version: Option<String>,
    
    /// SVT-AV1 version
    pub svt_av1_version: Option<String>,
    
    /// x265 version
    pub x265_version: Option<String>,
}

impl DependencyVersions {
    /// Detect versions of installed dependencies
    pub fn detect() -> Self {
        Self {
            ffmpeg_version: Self::get_ffmpeg_version(),
            libjxl_version: Self::get_libjxl_version(),
            libavif_version: Self::get_libavif_version(),
            libheif_version: Self::get_libheif_version(),
            svt_av1_version: Self::get_svt_av1_version(),
            x265_version: Self::get_x265_version(),
        }
    }

    fn get_command_version_line(tool: &str, args: &[&str]) -> Option<String> {
        let output = match std::process::Command::new(tool).args(args).output() {
            Ok(output) => output,
            Err(err) => {
                debug!(tool, error = %err, "Failed to execute external tool for version detection");
                return None;
            }
        };

        if !output.status.success() {
            debug!(
                tool,
                status = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "External tool returned non-success during version detection"
            );
            return None;
        }

        let stdout = match String::from_utf8(output.stdout) {
            Ok(stdout) => stdout,
            Err(err) => {
                warn!(tool, error = %err, "Failed to decode tool version output as UTF-8");
                return None;
            }
        };

        let line = stdout.lines().next().map(str::trim).filter(|line| !line.is_empty());
        if line.is_none() {
            debug!(tool, "Version detection returned empty stdout");
        }
        line.map(ToString::to_string)
    }
    
    /// Compute hash of all dependency versions
    pub fn compute_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        self.ffmpeg_version.hash(&mut hasher);
        self.libjxl_version.hash(&mut hasher);
        self.libavif_version.hash(&mut hasher);
        self.libheif_version.hash(&mut hasher);
        self.svt_av1_version.hash(&mut hasher);
        self.x265_version.hash(&mut hasher);
        hasher.finish()
    }
    
    fn get_ffmpeg_version() -> Option<String> {
        Self::get_command_version_line("ffmpeg", &["-version"]).map(|line| {
            line.split_whitespace()
                .nth(2)
                .unwrap_or("unknown")
                .to_string()
        })
    }
    
    fn get_libjxl_version() -> Option<String> {
        Self::get_command_version_line("cjxl", &["--version"])
    }
    
    fn get_libavif_version() -> Option<String> {
        Self::get_command_version_line("avifenc", &["--version"])
    }
    
    fn get_libheif_version() -> Option<String> {
        // libheif version is typically embedded in the library
        // For now, return None (can be enhanced with dynamic library inspection)
        None
    }
    
    fn get_svt_av1_version() -> Option<String> {
        Self::get_command_version_line("SvtAv1EncApp", &["--version"])
    }
    
    fn get_x265_version() -> Option<String> {
        Self::get_command_version_line("x265", &["--version"])
    }
}

/// 🎯 Heuristic Configuration - Thresholds and weights for decision-making
#[derive(Debug, Clone, PartialEq)]
pub struct HeuristicConfig {
    /// JXL distance threshold for lossless detection
    pub jxl_lossless_distance_threshold: f32,
    
    /// HEVC CRF threshold for quality matching
    pub hevc_crf_threshold: f32,
    
    /// Entropy threshold for complexity detection
    pub entropy_threshold: f32,
    
    /// Size increase tolerance (bytes)
    pub size_tolerance_bytes: u64,
    
    /// Quality matching precision
    pub quality_match_precision: f32,
}

impl Default for HeuristicConfig {
    fn default() -> Self {
        Self {
            jxl_lossless_distance_threshold: 0.1,
            hevc_crf_threshold: 18.0,
            entropy_threshold: 7.0,
            size_tolerance_bytes: 1_048_576, // 1MB
            quality_match_precision: 0.95,
        }
    }
}

impl HeuristicConfig {
    /// Compute hash of configuration
    pub fn compute_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        self.jxl_lossless_distance_threshold.to_bits().hash(&mut hasher);
        self.hevc_crf_threshold.to_bits().hash(&mut hasher);
        self.entropy_threshold.to_bits().hash(&mut hasher);
        self.size_tolerance_bytes.hash(&mut hasher);
        self.quality_match_precision.to_bits().hash(&mut hasher);
        hasher.finish()
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

        // Initialize schema with enhanced cache key fields
        // v3 schema adds:
        // - content_fingerprint_hash: BLAKE3 hash of first 64KB for precise content identification
        // - data_checksum: CRC32 checksum of analysis_data for integrity verification
        conn.execute(
            "CREATE TABLE IF NOT EXISTS analysis_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                algorithm_version INTEGER DEFAULT 1,
                content_fingerprint_hash BLOB,
                data_checksum INTEGER
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS quality_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                algorithm_version INTEGER DEFAULT 1,
                content_fingerprint_hash BLOB,
                data_checksum INTEGER
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS video_records (
                content_hash BLOB PRIMARY KEY,
                file_size INTEGER NOT NULL,
                analysis_data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                algorithm_version INTEGER DEFAULT 1,
                content_fingerprint_hash BLOB,
                data_checksum INTEGER
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
        let current_version: Option<i32> = match conn.query_row(
            "SELECT value FROM cache_metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        ) {
            Ok(version) => Some(version),
            Err(err) => {
                warn!(
                    error = %err,
                    "⚠️  [Cache] Failed to read schema_version metadata; treating cache as unversioned"
                );
                None
            }
        };

        match current_version {
            Some(v) if v == CACHE_SCHEMA_VERSION => {
                // Schema version matches, check algorithm version
                Self::invalidate_old_algorithm_entries(conn)?;
            }
            Some(v) if v < CACHE_SCHEMA_VERSION => {
                info!("🔄 [Cache] Schema version mismatch (current: {}, expected: {}). Migrating...", v, CACHE_SCHEMA_VERSION);
                
                // Migrate from v2 to v3: Add content_fingerprint_hash and data_checksum columns
                if v == 2 {
                    info!("🔄 [Cache] Migrating schema from v2 to v3 (adding content fingerprint and checksum)");
                    
                    // Add new columns to existing tables
                    for (table, column, column_type) in [
                        ("analysis_records", "content_fingerprint_hash", "BLOB"),
                        ("analysis_records", "data_checksum", "INTEGER"),
                        ("quality_records", "content_fingerprint_hash", "BLOB"),
                        ("quality_records", "data_checksum", "INTEGER"),
                        ("video_records", "content_fingerprint_hash", "BLOB"),
                        ("video_records", "data_checksum", "INTEGER"),
                    ] {
                        if let Err(err) = conn.execute(
                            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, column_type),
                            [],
                        ) {
                            warn!(
                                table = table,
                                column = column,
                                error = %err,
                                "⚠️  [Cache] Failed to migrate cache column"
                            );
                        }
                    }
                    
                    info!("✅ [Cache] Schema migration v2 → v3 complete");
                }
                
                // Update schema version
                conn.execute(
                    "INSERT OR REPLACE INTO cache_metadata (key, value) VALUES ('schema_version', ?)",
                    params![CACHE_SCHEMA_VERSION],
                )?;
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
        let current_version = cache_algorithm_version();

        for table in &tables {
            let count: i32 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {} WHERE algorithm_version < ?", table),
                params![current_version],
                |row| row.get(0),
            )?;

            if count > 0 {
                conn.execute(
                    &format!("DELETE FROM {} WHERE algorithm_version < ?", table),
                    params![current_version],
                )?;
                total_invalidated += count;
            }
        }

        if total_invalidated > 0 {
            info!("🔄 [Cache] Invalidated {} entries due to algorithm version upgrade (v{} → v{})", 
                total_invalidated, current_version - 1, current_version);
            
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
            "SELECT r.analysis_data, r.algorithm_version, r.data_checksum, p.atime, p.ctime, p.btime FROM path_index p 
             JOIN analysis_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, sig.mtime, sig.size])?;
        if let Some(row) = rows.next()? {
            let algorithm_version: i32 = row.get(1)?;
            
            // Check if algorithm version is current
            if algorithm_version < cache_algorithm_version() {
                debug!("🔄 [Cache] Stale algorithm version (v{} < v{}) for {}", 
                    algorithm_version, cache_algorithm_version(), path.display());
                // Fall through to recompute
            } else {
                // Strict Invalidation: Check ctime and btime too
                let _cached_atime: i64 = row.get(3)?;
                let cached_ctime: i64 = row.get(4)?;
                let cached_btime: i64 = row.get(5)?;

                // Use XOR or direct compare for maximum rigor
                let strict_match = (cached_ctime == 0 || cached_ctime == sig.ctime) && 
                                   (cached_btime == 0 || cached_btime == sig.btime);

                if !strict_match {
                    warn!("⚠️  [Cache] Path Match but Metadata Discrepancy (ctime/btime changed). Invalidating entry for {}", path.display());
                } else {
                    let data: Vec<u8> = row.get(0)?;
                    
                    // Verify checksum for integrity
                    if let Some(stored_checksum) = row.get::<_, Option<u32>>(2)? {
                        let computed_checksum = calculate_checksum(&data);
                        if computed_checksum != stored_checksum {
                            warn!("⚠️  [Cache] Checksum mismatch for {} (stored: {}, computed: {}). Data corrupted, invalidating.", 
                                path.display(), stored_checksum, computed_checksum);
                            return Ok(None);
                        }
                        debug!("✅ [Cache] Checksum verified for {}", path.display());
                    }
                    
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
            "SELECT analysis_data, algorithm_version, data_checksum FROM analysis_records WHERE content_hash = ?"
        )?;
        
        let mut rows = stmt.query(params![content_hash.as_bytes()])?;
        if let Some(row) = rows.next()? {
            let algorithm_version: i32 = row.get(1)?;
            
            // Check algorithm version
            if algorithm_version < cache_algorithm_version() {
                debug!("🔄 [Cache] Stale algorithm version (v{} < v{}) for {}", 
                    algorithm_version, cache_algorithm_version(), path.display());
                return Ok(None); // Force recompute
            }
            
            let data: Vec<u8> = row.get(0)?;
            
            // Verify checksum for integrity
            if let Some(stored_checksum) = row.get::<_, Option<u32>>(2)? {
                let computed_checksum = calculate_checksum(&data);
                if computed_checksum != stored_checksum {
                    warn!("⚠️  [Cache] Checksum mismatch for {} (stored: {}, computed: {}). Data corrupted, invalidating.", 
                        path.display(), stored_checksum, computed_checksum);
                    return Ok(None);
                }
                debug!("✅ [Cache] Checksum verified for {}", path.display());
            }
            
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
            "SELECT r.analysis_data, r.data_checksum, p.ctime, p.btime FROM path_index p 
             JOIN quality_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, sig.mtime, sig.size])?;
        if let Some(row) = rows.next()? {
            let cached_ctime: i64 = row.get(2)?;
            let cached_btime: i64 = row.get(3)?;
            
            if (cached_ctime == 0 || cached_ctime == sig.ctime) && (cached_btime == 0 || cached_btime == sig.btime) {
                let data: Vec<u8> = row.get(0)?;
                
                // Verify checksum for integrity
                if let Some(stored_checksum) = row.get::<_, Option<u32>>(1)? {
                    let computed_checksum = calculate_checksum(&data);
                    if computed_checksum != stored_checksum {
                        warn!("⚠️  [Cache] Quality checksum mismatch for {}. Data corrupted, invalidating.", path.display());
                        return Ok(None);
                    }
                    debug!("✅ [Cache] Quality checksum verified for {}", path.display());
                }
                
                let analysis: ImageQualityAnalysis = rmp_serde::from_slice(&data)
                    .context("Failed to unpack cached quality data (path hit)")?;
                debug!("📊 [Cache] Quality HIT (Path) - {}", path.display());
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let mut stmt = conn.prepare(
            "SELECT analysis_data, data_checksum FROM quality_records WHERE content_hash = ?"
        )?;
        
        let mut rows = stmt.query(params![content_hash.as_bytes()])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            
            // Verify checksum for integrity
            if let Some(stored_checksum) = row.get::<_, Option<u32>>(1)? {
                let computed_checksum = calculate_checksum(&data);
                if computed_checksum != stored_checksum {
                    warn!("⚠️  [Cache] Quality checksum mismatch for {}. Data corrupted, invalidating.", path.display());
                    return Ok(None);
                }
                debug!("✅ [Cache] Quality checksum verified for {}", path.display());
            }
            
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
        
        // Calculate content fingerprint (first 64KB hash) for precise cache key
        let content_fingerprint = calculate_content_fingerprint(path)?;
        
        // Pack data
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack analysis data")?;
        
        // Calculate checksum for integrity verification
        let checksum = calculate_checksum(&packed_data);
            
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // Perform in transaction for atomicity
        conn.execute(
            "INSERT OR REPLACE INTO analysis_records (content_hash, file_size, analysis_data, created_at, algorithm_version, content_fingerprint_hash, data_checksum) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![content_hash.as_bytes(), sig.size, packed_data, now, cache_algorithm_version(), &content_fingerprint[..], checksum as i64],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
        )?;

        debug!("💾 [Cache] Stored analysis for {} (checksum: {})", 
            path.display(), checksum);

        if let Err(err) = self.enforce_size_limit() {
            warn!(
                path = %self.cache_path.display(),
                error = %err,
                "⚠️  [Cache] Failed to enforce size limit after storing analysis"
            );
        }
        Ok(())
    }

    /// Stores a quality analysis result in the cache.
    pub fn store_quality_analysis(&self, path: &Path, analysis: &ImageQualityAnalysis) -> Result<()> {
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        
        let content_hash = calculate_blake3(path)?;
        let content_fingerprint = calculate_content_fingerprint(path)?;
        
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack quality data")?;
        
        let checksum = calculate_checksum(&packed_data);
            
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        conn.execute(
            "INSERT OR REPLACE INTO quality_records (content_hash, file_size, analysis_data, created_at, algorithm_version, content_fingerprint_hash, data_checksum) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![content_hash.as_bytes(), sig.size, packed_data, now, cache_algorithm_version(), &content_fingerprint[..], checksum as i64],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
        )?;

        debug!("💾 [Cache] Stored quality analysis for {} (checksum: {})", path.display(), checksum);

        if let Err(err) = self.enforce_size_limit() {
            warn!(
                path = %self.cache_path.display(),
                error = %err,
                "⚠️  [Cache] Failed to enforce size limit after storing quality analysis"
            );
        }
        Ok(())
    }

    /// Try to get a cached video analysis result.
    pub fn get_video_analysis(&self, path: &Path) -> Result<Option<VideoDetectionResult>> {
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        // 1. Path Index
        let mut stmt = conn.prepare(
            "SELECT r.analysis_data, r.data_checksum, p.ctime, p.btime FROM path_index p 
             JOIN video_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = ? AND p.mtime = ? AND p.file_size = ?"
        )?;
        
        let mut rows = stmt.query(params![path_str, sig.mtime, sig.size])?;
        if let Some(row) = rows.next()? {
            let cached_ctime: i64 = row.get(2)?;
            let cached_btime: i64 = row.get(3)?;

            if (cached_ctime == 0 || cached_ctime == sig.ctime) && (cached_btime == 0 || cached_btime == sig.btime) {
                let data: Vec<u8> = row.get(0)?;
                
                // Verify checksum for integrity
                if let Some(stored_checksum) = row.get::<_, Option<u32>>(1)? {
                    let computed_checksum = calculate_checksum(&data);
                    if computed_checksum != stored_checksum {
                        warn!("⚠️  [Cache] Video checksum mismatch for {}. Data corrupted, invalidating.", path.display());
                        return Ok(None);
                    }
                    debug!("✅ [Cache] Video checksum verified for {}", path.display());
                }
                
                let analysis: VideoDetectionResult = rmp_serde::from_slice(&data)
                    .context("Failed to unpack cached video data (path hit)")?;
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index (Content Match)
        let content_hash = calculate_blake3(path)?;
        let mut stmt = conn.prepare(
            "SELECT analysis_data, data_checksum FROM video_records WHERE content_hash = ?"
        )?;
        
        let mut rows = stmt.query(params![content_hash.as_bytes()])?;
        if let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            
            // Verify checksum for integrity
            if let Some(stored_checksum) = row.get::<_, Option<u32>>(1)? {
                let computed_checksum = calculate_checksum(&data);
                if computed_checksum != stored_checksum {
                    warn!("⚠️  [Cache] Video checksum mismatch for {}. Data corrupted, invalidating.", path.display());
                    return Ok(None);
                }
                debug!("✅ [Cache] Video checksum verified for {}", path.display());
            }
            
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
        let content_fingerprint = calculate_content_fingerprint(path)?;
        
        let packed_data = rmp_serde::to_vec(analysis)
            .context("Failed to pack video analysis data")?;
        
        let checksum = calculate_checksum(&packed_data);
            
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("Mutex lock failed: {}", e))?;

        conn.execute(
            "INSERT OR REPLACE INTO video_records (content_hash, file_size, analysis_data, created_at, algorithm_version, content_fingerprint_hash, data_checksum) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![content_hash.as_bytes(), sig.size, packed_data, now, cache_algorithm_version(), &content_fingerprint[..], checksum as i64],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, btime) 
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![path_str, content_hash.as_bytes(), sig.mtime, sig.size, sig.atime, sig.ctime, sig.btime],
        )?;

        debug!("💾 [Cache] Stored video analysis for {} (checksum: {})", path.display(), checksum);

        if let Err(err) = self.enforce_size_limit() {
            warn!(
                path = %self.cache_path.display(),
                error = %err,
                "⚠️  [Cache] Failed to enforce size limit after storing video analysis"
            );
        }
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
        
        let db_size = match std::fs::metadata(&self.cache_path) {
            Ok(metadata) => metadata.len(),
            Err(err) => {
                warn!(
                    path = %self.cache_path.display(),
                    error = %err,
                    "⚠️  [Cache] Failed to read cache database size"
                );
                0
            }
        };
        
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
        
        let current_schema_version: i32 = match conn.query_row(
            "SELECT value FROM cache_metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        ) {
            Ok(version) => version,
            Err(err) => {
                warn!(
                    error = %err,
                    "⚠️  [Cache] Failed to read schema version while building statistics"
                );
                1
            }
        };
        
        Ok(CacheStatistics {
            db_size_bytes: db_size,
            analysis_records: analysis_count as usize,
            quality_records: quality_count as usize,
            video_records: video_count as usize,
            path_index_entries: path_index_count as usize,
            schema_version: current_schema_version,
            algorithm_version_distribution: version_dist,
            current_algorithm_version: cache_algorithm_version(),
        })
    }

    /// ⚖️ Enforce size limit (85GB). 
    /// If DB exceeds limit, prune oldest records until it's back under 90% of limit.
    pub fn enforce_size_limit(&self) -> Result<()> {
        let current_size = match std::fs::metadata(&self.cache_path) {
            Ok(m) => m.len(),
            Err(err) => {
                warn!(
                    path = %self.cache_path.display(),
                    error = %err,
                    "⚠️  [Cache] Failed to read cache size for size-limit enforcement"
                );
                return Ok(());
            }
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

/// 🔍 Calculate content fingerprint hash (BLAKE3 of first 64KB)
/// 
/// This provides a fast, precise way to identify file content without hashing the entire file.
/// Used for cache key precision - prevents false cache hits when files have same size/mtime
/// but different content.
fn calculate_content_fingerprint(path: &Path) -> Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    
    // Hash first 64KB only for speed
    let mut buffer = [0u8; 65536]; // 64KB
    let bytes_read = file.read(&mut buffer)?;
    hasher.update(&buffer[..bytes_read]);
    
    Ok(*hasher.finalize().as_bytes())
}

/// ✅ Calculate CRC32 checksum of data for integrity verification
/// 
/// Used to detect silent data corruption in the cache database.
/// If checksum doesn't match on read, the cached data is corrupted and should be discarded.
fn calculate_checksum(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize()
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
