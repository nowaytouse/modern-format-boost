//! 🗄️ Image Analysis Cache - `PostgreSQL` Backend
//!
//! Hash-path SQL (M132): `algorithm_version FROM video_records` · (M133):
//! `algorithm_version FROM quality_records`
//!
//! Provides a highly efficient, persistent cache for image analysis results
//! using `PostgreSQL` and `MessagePack`. This ensures that expensive operations
//! like pixel-based entropy calculation, deep HEIC/AVIF parsing,
//! and quantization detection are only performed once per file content.

use crate::image_analyzer::ImageAnalysis;
use crate::image_quality_detector::ImageQualityAnalysis;
use crate::video_detection::Detection;
use anyhow::{Context, Result};
use blake3::Hasher;
use postgres::Client;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

// Import unified version management
use crate::version::{CACHE_SCHEMA_VERSION, cache_algorithm};

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
    pub fn db_size_mb(&self) -> f64 {
        crate::numeric_cast::u64_to_f64(self.db_size_bytes)
            / crate::numeric_cast::u64_to_f64(crate::constants::BYTES_PER_MB)
    }

    #[must_use]
    pub fn db_size_gb(&self) -> f64 {
        crate::numeric_cast::u64_to_f64(self.db_size_bytes)
            / crate::numeric_cast::u64_to_f64(crate::constants::BYTES_PER_GB)
    }

    #[must_use]
    pub fn stale_records(&self) -> i64 {
        self.algorithm_version_distribution
            .iter()
            .filter(|&(&v, _)| v < self.current_algorithm_version)
            .map(|(_, &count)| count)
            .sum()
    }
}

pub const CACHE_SIZE_LIMIT_BYTES: u64 = crate::constants::CACHE_SIZE_LIMIT_BYTES;

/// Opens a connection to the `PostgreSQL` database.
fn open_pg_client() -> Result<Client> {
    crate::database::open_pg_client()
}

/// 🏷️ File Signature for robust change detection
#[derive(Debug, Clone, PartialEq)]
struct FileSignature {
    /// Last modification time in nanoseconds since UNIX epoch.
    mtime: i64,
    /// Status change time (Unix) or last write time (Windows).
    ctime: i64,
    /// Birth/creation time in nanoseconds since UNIX epoch.
    btime: i64,
    /// Last access time in nanoseconds since UNIX epoch.
    atime: i64,
    /// File size in bytes.
    size: i64,
}

impl FileSignature {
    /// Extracts a file signature from the given path.
    fn from_path(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path)?;
        let size = i64::try_from(metadata.len())
            .with_context(|| format!("file size exceeds i64 for {}", path.display()))?;

        let mtime = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .try_into()
            .with_context(|| format!("mtime exceeds i64 nanoseconds for {}", path.display()))?;

        #[cfg(unix)]
        let ctime = metadata.ctime_nsec();
        #[cfg(windows)]
        use std::os::windows::fs::MetadataExt;
        #[cfg(windows)]
        let ctime =
            crate::numeric_cast::u64_to_i64_strict(metadata.last_write_time(), "last_write_time")
                .with_context(|| format!("ctime exceeds i64 for {}", path.display()))?;
        #[cfg(not(any(unix, windows)))]
        let ctime = mtime;

        let btime = match metadata.created() {
            Ok(t) => match t.duration_since(UNIX_EPOCH) {
                Ok(d) => i64::try_from(d.as_nanos()).with_context(|| {
                    format!("birth time exceeds i64 nanoseconds for {}", path.display())
                })?,
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "analysis_cache_btime",
                        format!(
                            "{}: birth time before UNIX epoch ({err}); using ctime for cache key",
                            path.display()
                        ),
                    );
                    ctime
                }
            },
            Err(err) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "analysis_cache_btime",
                    format!(
                        "{}: birth time unavailable ({err}); using ctime for cache key",
                        path.display()
                    ),
                );
                ctime
            }
        };

        let atime = match metadata.accessed() {
            Ok(t) => match t.duration_since(UNIX_EPOCH) {
                Ok(d) => i64::try_from(d.as_nanos()).with_context(|| {
                    format!("access time exceeds i64 nanoseconds for {}", path.display())
                })?,
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "analysis_cache_atime",
                        format!(
                            "{}: access time before UNIX epoch ({err}); using mtime for cache key",
                            path.display()
                        ),
                    );
                    mtime
                }
            },
            Err(err) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "analysis_cache_atime",
                    format!(
                        "{}: access time unavailable ({err}); using mtime for cache key",
                        path.display()
                    ),
                );
                mtime
            }
        };

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

const fn image_analysis_canvas_trustworthy(analysis: &ImageAnalysis) -> bool {
    analysis.width > 0 && analysis.height > 0
}

fn image_analysis_is_positive_cache_entry(path: &Path, analysis: &ImageAnalysis) -> bool {
    if analysis.analysis_error.is_some() {
        return false;
    }
    if !image_analysis_canvas_trustworthy(analysis) {
        return false;
    }

    !crate::media_conversion_gate::probe_path_can_be_animated_or_label(path, &analysis.format)
        || analysis.is_animated
}

fn video_analysis_canvas_trustworthy(analysis: &Detection) -> bool {
    analysis.width.is_some_and(|w| w > 0) && analysis.height.is_some_and(|h| h > 0)
}

fn video_analysis_is_positive_cache_entry(path: &Path, analysis: &Detection) -> bool {
    if !video_analysis_canvas_trustworthy(analysis) {
        return false;
    }
    let animation_capable =
        crate::media_conversion_gate::probe_path_can_be_animated_or_label(path, &analysis.format);
    if !animation_capable {
        return true;
    }
    analysis.frame_count.is_some_and(|count| count > 1)
}

fn log_negative_video_cache_rejected(path: &Path, analysis: &Detection, phase: &str) {
    crate::media_conversion_gate::analysis_cache_invalidate_audit(
        "analysis_cache_negative_video_rejected",
        path,
        format!(
            "phase={phase}; frame_count={}; canvas={}x{}; format={}",
            crate::media_conversion_gate::delivery_frame_count_label_u64(
                analysis.frame_count,
                &format!("cache reject {}", path.display()),
            ),
            crate::media_conversion_gate::delivery_audit_optional_u32(analysis.width),
            crate::media_conversion_gate::delivery_audit_optional_u32(analysis.height),
            analysis.format,
        ),
    );
    tracing::debug!(
        path = %path.display(),
        format = %analysis.format,
        frame_count = ?analysis.frame_count,
        width = ?analysis.width,
        height = ?analysis.height,
        phase,
        "Ignoring non-positive video analysis cache entry"
    );
}

fn purge_path_index_for_path(client: &mut Client, path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    let removed = client.execute("DELETE FROM path_index WHERE file_path = $1", &[&path_str])?;
    tracing::debug!(
        path = %path.display(),
        removed_path_index = removed,
        "Purged path_index for cache reject"
    );
    Ok(())
}

fn purge_negative_video_cache(client: &mut Client, path: &Path, content_hash: &[u8]) -> Result<()> {
    delete_cache_record_by_hash(client, "video_records", content_hash)?;
    purge_path_index_for_path(client, path)?;
    Ok(())
}

fn purge_negative_image_cache(client: &mut Client, path: &Path, content_hash: &[u8]) -> Result<()> {
    delete_cache_record_by_hash(client, "analysis_records", content_hash)?;
    purge_path_index_for_path(client, path)?;
    Ok(())
}

fn log_negative_image_cache_rejected(path: &Path, analysis: &ImageAnalysis, phase: &str) {
    crate::media_conversion_gate::analysis_cache_invalidate_audit(
        "analysis_cache_negative_image_rejected",
        path,
        format!(
            "phase={phase}; is_animated={}; canvas={}x{}; format={}; analysis_error={}",
            analysis.is_animated,
            analysis.width,
            analysis.height,
            analysis.format,
            match analysis.analysis_error.as_deref() {
                None => "none",
                Some(err) => err,
            },
        ),
    );
    tracing::debug!(
        path = %path.display(),
        format = %analysis.format,
        is_animated = analysis.is_animated,
        width = analysis.width,
        height = analysis.height,
        analysis_error = ?analysis.analysis_error,
        phase,
        "Ignoring non-positive image analysis cache entry"
    );
}

fn cache_record_algorithm_current(version: i32, path: &Path, phase: &str) -> bool {
    let current = cache_algorithm();
    if version >= current {
        return true;
    }
    crate::media_conversion_gate::analysis_cache_invalidate_audit(
        "analysis_cache_stale_algorithm_version",
        path,
        format!("phase={phase}; cached_version={version}; current_version={current}"),
    );
    false
}

const fn quality_analysis_canvas_trustworthy(analysis: &ImageQualityAnalysis) -> bool {
    analysis.width > 0 && analysis.height > 0
}

fn quality_analysis_is_positive_cache_entry(path: &Path, analysis: &ImageQualityAnalysis) -> bool {
    if !quality_analysis_canvas_trustworthy(analysis) {
        return false;
    }
    let animation_capable =
        crate::media_conversion_gate::probe_path_can_be_animated_or_label(path, &analysis.format);
    if !animation_capable {
        return true;
    }
    analysis.is_animated && analysis.frame_count.is_some_and(|fc| fc > 1)
}

fn purge_negative_quality_cache(
    client: &mut Client,
    path: &Path,
    content_hash: &[u8],
) -> Result<()> {
    delete_cache_record_by_hash(client, "quality_records", content_hash)?;
    purge_path_index_for_path(client, path)?;
    Ok(())
}

fn log_negative_quality_cache_rejected(path: &Path, analysis: &ImageQualityAnalysis, phase: &str) {
    crate::media_conversion_gate::analysis_cache_invalidate_audit(
        "analysis_cache_negative_quality_rejected",
        path,
        format!(
            "phase={phase}; is_animated={}; frame_count={}; canvas={}x{}; format={}",
            analysis.is_animated,
            crate::media_conversion_gate::delivery_frame_count_label(
                analysis.frame_count,
                &format!("cache reject {}", path.display()),
            ),
            analysis.width,
            analysis.height,
            analysis.format,
        ),
    );
    tracing::debug!(
        path = %path.display(),
        format = %analysis.format,
        is_animated = analysis.is_animated,
        frame_count = ?analysis.frame_count,
        width = analysis.width,
        height = analysis.height,
        phase,
        "Ignoring non-positive quality analysis cache entry"
    );
}

fn delete_cache_record_by_hash(
    client: &mut Client,
    table: &str,
    content_hash: &[u8],
) -> Result<()> {
    let removed = client.execute(
        &format!("DELETE FROM {table} WHERE content_hash = $1"),
        &[&content_hash],
    )?;
    tracing::debug!(
        table,
        removed,
        "Deleted non-positive cache record after rejecting cache hit"
    );
    Ok(())
}

fn purge_corrupt_cache_record(
    client: &mut Client,
    table: &str,
    path: &Path,
    content_hash: &[u8],
    reason: &str,
) -> Result<()> {
    delete_cache_record_by_hash(client, table, content_hash)?;
    purge_path_index_for_path(client, path)?;
    tracing::debug!(
        table,
        path = %path.display(),
        reason,
        "Purged corrupt cache record after cache integrity failure"
    );
    Ok(())
}

fn stored_content_fingerprint_matches_path(
    path: &Path,
    stored_fingerprint: Option<Vec<u8>>,
) -> Result<bool> {
    let Some(stored) = stored_fingerprint else {
        return Ok(false);
    };
    if stored.len() != 32 {
        return Ok(false);
    }
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&stored);
    Ok(calculate_content_fingerprint(path)? == fingerprint)
}

fn path_index_content_hash_matches_file(path: &Path, stored_content_hash: &[u8]) -> Result<bool> {
    Ok(calculate_blake3(path)?.as_bytes().as_slice() == stored_content_hash)
}

const fn cache_record_file_size_matches_path(sig: &FileSignature, stored_file_size: i64) -> bool {
    sig.size == stored_file_size
}

fn reject_cache_hit_on_record_file_size_mismatch(
    client: &mut Client,
    table: &str,
    path: &Path,
    content_hash: &[u8],
    sig: &FileSignature,
    stored_file_size: i64,
    phase: &str,
) -> Result<bool> {
    if cache_record_file_size_matches_path(sig, stored_file_size) {
        return Ok(true);
    }
    crate::media_conversion_gate::analysis_cache_invalidate_audit(
        "analysis_cache_record_file_size_mismatch",
        path,
        format!(
            "phase={phase}; table={table}; stored_file_size={stored_file_size}; live_file_size={}",
            sig.size
        ),
    );
    purge_corrupt_cache_record(
        client,
        table,
        path,
        content_hash,
        "record-file-size-mismatch",
    )?;
    Ok(false)
}

fn reject_stale_path_index_hit(
    client: &mut Client,
    path: &Path,
    stored_content_hash: &[u8],
    phase: &str,
) -> Result<bool> {
    if path_index_content_hash_matches_file(path, stored_content_hash)? {
        return Ok(true);
    }
    crate::media_conversion_gate::analysis_cache_invalidate_audit(
        "analysis_cache_path_index_stale",
        path,
        format!("phase={phase}; path_index content_hash does not match live file blake3"),
    );
    purge_path_index_for_path(client, path)?;
    Ok(false)
}

fn reject_cache_hit_on_content_fingerprint_mismatch(
    client: &mut Client,
    table: &str,
    path: &Path,
    content_hash: &[u8],
    stored_fingerprint: Option<Vec<u8>>,
    phase: &str,
) -> Result<bool> {
    if stored_content_fingerprint_matches_path(path, stored_fingerprint)? {
        return Ok(true);
    }
    crate::media_conversion_gate::analysis_cache_invalidate_audit(
        "analysis_cache_content_fingerprint_mismatch",
        path,
        format!("phase={phase}; table={table}"),
    );
    purge_corrupt_cache_record(
        client,
        table,
        path,
        content_hash,
        "content-fingerprint-mismatch",
    )?;
    Ok(false)
}

fn unpack_cached_payload<T>(
    client: &mut Client,
    table: &str,
    path: &Path,
    content_hash: &[u8],
    data: &[u8],
    phase: &str,
) -> Result<Option<T>>
where
    T: serde::de::DeserializeOwned,
{
    match rmp_serde::from_slice::<T>(data) {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            crate::media_conversion_gate::analysis_cache_invalidate_audit(
                "analysis_cache_payload_decode_failed",
                path,
                format!("phase={phase}; table={table}; err={err}"),
            );
            purge_corrupt_cache_record(client, table, path, content_hash, "payload-decode")?;
            Ok(None)
        }
    }
}

fn purge_orphan_path_index_entries(client: &mut Client) -> Result<()> {
    client.execute(
        "DELETE FROM path_index WHERE content_hash NOT IN (
            SELECT content_hash FROM analysis_records 
            UNION SELECT content_hash FROM quality_records 
            UNION SELECT content_hash FROM video_records
        )",
        &[],
    )?;
    Ok(())
}

impl AnalysisCache {
    /// Create a new analysis cache with default settings.
    ///
    /// # Errors
    /// Returns an error if the database connection fails or the schema is
    /// invalid.
    pub fn new() -> Result<Self> {
        let mut client = open_pg_client()?;
        Self::init_schema(&mut client)?;
        Ok(Self {})
    }

    /// Initializes the database schema if it doesn't exist.
    fn init_schema(client: &mut Client) -> Result<()> {
        let schema_sql = include_str!("../../../dev/src/config/sql/analysis_cache_pg.sql");
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

        match current_ver {
            None => {
                client.execute(
                    "INSERT INTO cache_metadata (key, value) VALUES ('schema_version', $1)",
                    &[&CACHE_SCHEMA_VERSION],
                )?;
            }
            Some(existing) if existing != CACHE_SCHEMA_VERSION => {
                Self::reset_cache_for_schema_cutover(client, existing)?;
            }
            Some(_) => {}
        }

        Self::invalidate_old_algorithm_entries(client)?;
        Ok(())
    }

    fn reset_cache_for_schema_cutover(
        client: &mut Client,
        previous_schema_version: i32,
    ) -> Result<()> {
        crate::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
            "analysis_cache_schema_cutover_purge",
            format!(
                "previous_schema_version={previous_schema_version}; \
                 new_schema_version={CACHE_SCHEMA_VERSION}"
            ),
        );
        client
            .batch_execute(
                "TRUNCATE TABLE path_index, analysis_records, quality_records, video_records",
            )
            .context("Failed to clear old cache tables during schema cutover")?;
        client.execute(
            "UPDATE cache_metadata SET value = $1 WHERE key = 'schema_version'",
            &[&CACHE_SCHEMA_VERSION],
        )?;
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_CACHE,
            &format!(
                "Cleared cached analysis payloads due to schema cutover \
                 (v{previous_schema_version} -> v{CACHE_SCHEMA_VERSION})"
            )
        );
        Ok(())
    }

    /// Deletes entries that were created with an older version of the analysis
    /// algorithm.
    fn invalidate_old_algorithm_entries(client: &mut Client) -> Result<()> {
        let tables = ["analysis_records", "quality_records", "video_records"];
        let mut total_invalidated: i64 = 0;
        let current_version = cache_algorithm();

        for table in &tables {
            let count: i64 = client
                .execute(
                    &format!("DELETE FROM {table} WHERE algorithm_version < $1"),
                    &[&current_version],
                )
                .map(u64::cast_signed)?;

            total_invalidated = total_invalidated.saturating_add(count);
        }

        if total_invalidated > 0 {
            crate::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                "analysis_cache_algorithm_upgrade_purge",
                format!(
                    "invalidated={total_invalidated}; current_algorithm_version={current_version}"
                ),
            );
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_CACHE,
                &format!(
                    "Invalidated {total_invalidated} entries due to algorithm version upgrade"
                )
            );

            purge_orphan_path_index_entries(client)?;
        }

        Ok(())
    }

    /// Create a local cache in the current working directory.
    ///
    /// # Errors
    /// Returns an error if the cache directory cannot be created.
    pub fn default_local() -> Result<Self> {
        Self::new()
    }

    /// Retrieve the analysis results for a given image file.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    /// # Panics
    ///
    /// Panics if the database schema is corrupted or columns are missing from
    /// the `analysis_records` table.
    pub fn get_analysis(&self, path: &Path) -> Result<Option<ImageAnalysis>> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        // 1. Path Index
        let row = client.query_opt(
            "SELECT r.analysis_data, r.algorithm_version, r.data_checksum, p.ctime, p.btime, \
             p.content_hash, r.content_fingerprint_hash, r.file_size FROM path_index p 
             JOIN analysis_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = $1 AND p.mtime = $2 AND p.file_size = $3",
            &[&path_str.to_string(), &sig.mtime, &sig.size],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(1);
            if !cache_record_algorithm_current(algorithm_version, path, "image-path-hit") {
                let content_hash: Vec<u8> = row.get(5);
                purge_negative_image_cache(&mut client, path, content_hash.as_slice())?;
                return Ok(None);
            }
            let row_ctime_epoch: i64 = row.get(3);
            let row_birthtime_epoch: i64 = row.get(4);

            if (row_ctime_epoch == 0 || row_ctime_epoch == sig.ctime)
                && (row_birthtime_epoch == 0 || row_birthtime_epoch == sig.btime)
            {
                let content_hash: Vec<u8> = row.get(5);
                if !reject_stale_path_index_hit(
                    &mut client,
                    path,
                    content_hash.as_slice(),
                    "image-path-hit",
                )? {
                    return Ok(None);
                }

                let stored_file_size: i64 = row.get(7);
                if !reject_cache_hit_on_record_file_size_mismatch(
                    &mut client,
                    "analysis_records",
                    path,
                    content_hash.as_slice(),
                    &sig,
                    stored_file_size,
                    "image-path-hit",
                )? {
                    return Ok(None);
                }

                let data: Vec<u8> = row.get(0);
                // Honest validation: If checksum is missing or invalid, treat as cache miss
                // (None)
                let stored_checksum_opt = row.get::<_, Option<i64>>(2);
                let valid_checksum = stored_checksum_opt
                    .and_then(|cs| crate::numeric_cast::i64_to_u32_strict(cs, "cache_checksum"));

                let Some(checksum) = valid_checksum else {
                    crate::media_conversion_gate::analysis_cache_invalidate_audit(
                        "analysis_cache_checksum_invalid",
                        path,
                        "missing or invalid checksum; invalidating cached entry (path hit)",
                    );
                    purge_corrupt_cache_record(
                        &mut client,
                        "analysis_records",
                        path,
                        content_hash.as_slice(),
                        "checksum-invalid-path",
                    )?;
                    return Ok(None);
                };

                if calculate_checksum(&data) != checksum {
                    crate::media_conversion_gate::analysis_cache_invalidate_audit(
                        "analysis_cache_checksum_mismatch",
                        path,
                        "checksum mismatch; invalidating cached entry (path hit)",
                    );
                    purge_corrupt_cache_record(
                        &mut client,
                        "analysis_records",
                        path,
                        content_hash.as_slice(),
                        "checksum-mismatch-path",
                    )?;
                    return Ok(None);
                }

                let stored_fingerprint: Option<Vec<u8>> = row.get(6);
                if !reject_cache_hit_on_content_fingerprint_mismatch(
                    &mut client,
                    "analysis_records",
                    path,
                    content_hash.as_slice(),
                    stored_fingerprint,
                    "path-hit",
                )? {
                    return Ok(None);
                }

                let Some(mut analysis) = unpack_cached_payload::<ImageAnalysis>(
                    &mut client,
                    "analysis_records",
                    path,
                    content_hash.as_slice(),
                    &data,
                    "path-hit",
                )?
                else {
                    return Ok(None);
                };
                analysis.file_path = path.display().to_string();
                if !image_analysis_is_positive_cache_entry(path, &analysis) {
                    log_negative_image_cache_rejected(path, &analysis, "path-hit");
                    purge_negative_image_cache(&mut client, path, content_hash.as_slice())?;
                    return Ok(None);
                }
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_CACHE,
                    &format!("HIT (Path) - {}", path.display())
                );
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let row = client.query_opt(
            "SELECT analysis_data, algorithm_version, data_checksum, content_fingerprint_hash, \
             file_size FROM analysis_records WHERE content_hash = $1",
            &[&content_hash.as_bytes().as_slice()],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(1);
            if !cache_record_algorithm_current(algorithm_version, path, "image-hash-hit") {
                purge_negative_image_cache(&mut client, path, content_hash.as_bytes().as_slice())?;
                return Ok(None);
            }
            let stored_file_size: i64 = row.get(4);
            if !reject_cache_hit_on_record_file_size_mismatch(
                &mut client,
                "analysis_records",
                path,
                content_hash.as_bytes().as_slice(),
                &sig,
                stored_file_size,
                "image-hash-hit",
            )? {
                return Ok(None);
            }
            let data: Vec<u8> = row.get(0);
            let stored_checksum_opt = row.get::<_, Option<i64>>(2);
            let valid_checksum = stored_checksum_opt
                .and_then(|cs| crate::numeric_cast::i64_to_u32_strict(cs, "cache_checksum"));

            let Some(checksum) = valid_checksum else {
                crate::media_conversion_gate::analysis_cache_invalidate_audit(
                    "analysis_cache_checksum_invalid",
                    path,
                    "missing or invalid checksum; invalidating (hash hit)",
                );
                purge_corrupt_cache_record(
                    &mut client,
                    "analysis_records",
                    path,
                    content_hash.as_bytes().as_slice(),
                    "checksum-invalid-hash",
                )?;
                return Ok(None);
            };

            if calculate_checksum(&data) != checksum {
                crate::media_conversion_gate::analysis_cache_invalidate_audit(
                    "analysis_cache_checksum_mismatch",
                    path,
                    "checksum mismatch; invalidating (hash hit)",
                );
                purge_corrupt_cache_record(
                    &mut client,
                    "analysis_records",
                    path,
                    content_hash.as_bytes().as_slice(),
                    "checksum-mismatch-hash",
                )?;
                return Ok(None);
            }

            let stored_fingerprint: Option<Vec<u8>> = row.get(3);
            if !reject_cache_hit_on_content_fingerprint_mismatch(
                &mut client,
                "analysis_records",
                path,
                content_hash.as_bytes().as_slice(),
                stored_fingerprint,
                "hash-hit",
            )? {
                return Ok(None);
            }

            let Some(mut analysis) = unpack_cached_payload::<ImageAnalysis>(
                &mut client,
                "analysis_records",
                path,
                content_hash.as_bytes().as_slice(),
                &data,
                "hash-hit",
            )?
            else {
                return Ok(None);
            };
            analysis.file_path = path.display().to_string();
            if !image_analysis_is_positive_cache_entry(path, &analysis) {
                log_negative_image_cache_rejected(path, &analysis, "hash-hit");
                purge_negative_image_cache(&mut client, path, content_hash.as_bytes().as_slice())?;
                return Ok(None);
            }

            // Backfill path index
            client.execute(
                "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, \
                 btime) 
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime \
                 = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime \
                 = EXCLUDED.ctime, btime = EXCLUDED.btime",
                &[
                    &path_str.to_string(),
                    &content_hash.as_bytes().as_slice(),
                    &sig.mtime,
                    &sig.size,
                    &sig.atime,
                    &sig.ctime,
                    &sig.btime,
                ],
            )?;

            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_CACHE,
                &format!("HIT (Hash) - {}", path.display())
            );
            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Retrieve the quality analysis results for a given image file.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    /// # Panics
    ///
    /// Panics if the database schema is corrupted or columns are missing from
    /// the `quality_records` table.
    pub fn get_quality_analysis(&self, path: &Path) -> Result<Option<ImageQualityAnalysis>> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        // 1. Path Index
        let row = client.query_opt(
            "SELECT r.analysis_data, r.data_checksum, r.algorithm_version, p.ctime, p.btime, \
             p.content_hash, r.content_fingerprint_hash, r.file_size FROM path_index p 
             JOIN quality_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = $1 AND p.mtime = $2 AND p.file_size = $3",
            &[&path_str.to_string(), &sig.mtime, &sig.size],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(2);
            if !cache_record_algorithm_current(algorithm_version, path, "quality-path-hit") {
                let content_hash: Vec<u8> = row.get(5);
                purge_negative_quality_cache(&mut client, path, content_hash.as_slice())?;
                return Ok(None);
            }
            let row_ctime_epoch: i64 = row.get(3);
            let row_birthtime_epoch: i64 = row.get(4);

            if (row_ctime_epoch == 0 || row_ctime_epoch == sig.ctime)
                && (row_birthtime_epoch == 0 || row_birthtime_epoch == sig.btime)
            {
                let content_hash: Vec<u8> = row.get(5);
                if !reject_stale_path_index_hit(
                    &mut client,
                    path,
                    content_hash.as_slice(),
                    "quality-path-hit",
                )? {
                    return Ok(None);
                }

                let stored_file_size: i64 = row.get(7);
                if !reject_cache_hit_on_record_file_size_mismatch(
                    &mut client,
                    "quality_records",
                    path,
                    content_hash.as_slice(),
                    &sig,
                    stored_file_size,
                    "quality-path-hit",
                )? {
                    return Ok(None);
                }

                let data: Vec<u8> = row.get(0);
                let stored_checksum_opt = row.get::<_, Option<i64>>(1);
                let valid_checksum = stored_checksum_opt
                    .and_then(|cs| crate::numeric_cast::i64_to_u32_strict(cs, "quality_checksum"));

                let Some(checksum) = valid_checksum else {
                    crate::media_conversion_gate::analysis_cache_invalidate_audit(
                        "analysis_cache_checksum_invalid",
                        path,
                        "missing or invalid checksum; invalidating (quality path hit)",
                    );
                    purge_corrupt_cache_record(
                        &mut client,
                        "quality_records",
                        path,
                        content_hash.as_slice(),
                        "checksum-invalid-path",
                    )?;
                    return Ok(None);
                };

                if calculate_checksum(&data) != checksum {
                    crate::media_conversion_gate::analysis_cache_invalidate_audit(
                        "analysis_cache_checksum_mismatch",
                        path,
                        "checksum mismatch (quality path index)",
                    );
                    purge_corrupt_cache_record(
                        &mut client,
                        "quality_records",
                        path,
                        content_hash.as_slice(),
                        "checksum-mismatch-path",
                    )?;
                    return Ok(None);
                }

                let stored_fingerprint: Option<Vec<u8>> = row.get(6);
                if !reject_cache_hit_on_content_fingerprint_mismatch(
                    &mut client,
                    "quality_records",
                    path,
                    content_hash.as_slice(),
                    stored_fingerprint,
                    "path-hit",
                )? {
                    return Ok(None);
                }

                let Some(analysis) = unpack_cached_payload::<ImageQualityAnalysis>(
                    &mut client,
                    "quality_records",
                    path,
                    content_hash.as_slice(),
                    &data,
                    "path-hit",
                )?
                else {
                    return Ok(None);
                };
                if !quality_analysis_is_positive_cache_entry(path, &analysis) {
                    log_negative_quality_cache_rejected(path, &analysis, "path-hit");
                    purge_negative_quality_cache(&mut client, path, content_hash.as_slice())?;
                    return Ok(None);
                }
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_CACHE,
                    &format!("Quality HIT (Path) - {}", path.display())
                );
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let row = client.query_opt(
            "SELECT analysis_data, data_checksum, algorithm_version, content_fingerprint_hash, \
             file_size FROM quality_records WHERE content_hash = $1",
            &[&content_hash.as_bytes().as_slice()],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(2);
            if !cache_record_algorithm_current(algorithm_version, path, "quality-hash-hit") {
                purge_negative_quality_cache(
                    &mut client,
                    path,
                    content_hash.as_bytes().as_slice(),
                )?;
                return Ok(None);
            }
            let stored_file_size: i64 = row.get(4);
            if !reject_cache_hit_on_record_file_size_mismatch(
                &mut client,
                "quality_records",
                path,
                content_hash.as_bytes().as_slice(),
                &sig,
                stored_file_size,
                "quality-hash-hit",
            )? {
                return Ok(None);
            }
            let data: Vec<u8> = row.get(0);
            let stored_checksum_opt = row.get::<_, Option<i64>>(1);
            let valid_checksum = stored_checksum_opt
                .and_then(|cs| crate::numeric_cast::i64_to_u32_strict(cs, "quality_checksum"));

            let Some(checksum) = valid_checksum else {
                crate::media_conversion_gate::analysis_cache_invalidate_audit(
                    "analysis_cache_checksum_invalid",
                    path,
                    "missing or invalid checksum; invalidating (quality hash hit)",
                );
                purge_corrupt_cache_record(
                    &mut client,
                    "quality_records",
                    path,
                    content_hash.as_bytes().as_slice(),
                    "checksum-invalid-hash",
                )?;
                return Ok(None);
            };

            if calculate_checksum(&data) != checksum {
                crate::media_conversion_gate::analysis_cache_invalidate_audit(
                    "analysis_cache_checksum_mismatch",
                    path,
                    "checksum mismatch (quality hash hit)",
                );
                purge_corrupt_cache_record(
                    &mut client,
                    "quality_records",
                    path,
                    content_hash.as_bytes().as_slice(),
                    "checksum-mismatch-hash",
                )?;
                return Ok(None);
            }

            let stored_fingerprint: Option<Vec<u8>> = row.get(3);
            if !reject_cache_hit_on_content_fingerprint_mismatch(
                &mut client,
                "quality_records",
                path,
                content_hash.as_bytes().as_slice(),
                stored_fingerprint,
                "hash-hit",
            )? {
                return Ok(None);
            }

            let Some(analysis) = unpack_cached_payload::<ImageQualityAnalysis>(
                &mut client,
                "quality_records",
                path,
                content_hash.as_bytes().as_slice(),
                &data,
                "hash-hit",
            )?
            else {
                return Ok(None);
            };
            if !quality_analysis_is_positive_cache_entry(path, &analysis) {
                log_negative_quality_cache_rejected(path, &analysis, "hash-hit");
                purge_negative_quality_cache(
                    &mut client,
                    path,
                    content_hash.as_bytes().as_slice(),
                )?;
                return Ok(None);
            }

            // Backfill path index
            client.execute(
                "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, \
                 btime) 
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime \
                 = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime \
                 = EXCLUDED.ctime, btime = EXCLUDED.btime",
                &[
                    &path_str.to_string(),
                    &content_hash.as_bytes().as_slice(),
                    &sig.mtime,
                    &sig.size,
                    &sig.atime,
                    &sig.ctime,
                    &sig.btime,
                ],
            )?;

            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_CACHE,
                &format!("Quality HIT (Hash) - {}", path.display())
            );
            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Store the analysis results for a given image file.
    ///
    /// # Errors
    /// Returns an error if the database insertion fails.
    pub fn store_analysis(&self, path: &Path, analysis: &ImageAnalysis) -> Result<()> {
        if !image_analysis_is_positive_cache_entry(path, analysis) {
            log_negative_image_cache_rejected(path, analysis, "store");
            return Ok(());
        }

        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        let content_hash = calculate_blake3(path)?;
        let content_fingerprint = calculate_content_fingerprint(path)?;
        let packed_data = rmp_serde::to_vec(analysis).context("Failed to pack analysis data")?;
        let checksum = calculate_checksum(&packed_data);
        let now = crate::numeric_cast::unix_secs_i64_result()?;

        let mut tx = client.transaction()?;
        tx.execute(
            "INSERT INTO analysis_records (content_hash, file_size, analysis_data, created_at, \
             algorithm_version, content_fingerprint_hash, data_checksum)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (content_hash) DO UPDATE SET file_size = EXCLUDED.file_size, \
             analysis_data = EXCLUDED.analysis_data, created_at = EXCLUDED.created_at, \
             algorithm_version = EXCLUDED.algorithm_version, content_fingerprint_hash = \
             EXCLUDED.content_fingerprint_hash, data_checksum = EXCLUDED.data_checksum",
            &[
                &content_hash.as_bytes().as_slice(),
                &sig.size,
                &packed_data,
                &now,
                &cache_algorithm(),
                &content_fingerprint.as_slice(),
                &(i64::from(checksum)),
            ],
        )?;
        tx.execute(
            "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, \
             btime)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = \
             EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = \
             EXCLUDED.ctime, btime = EXCLUDED.btime",
            &[
                &path_str.to_string(),
                &content_hash.as_bytes().as_slice(),
                &sig.mtime,
                &sig.size,
                &sig.atime,
                &sig.ctime,
                &sig.btime,
            ],
        )?;
        tx.commit()?;

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_CACHE,
            &format!("Stored analysis for {}", path.display())
        );
        Ok(())
    }

    /// Store the quality analysis results for a given image file.
    ///
    /// # Errors
    /// Returns an error if the database insertion fails.
    pub fn store_quality_analysis(
        &self,
        path: &Path,
        analysis: &ImageQualityAnalysis,
    ) -> Result<()> {
        if !quality_analysis_is_positive_cache_entry(path, analysis) {
            log_negative_quality_cache_rejected(path, analysis, "store");
            return Ok(());
        }

        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        let content_hash = calculate_blake3(path)?;
        let content_fingerprint = calculate_content_fingerprint(path)?;
        let packed_data = rmp_serde::to_vec(analysis).context("Failed to pack quality data")?;
        let checksum = calculate_checksum(&packed_data);
        let now = crate::numeric_cast::unix_secs_i64_result()?;

        let mut tx = client.transaction()?;
        tx.execute(
            "INSERT INTO quality_records (content_hash, file_size, analysis_data, created_at, \
             algorithm_version, content_fingerprint_hash, data_checksum)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (content_hash) DO UPDATE SET file_size = EXCLUDED.file_size, \
             analysis_data = EXCLUDED.analysis_data, created_at = EXCLUDED.created_at, \
             algorithm_version = EXCLUDED.algorithm_version, content_fingerprint_hash = \
             EXCLUDED.content_fingerprint_hash, data_checksum = EXCLUDED.data_checksum",
            &[
                &content_hash.as_bytes().as_slice(),
                &sig.size,
                &packed_data,
                &now,
                &cache_algorithm(),
                &content_fingerprint.as_slice(),
                &(i64::from(checksum)),
            ],
        )?;
        tx.execute(
            "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, \
             btime)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = \
             EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = \
             EXCLUDED.ctime, btime = EXCLUDED.btime",
            &[
                &path_str.to_string(),
                &content_hash.as_bytes().as_slice(),
                &sig.mtime,
                &sig.size,
                &sig.atime,
                &sig.ctime,
                &sig.btime,
            ],
        )?;
        tx.commit()?;

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_CACHE,
            &format!("Stored quality analysis for {}", path.display())
        );
        Ok(())
    }

    /// Retrieve the video detection results for a given file.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    /// # Panics
    ///
    /// Panics if the database schema is corrupted or columns are missing from
    /// the `video_records` table.
    pub fn get_video_analysis(&self, path: &Path) -> Result<Option<Detection>> {
        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();

        // 1. Path Index
        let row = client.query_opt(
            "SELECT r.analysis_data, r.data_checksum, r.algorithm_version, p.ctime, p.btime, \
             p.content_hash, r.content_fingerprint_hash, r.file_size FROM path_index p 
             JOIN video_records r ON p.content_hash = r.content_hash
             WHERE p.file_path = $1 AND p.mtime = $2 AND p.file_size = $3",
            &[&path_str.to_string(), &sig.mtime, &sig.size],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(2);
            if !cache_record_algorithm_current(algorithm_version, path, "video-path-hit") {
                let content_hash: Vec<u8> = row.get(5);
                purge_negative_video_cache(&mut client, path, content_hash.as_slice())?;
                return Ok(None);
            }
            let row_ctime_epoch: i64 = row.get(3);
            let row_birthtime_epoch: i64 = row.get(4);

            if (row_ctime_epoch == 0 || row_ctime_epoch == sig.ctime)
                && (row_birthtime_epoch == 0 || row_birthtime_epoch == sig.btime)
            {
                let content_hash: Vec<u8> = row.get(5);
                if !reject_stale_path_index_hit(
                    &mut client,
                    path,
                    content_hash.as_slice(),
                    "video-path-hit",
                )? {
                    return Ok(None);
                }

                let stored_file_size: i64 = row.get(7);
                if !reject_cache_hit_on_record_file_size_mismatch(
                    &mut client,
                    "video_records",
                    path,
                    content_hash.as_slice(),
                    &sig,
                    stored_file_size,
                    "video-path-hit",
                )? {
                    return Ok(None);
                }

                let data: Vec<u8> = row.get(0);
                let stored_checksum_opt = row.get::<_, Option<i64>>(1);
                let valid_checksum = stored_checksum_opt
                    .and_then(|cs| crate::numeric_cast::i64_to_u32_strict(cs, "video_checksum"));

                let Some(checksum) = valid_checksum else {
                    crate::media_conversion_gate::analysis_cache_invalidate_audit(
                        "analysis_cache_checksum_invalid",
                        path,
                        "missing or invalid checksum; invalidating (video path hit)",
                    );
                    purge_corrupt_cache_record(
                        &mut client,
                        "video_records",
                        path,
                        content_hash.as_slice(),
                        "checksum-invalid-path",
                    )?;
                    return Ok(None);
                };

                if calculate_checksum(&data) != checksum {
                    crate::media_conversion_gate::analysis_cache_invalidate_audit(
                        "analysis_cache_checksum_mismatch",
                        path,
                        "checksum mismatch (video path index)",
                    );
                    purge_corrupt_cache_record(
                        &mut client,
                        "video_records",
                        path,
                        content_hash.as_slice(),
                        "checksum-mismatch-path",
                    )?;
                    return Ok(None);
                }

                let stored_fingerprint: Option<Vec<u8>> = row.get(6);
                if !reject_cache_hit_on_content_fingerprint_mismatch(
                    &mut client,
                    "video_records",
                    path,
                    content_hash.as_slice(),
                    stored_fingerprint,
                    "path-hit",
                )? {
                    return Ok(None);
                }

                let Some(mut analysis) = unpack_cached_payload::<Detection>(
                    &mut client,
                    "video_records",
                    path,
                    content_hash.as_slice(),
                    &data,
                    "path-hit",
                )?
                else {
                    return Ok(None);
                };
                analysis.file_path = path.display().to_string();
                if !video_analysis_is_positive_cache_entry(path, &analysis) {
                    log_negative_video_cache_rejected(path, &analysis, "path-hit");
                    purge_negative_video_cache(&mut client, path, content_hash.as_slice())?;
                    return Ok(None);
                }
                return Ok(Some(analysis));
            }
        }

        // 2. Hash Index
        let content_hash = calculate_blake3(path)?;
        let row = client.query_opt(
            "SELECT analysis_data, data_checksum, algorithm_version, content_fingerprint_hash, \
             file_size FROM video_records WHERE content_hash = $1",
            &[&content_hash.as_bytes().as_slice()],
        )?;

        if let Some(row) = row {
            let algorithm_version: i32 = row.get(2);
            if !cache_record_algorithm_current(algorithm_version, path, "video-hash-hit") {
                purge_negative_video_cache(&mut client, path, content_hash.as_bytes().as_slice())?;
                return Ok(None);
            }
            let stored_file_size: i64 = row.get(4);
            if !reject_cache_hit_on_record_file_size_mismatch(
                &mut client,
                "video_records",
                path,
                content_hash.as_bytes().as_slice(),
                &sig,
                stored_file_size,
                "video-hash-hit",
            )? {
                return Ok(None);
            }
            let data: Vec<u8> = row.get(0);
            let stored_checksum_opt = row.get::<_, Option<i64>>(1);
            let valid_checksum = stored_checksum_opt
                .and_then(|cs| crate::numeric_cast::i64_to_u32_strict(cs, "video_checksum"));

            let Some(checksum) = valid_checksum else {
                crate::media_conversion_gate::analysis_cache_invalidate_audit(
                    "analysis_cache_checksum_invalid",
                    path,
                    "missing or invalid checksum; invalidating",
                );
                purge_corrupt_cache_record(
                    &mut client,
                    "video_records",
                    path,
                    content_hash.as_bytes().as_slice(),
                    "checksum-invalid-hash",
                )?;
                return Ok(None);
            };

            if calculate_checksum(&data) != checksum {
                crate::media_conversion_gate::analysis_cache_invalidate_audit(
                    "analysis_cache_checksum_mismatch",
                    path,
                    "checksum mismatch",
                );
                purge_corrupt_cache_record(
                    &mut client,
                    "video_records",
                    path,
                    content_hash.as_bytes().as_slice(),
                    "checksum-mismatch-hash",
                )?;
                return Ok(None);
            }

            let stored_fingerprint: Option<Vec<u8>> = row.get(3);
            if !reject_cache_hit_on_content_fingerprint_mismatch(
                &mut client,
                "video_records",
                path,
                content_hash.as_bytes().as_slice(),
                stored_fingerprint,
                "hash-hit",
            )? {
                return Ok(None);
            }

            let Some(mut analysis) = unpack_cached_payload::<Detection>(
                &mut client,
                "video_records",
                path,
                content_hash.as_bytes().as_slice(),
                &data,
                "hash-hit",
            )?
            else {
                return Ok(None);
            };
            analysis.file_path = path.display().to_string();
            if !video_analysis_is_positive_cache_entry(path, &analysis) {
                log_negative_video_cache_rejected(path, &analysis, "hash-hit");
                purge_negative_video_cache(&mut client, path, content_hash.as_bytes().as_slice())?;
                return Ok(None);
            }

            // Backfill path index
            client.execute(
                "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, \
                 btime) 
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime \
                 = EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime \
                 = EXCLUDED.ctime, btime = EXCLUDED.btime",
                &[
                    &path_str.to_string(),
                    &content_hash.as_bytes().as_slice(),
                    &sig.mtime,
                    &sig.size,
                    &sig.atime,
                    &sig.ctime,
                    &sig.btime,
                ],
            )?;

            return Ok(Some(analysis));
        }

        Ok(None)
    }

    /// Store the video detection results for a given file.
    ///
    /// # Errors
    /// Returns an error if the database insertion fails.
    pub fn store_video_analysis(&self, path: &Path, analysis: &Detection) -> Result<()> {
        if !video_analysis_is_positive_cache_entry(path, analysis) {
            log_negative_video_cache_rejected(path, analysis, "store");
            return Ok(());
        }

        let mut client = open_pg_client()?;
        let sig = FileSignature::from_path(path)?;
        let path_str = path.to_string_lossy();
        let content_hash = calculate_blake3(path)?;
        let content_fingerprint = calculate_content_fingerprint(path)?;
        let packed_data = rmp_serde::to_vec(analysis).context("Failed to pack video data")?;
        let checksum = calculate_checksum(&packed_data);
        let now = crate::numeric_cast::unix_secs_i64_result()?;

        let mut tx = client.transaction()?;
        tx.execute(
            "INSERT INTO video_records (content_hash, file_size, analysis_data, created_at, \
             algorithm_version, content_fingerprint_hash, data_checksum)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (content_hash) DO UPDATE SET file_size = EXCLUDED.file_size, \
             analysis_data = EXCLUDED.analysis_data, created_at = EXCLUDED.created_at, \
             algorithm_version = EXCLUDED.algorithm_version, content_fingerprint_hash = \
             EXCLUDED.content_fingerprint_hash, data_checksum = EXCLUDED.data_checksum",
            &[
                &content_hash.as_bytes().as_slice(),
                &sig.size,
                &packed_data,
                &now,
                &cache_algorithm(),
                &content_fingerprint.as_slice(),
                &(i64::from(checksum)),
            ],
        )?;
        tx.execute(
            "INSERT INTO path_index (file_path, content_hash, mtime, file_size, atime, ctime, \
             btime)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (file_path) DO UPDATE SET content_hash = EXCLUDED.content_hash, mtime = \
             EXCLUDED.mtime, file_size = EXCLUDED.file_size, atime = EXCLUDED.atime, ctime = \
             EXCLUDED.ctime, btime = EXCLUDED.btime",
            &[
                &path_str.to_string(),
                &content_hash.as_bytes().as_slice(),
                &sig.mtime,
                &sig.size,
                &sig.atime,
                &sig.ctime,
                &sig.btime,
            ],
        )?;
        tx.commit()?;

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_CACHE,
            &format!("Stored video analysis for {}", path.display())
        );
        Ok(())
    }

    /// Delete old records from the cache.
    ///
    /// # Errors
    /// Returns an error if the database deletion fails.
    /// # Panics
    ///
    /// Panics if the database schema is corrupted or columns are missing.
    pub fn cleanup_old_records(&self, max_age_secs: i64) -> Result<usize> {
        let mut client = open_pg_client()?;
        let now = crate::numeric_cast::unix_secs_i64_result()?;
        let threshold = now - max_age_secs;

        let tables = ["analysis_records", "quality_records", "video_records"];
        let mut removed: usize = 0;
        for table in &tables {
            let count = match usize::try_from(client.execute(
                &format!("DELETE FROM {table} WHERE created_at < $1"),
                &[&threshold],
            )?) {
                Ok(v) => v,
                Err(e) => {
                    crate::media_conversion_gate::delivery_api_batch_fallback_audit(
                        "analysis_cache_prune_count_invalid",
                        format!(
                            "failed to parse cache pruning result for {table}: {e:?}; assuming 0 \
                             removed"
                        ),
                    );
                    0
                }
            };
            removed = removed.saturating_add(count);
        }

        if removed > 0 {
            purge_orphan_path_index_entries(&mut client)?;
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_CACHE,
                &format!("Pruned {removed} old records across analysis/quality/video cache tables")
            );
        }
        Ok(removed)
    }

    /// Get cache usage statistics.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    /// # Panics
    ///
    /// Panics if the database schema is corrupted or mandatory metadata entries
    /// are missing.
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
                let entry = version_dist.entry(v).or_insert(0i64);
                *entry = entry.saturating_add(c);
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
            analysis_records: crate::numeric_cast::i64_to_usize_sat(analysis_count),
            quality_records: crate::numeric_cast::i64_to_usize_sat(quality_count),
            video_records: crate::numeric_cast::i64_to_usize_sat(video_count),
            path_index_entries: crate::numeric_cast::i64_to_usize_sat(path_index_count),
            schema_version,
            algorithm_version_distribution: version_dist,
            current_algorithm_version: cache_algorithm(),
        })
    }

    /// Enforce the cache size limit by deleting old records.
    ///
    /// # Errors
    /// Returns an error if the database deletion fails.
    pub const fn enforce_size_limit(&self) -> Result<()> {
        // Size enforcement in shared Postgres is handled differently (usually by policy
        // or quota) or we can implement a row-count based pruning here if
        // needed. For now, we rely on cleanup_old_records.
        Ok(())
    }
}

/// Calculates the BLAKE3 hash of a file's entire content.
fn calculate_blake3(path: &Path) -> Result<blake3::Hash> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0u8; crate::constants::IO_BUFFER_SIZE_LARGE];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(crate::media_conversion_gate::probe_hash_buffer_slice(
            &buffer, bytes_read, "blake3",
        ));
    }
    Ok(hasher.finalize())
}

/// Calculates a quick content fingerprint using the first 64KB of a file.
fn calculate_content_fingerprint(path: &Path) -> Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0u8; crate::constants::IO_BUFFER_SIZE_LARGE];
    let bytes_read = file.read(&mut buffer)?;
    hasher.update(crate::media_conversion_gate::probe_hash_buffer_slice(
        &buffer, bytes_read, "blake3",
    ));
    Ok(*hasher.finalize().as_bytes())
}

/// Calculates a CRC32 checksum for data integrity verification.
fn calculate_checksum(data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_detection::PrecisionMetadata;
    use crate::image_quality_detector::{ImageContentType, ImageQualityAnalysis};
    use crate::types::{ProcessHistory, Visual};
    use std::io::Write;

    fn test_quality_analysis(
        format: &str,
        width: u32,
        height: u32,
        is_animated: bool,
        frame_count: Option<u32>,
    ) -> ImageQualityAnalysis {
        ImageQualityAnalysis {
            width,
            height,
            file_size: 1,
            format: format.to_string(),
            has_alpha: false,
            is_animated,
            frame_count,
            complexity: None,
            edge_density: None,
            color_diversity: None,
            texture_variance: None,
            noise_level: None,
            sharpness: None,
            contrast: None,
            content_type: ImageContentType {
                name: "PHOTO".to_string(),
            },
            confidence: None,
            precision: PrecisionMetadata::default(),
            history: ProcessHistory::default(),
            perception: Visual::default(),
        }
    }

    #[test]
    fn test_calculate_checksum() {
        let data = b"hello world";
        let c1 = calculate_checksum(data);
        let c2 = calculate_checksum(data);
        assert_eq!(c1, c2);
        assert_ne!(c1, calculate_checksum(b"hello world!"));
    }

    #[test]
    fn test_file_fingerprint_and_blake3() -> Result<()> {
        let mut temp = tempfile::NamedTempFile::new()?;
        temp.write_all(b"test data for cache fingerprint")?;
        let path = temp.path();

        let h1 = calculate_blake3(path)?;
        let f1 = calculate_content_fingerprint(path)?;

        assert_ne!(h1.as_bytes(), &[0u8; 32]);
        assert_ne!(f1, [0u8; 32]);

        let sig = FileSignature::from_path(path)?;
        assert_eq!(sig.size, i64::try_from(temp.as_file().metadata()?.len())?);
        Ok(())
    }

    #[test]
    fn static_animation_capable_image_analysis_is_not_cacheable() {
        let analysis = ImageAnalysis {
            format: "GIF".to_string(),
            is_animated: false,
            ..ImageAnalysis::default()
        };

        assert!(!image_analysis_is_positive_cache_entry(
            Path::new("sample.gif"),
            &analysis
        ));
    }

    #[test]
    fn animated_image_analysis_is_cacheable() {
        let analysis = ImageAnalysis {
            format: "GIF".to_string(),
            width: 10,
            height: 8,
            is_animated: true,
            ..ImageAnalysis::default()
        };

        assert!(image_analysis_is_positive_cache_entry(
            Path::new("sample.gif"),
            &analysis
        ));
    }

    #[test]
    fn static_jpeg_image_analysis_is_cacheable() {
        let analysis = ImageAnalysis {
            format: "JPEG".to_string(),
            width: 1920,
            height: 1080,
            is_animated: false,
            ..ImageAnalysis::default()
        };

        assert!(image_analysis_is_positive_cache_entry(
            Path::new("sample.jpg"),
            &analysis
        ));
    }

    #[test]
    fn static_quality_analysis_is_cacheable_m133() {
        let analysis = test_quality_analysis("JPEG", 640, 480, false, Some(1));

        assert!(quality_analysis_is_positive_cache_entry(
            Path::new("sample.jpg"),
            &analysis
        ));
    }

    #[test]
    fn zero_canvas_quality_analysis_is_not_cacheable_m133() {
        let analysis = test_quality_analysis("WEBP", 0, 80, true, Some(2));

        assert!(!quality_analysis_is_positive_cache_entry(
            Path::new("sample.webp"),
            &analysis
        ));
    }

    #[test]
    fn single_frame_animated_quality_analysis_is_not_cacheable_m133() {
        let analysis = test_quality_analysis("GIF", 10, 8, true, Some(1));

        assert!(!quality_analysis_is_positive_cache_entry(
            Path::new("sample.gif"),
            &analysis
        ));
    }

    #[test]
    fn zero_canvas_image_analysis_is_not_cacheable_m131() {
        let analysis = ImageAnalysis {
            format: "GIF".to_string(),
            width: 0,
            height: 80,
            is_animated: true,
            ..ImageAnalysis::default()
        };

        assert!(!image_analysis_is_positive_cache_entry(
            Path::new("sample.gif"),
            &analysis
        ));
    }

    #[test]
    fn animated_image_analysis_requires_canvas_m131() {
        let analysis = ImageAnalysis {
            format: "GIF".to_string(),
            width: 10,
            height: 8,
            is_animated: true,
            ..ImageAnalysis::default()
        };

        assert!(image_analysis_is_positive_cache_entry(
            Path::new("sample.gif"),
            &analysis
        ));
    }

    #[test]
    fn single_frame_animation_capable_video_analysis_is_not_cacheable() {
        let detection = Detection {
            format: "gif".to_string(),
            frame_count: Some(1),
            ..Detection::default()
        };

        assert!(!video_analysis_is_positive_cache_entry(
            Path::new("sample.gif"),
            &detection
        ));
    }

    #[test]
    fn multi_frame_animation_capable_video_analysis_is_cacheable() {
        let detection = Detection {
            format: "gif".to_string(),
            frame_count: Some(2),
            width: Some(10),
            height: Some(8),
            ..Detection::default()
        };

        assert!(video_analysis_is_positive_cache_entry(
            Path::new("sample.gif"),
            &detection
        ));
    }

    #[test]
    fn multi_frame_zero_canvas_video_analysis_is_not_cacheable_m130() {
        let detection = Detection {
            format: "webp".to_string(),
            frame_count: Some(2),
            width: None,
            height: None,
            ..Detection::default()
        };

        assert!(!video_analysis_is_positive_cache_entry(
            Path::new("sample.webp"),
            &detection
        ));
    }

    #[test]
    fn non_animated_video_requires_trustworthy_canvas_m130() {
        let detection = Detection {
            format: "mp4".to_string(),
            frame_count: Some(240),
            width: Some(0),
            height: Some(1080),
            ..Detection::default()
        };

        assert!(!video_analysis_is_positive_cache_entry(
            Path::new("sample.mp4"),
            &detection
        ));
    }
}
