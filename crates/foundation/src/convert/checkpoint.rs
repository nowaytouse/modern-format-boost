//! Checkpoint & Resume Module (Progress Tracking)
//!
//! Provides atomic operation protection and resume capability for all
//! conversion tools:
//! - Progress tracking: Record completed files for resume after interruption
//! - Atomic delete: Verify output integrity before deleting original
//! - Lock file: Prevent concurrent processing of same directory
//!
//! # Usage
//! ```no_run
//! use foundation::checkpoint::{Manager, safe_delete_original};
//! use foundation::constants::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE;
//! use std::path::Path;
//!
//! fn main() -> anyhow::Result<()> {
//!     let target_dir = Path::new("/tmp/test");
//!     let file_path = Path::new("/tmp/test/file.jpg");
//!     let input = Path::new("/tmp/test/input.jpg");
//!     let output = Path::new("/tmp/test/output.jxl");
//!
//!     // Initialize checkpoint for a directory
//!     let mut checkpoint = Manager::new(target_dir)?;
//!
//!     // Check if file was already processed
//!     if !checkpoint.is_completed(&file_path) {
//!         // ... do conversion ...
//!
//!         // Mark as completed
//!         checkpoint.mark_completed(&file_path)?;
//!     }
//!
//!     // Safe delete with integrity check
//!     foundation::checkpoint::safe_delete_original(
//!         &input,
//!         &output,
//!         foundation::constants::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
//!     )?;
//!     Ok(())
//! }
//! ```

use crate::{HostnameBuilder, ToolBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use crate::version::{CACHE_SCHEMA_VERSION, cache_algorithm};

/// The central location for all MFB progress tracking to avoid polluting user
/// directories.
///
/// # Errors
/// Returns an error if no usable home/progress root can be determined.
fn get_central_progress_dir() -> io::Result<PathBuf> {
    match std::env::var(crate::constants::ENV_MFB_PROGRESS_DIR) {
        Ok(path) => return Ok(PathBuf::from(path)),
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_progress_dir",
                format!(
                    "failed to read {}: {e}",
                    crate::constants::ENV_MFB_PROGRESS_DIR
                ),
            );
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Failed to read checkpoint progress dir env {}: {e}",
                    crate::constants::ENV_MFB_PROGRESS_DIR
                ),
            ));
        }
    }

    let root = crate::process_lock::get_mfb_root().map_err(|e| {
        io::Error::other(format!("Failed to determine checkpoint progress root: {e}"))
    })?;
    Ok(root.join("progress"))
}
/// Timeout in seconds for considering a lock file as stale.
///
/// After this duration, a lock file is considered abandoned and can be
/// safely overridden. Default is 24 hours.
const LOCK_STALE_TIMEOUT_SECS: u64 = crate::constants::LOCK_STALE_TIMEOUT_SECS;

/// Current version of the checkpoint file format.
///
/// Increment this when the checkpoint structure changes to invalidate
/// old checkpoint files and force regeneration.
const CHECKPOINT_FORMAT_VERSION: u32 = crate::constants::CHECKPOINT_FORMAT_VERSION;

/// Wall-clock duration for checkpoint headers (M29 SSOT; never used to
/// fabricate file mtimes).
fn checkpoint_wall_clock_duration_since_epoch() -> io::Result<std::time::Duration> {
    crate::media_conversion_gate::unix_duration_since_epoch_optional().ok_or_else(|| {
        io::Error::other("checkpoint wall-clock SSOT unavailable (SystemTime before UNIX_EPOCH)")
    })
}

/// Gets the current Unix timestamp in seconds (M29 epoch SSOT).
fn current_unix_secs() -> io::Result<u64> {
    crate::media_conversion_gate::unix_epoch_secs_optional().ok_or_else(|| {
        io::Error::other("checkpoint epoch SSOT unavailable (SystemTime before UNIX_EPOCH)")
    })
}

/// Represents a single file entry in a checkpoint.
///
/// Stores essential metadata about a file to detect changes between runs.
/// This enables efficient incremental processing by comparing current
/// file state with the checkpointed state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CheckpointEntry {
    /// Relative path to the file from the checkpoint root.
    path: String,
    /// File size in bytes.
    size: i64,
    /// Last modification time in Unix seconds.
    mtime: i64,
    /// Creation time in Unix seconds.
    ctime: i64,
    /// Birth time in Unix seconds (if available).
    #[serde(default)]
    btime: Option<i64>,
}

impl CheckpointEntry {
    fn from_path(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let size = crate::numeric_cast::u64_to_i64_strict(metadata.len(), "size")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "File size cast failed"))?;
        let mtime = crate::numeric_cast::u128_to_i64_strict(
            metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("file mtime before UNIX epoch ({e})"),
                    )
                })?
                .as_nanos(),
            "mtime",
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "mtime cast failed"))?;

        #[cfg(unix)]
        let ctime = {
            use std::os::unix::fs::MetadataExt;
            metadata.ctime_nsec()
        };
        #[cfg(windows)]
        use std::os::windows::fs::MetadataExt;
        #[cfg(windows)]
        let ctime = crate::numeric_cast::u64_to_i64_strict(metadata.last_write_time(), "ctime")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ctime cast failed"))?;
        #[cfg(not(any(unix, windows)))]
        let ctime = mtime;

        let btime = match metadata.created() {
            Ok(created) => match created.duration_since(UNIX_EPOCH) {
                Ok(duration) => {
                    crate::numeric_cast::u128_to_i64_strict(duration.as_nanos(), "btime")
                        .map_or_else(
                            || {
                                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                                    "checkpoint_entry",
                                    path,
                                    "birth-time nanoseconds did not fit i64; omitting btime",
                                );
                                None
                            },
                            Some,
                        )
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_checkpoint_path_audit(
                        "checkpoint_entry",
                        path,
                        format!("birth time is before UNIX_EPOCH; omitting btime: {e}"),
                    );
                    None
                }
            },
            Err(e) => {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_entry",
                    path,
                    format!("failed to read birth time; omitting btime: {e}"),
                );
                None
            }
        };

        Ok(Self {
            path: Manager::normalize_path(path),
            size,
            mtime,
            ctime,
            btime,
        })
    }

    fn matches_current_file(&self, path: &Path) -> io::Result<bool> {
        Ok(Self::from_path(path)? == *self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CheckpointHeader {
    format_version: u32,
    target_dir: String,
    output_root: Option<String>,
    cache_algorithm_version: i32,
    cache_schema_version: i32,
    created_at: u64,
}

impl CheckpointHeader {
    fn new(target_dir: &Path, output_root: Option<&Path>) -> io::Result<Self> {
        Ok(Self {
            format_version: CHECKPOINT_FORMAT_VERSION,
            target_dir: Manager::normalize_path(target_dir),
            output_root: output_root.map(Manager::normalize_path),
            cache_algorithm_version: cache_algorithm(),
            cache_schema_version: CACHE_SCHEMA_VERSION,
            created_at: current_unix_secs()?,
        })
    }

    fn is_compatible_with(&self, expected: &Self) -> bool {
        self.format_version == expected.format_version
            && self.target_dir == expected.target_dir
            && self.output_root == expected.output_root
            && self.cache_algorithm_version == expected.cache_algorithm_version
            && self.cache_schema_version == expected.cache_schema_version
    }
}

#[derive(Debug, Default)]
struct LoadedCheckpointState {
    header: Option<CheckpointHeader>,
    entries: HashMap<String, CheckpointEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointBlob {
    header: CheckpointHeader,
    entries: HashMap<String, CheckpointEntry>,
}

const _: () = assert!(crate::constants::CHECKPOINT_FORMAT_VERSION == 2);
const CHECKPOINT_BLOB_SCHEMA: i32 = 2;

fn load_progress_from_sqlite(key: &str) -> io::Result<Option<LoadedCheckpointState>> {
    let Some(bytes) = crate::mfb_sqlite_store::blob_get(
        crate::mfb_sqlite_store::NS_CHECKPOINT,
        key,
        CHECKPOINT_BLOB_SCHEMA,
    )
    .map_err(io::Error::other)?
    else {
        return Ok(None);
    };
    let blob: CheckpointBlob = serde_json::from_slice(&bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse checkpoint SQLite blob: {err}"),
        )
    })?;
    Ok(Some(LoadedCheckpointState {
        header: Some(blob.header),
        entries: blob.entries,
    }))
}

fn save_progress_to_sqlite(
    key: &str,
    header: &CheckpointHeader,
    entries: &HashMap<String, CheckpointEntry>,
) -> io::Result<()> {
    let blob = CheckpointBlob {
        header: header.clone(),
        entries: entries.clone(),
    };
    let bytes =
        serde_json::to_vec(&blob).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    crate::mfb_sqlite_store::blob_put(
        crate::mfb_sqlite_store::NS_CHECKPOINT,
        key,
        CHECKPOINT_BLOB_SCHEMA,
        None,
        &bytes,
    )
    .map_err(io::Error::other)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockInfo {
    pid: u32,
    start_time: u64,
    created_at: u64,
    hostname: String,
}

impl LockInfo {
    fn new() -> io::Result<Self> {
        let now = current_unix_secs()?;
        Ok(Self {
            pid: std::process::id(),
            start_time: crate::media_conversion_gate::delivery_checkpoint_lock_start_time_or_now(
                get_process_start_time(),
                now,
            ),
            created_at: now,
            hostname: get_hostname(),
        })
    }

    fn is_stale(&self) -> bool {
        let Ok(now_secs) = checkpoint_wall_clock_duration_since_epoch().map(|d| d.as_secs()) else {
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_fallback",
                "Checkpoint SSOT wall-clock missing; treating lock as stale to prevent deadlock",
            );
            return true;
        };
        now_secs.saturating_sub(self.created_at) > LOCK_STALE_TIMEOUT_SECS
    }
}

/// Gets the start time of the current process.
///
/// # Returns
/// Process start time in Unix seconds, or None if unavailable
#[cfg(unix)]
fn get_process_start_time() -> Option<u64> {
    get_process_start_time_for_pid(std::process::id())
}

/// Gets the start time of the current process (non-Unix fallback).
///
/// On non-Unix systems, returns current time as approximation.
///
/// # Returns
/// Process start time approximation in Unix seconds
#[cfg(not(unix))]
fn get_process_start_time() -> Option<u64> {
    match current_unix_secs() {
        Ok(now) => Some(now),
        Err(e) => {
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_process",
                format!("failed to read wall-clock for process start fallback: {e}"),
            );
            None
        }
    }
}

/// Gets the start time of a specific process by PID.
///
/// # Arguments
/// * `pid` - Process ID to query
///
/// # Returns
/// Process start time in Unix seconds, or None if unavailable
#[cfg(unix)]
fn get_process_start_time_for_pid(pid: u32) -> Option<u64> {
    for field in ["etimes", "etime"] {
        let output = match crate::tool_builders::PsBuilder::new()
            .pid(pid)
            .output_field(field)
            .build()
            .output()
        {
            Ok(o) => o,
            Err(err) => {
                crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                    "checkpoint_process",
                    format!("Failed to query process age for PID {pid} via ps {field}: {err}"),
                );
                return None;
            }
        };

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let elapsed_secs = match field {
                "etimes" => crate::numeric_cast::parse_strict::<u64>(stdout.trim(), "ps_etimes"),
                "etime" => parse_ps_etime_to_secs(&stdout),
                _ => None,
            };

            if let Some(elapsed_secs) = elapsed_secs {
                return match current_unix_secs() {
                    Ok(now) => Some(now.saturating_sub(elapsed_secs)),
                    Err(e) => {
                        crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                            "checkpoint_process",
                            format!(
                                "failed to read wall-clock while computing process start for PID \
                                 {pid}: {e}"
                            ),
                        );
                        None
                    }
                };
            }

            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_process",
                format!(
                    "Failed to parse process age for PID {} from ps {} output: {}",
                    pid,
                    field,
                    stdout.trim()
                ),
            );
            continue;
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if ps_field_unsupported(stderr.trim()) {
            continue;
        }

        crate::media_conversion_gate::delivery_checkpoint_batch_audit(
            "checkpoint_process",
            format!(
                "ps {} returned non-zero while querying PID {}: {}",
                field,
                pid,
                stderr.trim()
            ),
        );
    }

    None
}

/// Checks if a ps field is unsupported based on stderr output.
///
/// # Arguments
/// * `stderr` - The stderr output from ps command
///
/// # Returns
/// `true` if the field is unsupported, `false` otherwise
#[cfg(unix)]
fn ps_field_unsupported(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("keyword not found")
        || lower.contains("no valid keywords")
        || lower.contains("invalid keyword")
}

/// Parses ps etime output to Unix seconds.
///
/// # Arguments
/// * `raw` - The raw etime output from ps command
///
/// # Returns
/// Unix seconds, or None if parsing fails
#[cfg(unix)]
fn parse_ps_etime_to_secs(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (days, clock) = match trimmed.split_once('-') {
        Some((days_str, rest)) => (
            crate::numeric_cast::parse_strict::<u64>(days_str.trim(), "ps_etime_days")?,
            rest.trim(),
        ),
        None => (0, trimmed),
    };

    let parts: Vec<_> = clock.split(':').collect();
    let clock_secs = match parts.as_slice() {
        [minutes, seconds] => {
            crate::numeric_cast::parse_strict::<u64>(minutes.trim(), "ps_etime_minutes")
                .map(|m| m * 60)
                .and_then(|m_scaled| {
                    crate::numeric_cast::parse_strict::<u64>(seconds.trim(), "ps_etime_seconds")
                        .map(|s| m_scaled + s)
                })?
        }
        [hours, minutes, seconds] => {
            let h = crate::numeric_cast::parse_strict::<u64>(hours.trim(), "ps_etime_hours")
                .map(|h| h * 3600)?;
            let m = crate::numeric_cast::parse_strict::<u64>(minutes.trim(), "ps_etime_minutes")
                .map(|m| m * 60)?;
            let s = crate::numeric_cast::parse_strict::<u64>(seconds.trim(), "ps_etime_seconds")?;
            h + m + s
        }
        _ => return None,
    };

    Some(days * 24 * 3600 + clock_secs)
}

#[cfg(not(unix))]
fn get_process_start_time_for_pid(_pid: u32) -> Option<u64> {
    None
}

/// Gets the hostname of the current machine.
///
/// Uses system commands to retrieve the hostname, with fallbacks for different
/// platforms. Returns "unknown" if hostname cannot be determined or is not
/// valid UTF-8.
///
/// # Returns
/// Hostname string, or "unknown" if unavailable
fn get_hostname() -> String {
    #[cfg(unix)]
    {
        match HostnameBuilder::new().build().output() {
            Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
                Ok(s) => s.trim().to_string(),
                Err(err) => {
                    crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                        "checkpoint_process",
                        format!("Non-UTF-8 hostname output: {err}"),
                    );
                    "unknown".to_string()
                }
            },
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                    "checkpoint_process",
                    format!("hostname returned non-zero status: {}", stderr.trim()),
                );
                "unknown".to_string()
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                    "checkpoint_process",
                    format!("Failed to query hostname: {err}"),
                );
                "unknown".to_string()
            }
        }
    }
    #[cfg(not(unix))]
    {
        "unknown".to_string()
    }
}

use std::sync::Mutex;

pub struct Manager {
    progress_dir: PathBuf,
    lock_file: PathBuf,
    checkpoint_key: String,
    header: CheckpointHeader,
    completed: Mutex<HashMap<String, CheckpointEntry>>,
    resume_mode: AtomicBool,
}

impl Manager {
    /// # Errors
    ///
    /// Returns an error if the progress directory cannot be created or if
    /// initialization fails.
    pub fn new(target_dir: &Path) -> io::Result<Self> {
        Self::new_with_context(target_dir, None)
    }

    /// # Errors
    ///
    /// Returns an error if the progress directory cannot be created or if
    /// initialization fails.
    pub fn new_resuming(target_dir: &Path) -> io::Result<Self> {
        Self::new_resuming_with_context(target_dir, None)
    }

    /// # Errors
    ///
    /// Returns an error if the progress directory cannot be created or if
    /// initialization fails.
    pub fn new_with_context(target_dir: &Path, output_root: Option<&Path>) -> io::Result<Self> {
        Self::new_with_context_inner(target_dir, output_root, false)
    }

    /// # Errors
    ///
    /// Returns an error if the progress directory cannot be created or if
    /// initialization fails.
    pub fn new_resuming_with_context(
        target_dir: &Path,
        output_root: Option<&Path>,
    ) -> io::Result<Self> {
        Self::new_with_context_inner(target_dir, output_root, true)
    }

    /// Return the number of still-valid saved entries without consuming or
    /// deleting the checkpoint.
    pub fn saved_entry_count(target_dir: &Path, output_root: Option<&Path>) -> io::Result<usize> {
        let canonical_target = Self::normalize_path_to_buf(target_dir);
        let checkpoint_key = Self::hash_path(&canonical_target);
        let header = CheckpointHeader::new(&canonical_target, output_root)?;
        let loaded = Self::load_progress(&checkpoint_key)?;
        let (valid, _, _) = Self::validate_loaded_state(&loaded, &header, output_root);
        Ok(valid.len())
    }

    /// Explicitly discard the saved checkpoint for a user-requested fresh run.
    pub fn discard_saved_progress(target_dir: &Path) -> io::Result<()> {
        let canonical_target = Self::normalize_path_to_buf(target_dir);
        Self::discard_progress_for_fresh_run(&Self::hash_path(&canonical_target))?;
        crate::conversion::clear_processed_list();
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if the progress directory cannot be created or if
    /// initialization fails.
    fn new_with_context_inner(
        target_dir: &Path,
        output_root: Option<&Path>,
        resume_enabled: bool,
    ) -> io::Result<Self> {
        let canonical_target = Self::normalize_path_to_buf(target_dir);
        let dir_hash = Self::hash_path(&canonical_target);
        let header = CheckpointHeader::new(&canonical_target, output_root)?;

        let central_dir = get_central_progress_dir()?;
        fs::create_dir_all(&central_dir)?;

        let checkpoint_key = dir_hash;
        let lock_file = central_dir.join(format!("{checkpoint_key}.lock"));

        let (completed_set, resume_mode, reset_reason) = if resume_enabled {
            let loaded = Self::load_progress(&checkpoint_key)?;
            Self::validate_loaded_state(&loaded, &header, output_root)
        } else {
            Self::discard_progress_for_fresh_run(&checkpoint_key)?;
            crate::conversion::clear_processed_list();
            (HashMap::new(), false, None)
        };

        if let Some(reason) = reset_reason.as_deref() {
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_fallback",
                reason,
            );
            if let Err(err) = crate::mfb_sqlite_store::blob_delete(
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                &checkpoint_key,
            ) {
                crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                    "checkpoint_progress",
                    format!(
                        "Failed to remove invalidated checkpoint blob for key {checkpoint_key}: \
                         {err}"
                    ),
                );
            }
        }

        let manager = Self {
            progress_dir: central_dir,
            lock_file,
            checkpoint_key,
            header,
            completed: Mutex::new(completed_set),
            resume_mode: AtomicBool::new(resume_mode),
        };

        if manager.resume_mode.load(Ordering::Relaxed)
            && let Err(err) = manager.persist_progress()
        {
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_progress",
                format!("Failed to compact validated checkpoint state: {err}"),
            );
        }

        Ok(manager)
    }

    fn discard_progress_for_fresh_run(checkpoint_key: &str) -> io::Result<()> {
        match crate::mfb_sqlite_store::blob_delete(
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            checkpoint_key,
        ) {
            Ok(rows) if rows > 0 => {
                crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                    "checkpoint_progress",
                    format!(
                        "Fresh run explicitly selected; discarded {rows} saved checkpoint row(s)."
                    ),
                );
            }
            Ok(_) => {}
            Err(err) => {
                let message = format!(
                    "Fresh run was explicitly selected, but failed to discard checkpoint blob for \
                     key {checkpoint_key}: {err}"
                );
                crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                    "checkpoint_progress",
                    &message,
                );
                return Err(io::Error::other(message));
            }
        }
        Ok(())
    }

    // Rationale: This function handles complex, sequential initialization or
    // business logic where further fragmentation would hinder readability and
    // maintainability.
    /// # Errors
    ///
    /// Returns an error if the lock file cannot be read.
    pub fn check_lock(&self) -> io::Result<Option<u32>> {
        if !self.lock_file.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.lock_file)?;

        match serde_json::from_str::<LockInfo>(&content) {
            Ok(lock_info) => {
                if lock_info.pid == std::process::id() {
                    crate::media_conversion_gate::delivery_checkpoint_path_audit(
                        "checkpoint_lock",
                        &self.lock_file,
                        "Found existing lock file with own PID; assuming clean reuse",
                    );
                    return Ok(None);
                }

                if lock_info.is_stale() {
                    crate::media_conversion_gate::delivery_checkpoint_path_audit(
                        "checkpoint_lock",
                        &self.lock_file,
                        "LOCK STALE: Lock file older than 24 hours, removing",
                    );
                    if let Err(e) = fs::remove_file(&self.lock_file) {
                        crate::media_conversion_gate::delivery_checkpoint_path_audit(
                            "checkpoint_lock",
                            &self.lock_file,
                            format!("Failed to remove stale lock file: {e}"),
                        );
                    }
                    return Ok(None);
                }

                #[cfg(unix)]
                {
                    let exists = match crate::tool_builders::KillBuilder::new()
                        .signal("-0")
                        .pid(lock_info.pid)
                        .build()
                        .status()
                    {
                        Ok(status) => status.success(),
                        Err(err) => {
                            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                                "checkpoint_lock",
                                &self.lock_file,
                                format!(
                                    "Failed to probe lock owner PID {} via kill -0: {}",
                                    lock_info.pid, err
                                ),
                            );
                            false
                        }
                    };

                    if !exists {
                        crate::media_conversion_gate::delivery_checkpoint_path_audit(
                            "checkpoint_lock",
                            &self.lock_file,
                            format!(
                                "LOCK STALE: PID {} no longer exists, removing",
                                lock_info.pid
                            ),
                        );
                        if let Err(e) = fs::remove_file(&self.lock_file) {
                            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                                "checkpoint_lock",
                                &self.lock_file,
                                format!("Failed to remove stale lock file: {e}"),
                            );
                        }
                        return Ok(None);
                    }

                    match get_process_start_time_for_pid(lock_info.pid) {
                        Some(current_start) if current_start != lock_info.start_time => {
                            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                                "checkpoint_lock",
                                &self.lock_file,
                                format!(
                                    "LOCK STALE: PID {} reused (start time mismatch), removing",
                                    lock_info.pid
                                ),
                            );
                            if let Err(e) = fs::remove_file(&self.lock_file) {
                                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                                    "checkpoint_lock",
                                    &self.lock_file,
                                    format!("Failed to remove stale lock file: {e}"),
                                );
                            }
                            return Ok(None);
                        }
                        Some(_) => {}
                        None => {
                            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                                "checkpoint_lock",
                                &self.lock_file,
                                format!(
                                    "Unable to verify process start time for PID {}; preserving \
                                     active lock",
                                    lock_info.pid
                                ),
                            );
                        }
                    }

                    return Ok(Some(lock_info.pid));
                }

                #[cfg(not(unix))]
                {
                    return Ok(Some(lock_info.pid));
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_lock",
                    &self.lock_file,
                    format!("Failed to parse structured lock JSON; trying legacy PID lock: {e}"),
                );
            }
        }

        match content.trim().parse::<u32>() {
            Ok(pid) => {
                if pid == std::process::id() {
                    crate::media_conversion_gate::delivery_checkpoint_path_audit(
                        "checkpoint_lock",
                        &self.lock_file,
                        "Found existing lock file with own PID; assuming clean reuse",
                    );
                    return Ok(None);
                }
                match fs::metadata(&self.lock_file) {
                    Ok(meta) => match meta.modified() {
                        Ok(modified) => match modified.elapsed() {
                            Ok(elapsed) if elapsed.as_secs() > LOCK_STALE_TIMEOUT_SECS => {
                                if let Err(e) = fs::remove_file(&self.lock_file) {
                                    crate::media_conversion_gate::delivery_checkpoint_path_audit(
                                        "checkpoint_lock",
                                        &self.lock_file,
                                        format!("Failed to remove stale lock file: {e}"),
                                    );
                                }
                                return Ok(None);
                            }
                            Ok(_) => {}
                            Err(e) => {
                                return Err(io::Error::other(format!(
                                    "Failed to compute legacy lock age for {}: {e}",
                                    self.lock_file.display()
                                )));
                            }
                        },
                        Err(e) => {
                            return Err(io::Error::new(
                                e.kind(),
                                format!(
                                    "Failed to read legacy lock modification time from {}: {e}",
                                    self.lock_file.display()
                                ),
                            ));
                        }
                    },
                    Err(e) => {
                        return Err(io::Error::new(
                            e.kind(),
                            format!(
                                "Failed to read legacy lock metadata from {}: {e}",
                                self.lock_file.display()
                            ),
                        ));
                    }
                }
                return Ok(Some(pid));
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_lock",
                    &self.lock_file,
                    format!("LOCK INVALID: Cannot parse lock file as legacy PID: {e}"),
                );
            }
        }

        crate::media_conversion_gate::delivery_checkpoint_path_audit(
            "checkpoint_lock",
            &self.lock_file,
            "LOCK INVALID: Cannot parse lock file, removing",
        );
        if let Err(e) = fs::remove_file(&self.lock_file) {
            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                "checkpoint_lock",
                &self.lock_file,
                format!("Failed to remove invalid lock file: {e}"),
            );
        }
        Ok(None)
    }

    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired or if the lock file
    /// exists and is not stale.
    pub fn acquire_lock(&self) -> io::Result<()> {
        const MAX_LOCK_RETRIES: u32 = crate::constants::LOCK_MAX_RETRIES;
        let lock_info = LockInfo::new()?;
        let json = serde_json::to_string_pretty(&lock_info)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        for _attempt in 0..MAX_LOCK_RETRIES {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.lock_file)
            {
                Ok(mut file) => {
                    file.write_all(json.as_bytes())?;
                    return Ok(());
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    if let Some(pid) = self.check_lock()? {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!("Checkpoint lock already held by PID {pid}"),
                        ));
                    }
                    // Stale lock was cleared; retry create_new
                }
                Err(err) => return Err(err),
            }
        }
        Err(io::Error::other(
            "Failed to acquire checkpoint lock after maximum retries",
        ))
    }

    /// # Errors
    ///
    /// Returns an error if the lock file cannot be removed.
    pub fn release_lock(&self) -> io::Result<()> {
        if self.lock_file.exists() {
            fs::remove_file(&self.lock_file)?;
        }
        Ok(())
    }

    pub fn is_resume_mode(&self) -> bool {
        self.resume_mode.load(Ordering::Relaxed)
    }

    pub fn completed_count(&self) -> usize {
        crate::media_conversion_gate::mutex_guard_or_recover(
            "checkpoint_completed",
            self.completed.lock(),
        )
        .len()
    }

    pub fn is_completed(&self, path: &Path) -> bool {
        let key = Self::normalize_path(path);
        let maybe_entry = {
            let completed = crate::media_conversion_gate::mutex_guard_or_recover(
                "checkpoint_completed",
                self.completed.lock(),
            );
            completed.get(&key).cloned()
        };

        let Some(entry) = maybe_entry else {
            return false;
        };

        match entry.matches_current_file(path) {
            Ok(true) => true,
            Ok(false) => {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_lock",
                    &self.lock_file,
                    format!(
                        "Resume entry became stale after input changed: {}. Reprocessing.",
                        path.display()
                    ),
                );
                if let Err(err) = self.drop_completed_entry(&key) {
                    crate::media_conversion_gate::delivery_checkpoint_path_audit(
                        "checkpoint_lock",
                        &self.lock_file,
                        format!(
                            "Failed to remove stale checkpoint entry for {}: {}. Reprocessing \
                             continues, but checkpoint state may be stale.",
                            path.display(),
                            err
                        ),
                    );
                }
                false
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_fallback",
                    &self.lock_file,
                    format!(
                        "Failed to validate checkpoint entry {}: {}. Reprocessing.",
                        path.display(),
                        err
                    ),
                );
                false
            }
        }
    }

    /// # Errors
    ///
    /// Returns an error if the file metadata cannot be read or if the progress
    /// file cannot be written.
    pub fn mark_completed(&self, path: &Path) -> io::Result<()> {
        let entry = CheckpointEntry::from_path(path)?;
        let key = entry.path.clone();
        {
            let mut completed = crate::media_conversion_gate::mutex_guard_or_recover(
                "checkpoint_completed",
                self.completed.lock(),
            );
            if completed.contains_key(&key) {
                return Ok(());
            }
            completed.insert(key, entry);
        }
        self.resume_mode.store(true, Ordering::Relaxed);
        self.persist_progress()?;

        // Also sync to the global processed list in conversion module
        crate::conversion::mark_as_processed(path);
        Ok(())
    }

    pub fn sync_to_processed_list(&self) {
        let completed = crate::media_conversion_gate::mutex_guard_or_recover(
            "checkpoint_completed",
            self.completed.lock(),
        );
        for path_str in completed.keys() {
            crate::conversion::mark_as_processed(Path::new(path_str));
        }
    }

    /// # Errors
    ///
    /// Returns an error if the progress file cannot be removed.
    pub fn clear_progress(&self) -> io::Result<()> {
        crate::media_conversion_gate::mutex_guard_or_recover(
            "checkpoint_completed",
            self.completed.lock(),
        )
        .clear();
        self.resume_mode.store(false, Ordering::Relaxed);
        crate::mfb_sqlite_store::blob_delete(
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &self.checkpoint_key,
        )
        .map_err(io::Error::other)?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if progress clearing fails.
    pub fn reset_if_output_root_missing(&self, output_root: Option<&Path>) -> io::Result<bool> {
        let Some(output_root) = output_root else {
            return Ok(false);
        };

        if !self.is_resume_mode() || output_root.exists() {
            return Ok(false);
        }

        let completed = self.completed_count();
        crate::media_conversion_gate::delivery_checkpoint_batch_audit(
            "checkpoint_fallback",
            format!(
                "Found {} saved resume entries, but output root {} is missing. Assuming the \
                 optimized folder was intentionally removed; clearing old resume state and \
                 restarting full processing.",
                completed,
                output_root.display()
            ),
        );
        self.clear_progress()?;
        crate::conversion::clear_processed_list();
        Ok(true)
    }

    /// # Errors
    ///
    /// Returns an error if the progress or lock files cannot be removed.
    pub fn cleanup(&self) -> io::Result<()> {
        if let Err(err) = crate::mfb_sqlite_store::blob_delete(
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &self.checkpoint_key,
        ) {
            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                "checkpoint_progress",
                &self.progress_dir,
                format!(
                    "Failed to remove checkpoint blob for key {}: {err}",
                    self.checkpoint_key
                ),
            );
        }
        if self.lock_file.exists()
            && let Err(err) = fs::remove_file(&self.lock_file)
        {
            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                "checkpoint_lock",
                &self.lock_file,
                format!(
                    "Failed to remove lock file {}: {}",
                    self.lock_file.display(),
                    err
                ),
            );
        }
        Ok(())
    }

    fn normalize_path_to_buf(path: &Path) -> PathBuf {
        crate::media_conversion_gate::canonicalize_for_checkpoint_path(path)
    }

    pub fn progress_dir(&self) -> &Path {
        &self.progress_dir
    }

    fn hash_path(path: &Path) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        path.to_string_lossy().hash(&mut hasher);
        format!("{:x}", hasher.finish())[..8].to_string()
    }

    fn normalize_path(path: &Path) -> String {
        match Self::normalize_path_to_buf(path).to_str() {
            Some(s) => s.to_string(),
            None => path.display().to_string(),
        }
    }

    // Rationale: This function handles complex, sequential initialization or
    // business logic where further fragmentation would hinder readability and
    // maintainability.
    fn load_progress(checkpoint_key: &str) -> io::Result<LoadedCheckpointState> {
        let loaded = load_progress_from_sqlite(checkpoint_key)?;
        Ok(match loaded {
            Some(state) => state,
            None => LoadedCheckpointState {
                header: None,
                entries: HashMap::new(),
            },
        })
    }

    fn validate_loaded_state(
        loaded: &LoadedCheckpointState,
        expected_header: &CheckpointHeader,
        output_root: Option<&Path>,
    ) -> (HashMap<String, CheckpointEntry>, bool, Option<String>) {
        let Some(header) = loaded.header.as_ref() else {
            if loaded.entries.is_empty() {
                return (HashMap::new(), false, None);
            }
            return (
                HashMap::new(),
                false,
                Some(
                    "Checkpoint entries were found without a header. Clearing invalid resume \
                     state."
                        .to_string(),
                ),
            );
        };

        if !header.is_compatible_with(expected_header) {
            return (
                HashMap::new(),
                false,
                Some(format!(
                    "Checkpoint context changed (target/output/cache version mismatch). Clearing \
                     stale resume state for {}.",
                    expected_header.target_dir
                )),
            );
        }

        if let Some(output_root) = output_root
            && !output_root.exists()
            && !loaded.entries.is_empty()
        {
            return (
                HashMap::new(),
                false,
                Some(format!(
                    "Found {} saved resume entries, but output root {} is missing. Assuming the \
                     optimized folder was intentionally removed; clearing old resume state and \
                     restarting full processing.",
                    loaded.entries.len(),
                    output_root.display()
                )),
            );
        }

        let mut valid = HashMap::new();
        let mut missing = 0usize;
        let mut changed = 0usize;
        let mut unreadable = 0usize;

        for (path, entry) in &loaded.entries {
            match entry.matches_current_file(Path::new(path)) {
                Ok(true) => {
                    valid.insert(path.clone(), entry.clone());
                }
                Ok(false) => changed += 1,
                Err(err) if err.kind() == io::ErrorKind::NotFound => missing += 1,
                Err(_) => unreadable += 1,
            }
        }

        if valid.is_empty() && (!loaded.entries.is_empty()) {
            return (
                HashMap::new(),
                false,
                Some(format!(
                    "All saved checkpoint entries became invalid during startup validation \
                     (changed: {changed}, missing: {missing}, unreadable: {unreadable}). Clearing \
                     stale resume state."
                )),
            );
        }

        if changed > 0 || missing > 0 || unreadable > 0 {
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_lock",
                format!(
                    "Dropped stale resume entries during validation (changed: {changed}, missing: \
                     {missing}, unreadable: {unreadable})."
                ),
            );
        }

        let resume_mode = !valid.is_empty();
        (valid, resume_mode, None)
    }

    fn persist_progress(&self) -> io::Result<()> {
        let entries: HashMap<String, CheckpointEntry> =
            crate::media_conversion_gate::mutex_guard_or_recover(
                "checkpoint_completed",
                self.completed.lock(),
            )
            .clone();
        save_progress_to_sqlite(&self.checkpoint_key, &self.header, &entries)
    }

    fn drop_completed_entry(&self, key: &str) -> io::Result<()> {
        let became_empty = {
            let mut completed = crate::media_conversion_gate::mutex_guard_or_recover(
                "checkpoint_completed",
                self.completed.lock(),
            );
            completed.remove(key);
            completed.is_empty()
        };
        if became_empty {
            self.resume_mode.store(false, Ordering::Relaxed);
        }
        self.persist_progress()
    }
}

/// Resolve a resume decision before any media work begins.
///
/// Explicit flags win. With saved state and no flags, terminal users are
/// prompted; GUI/non-interactive callers receive a stable error marker and
/// must resubmit with `--resume` or `--no-resume`.
pub fn resolve_resume_choice(
    target_dir: &Path,
    output_root: Option<&Path>,
    resume: bool,
    no_resume: bool,
) -> io::Result<bool> {
    if resume && no_resume {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--resume and --no-resume cannot be used together",
        ));
    }
    if resume {
        return Ok(true);
    }
    if no_resume {
        Manager::discard_saved_progress(target_dir)?;
        return Ok(false);
    }

    let saved = Manager::saved_entry_count(target_dir, output_root)?;
    if saved == 0 {
        return Ok(false);
    }
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(format!(
            "MFB_RESUME_DECISION_REQUIRED: detected {saved} valid saved checkpoint entries; rerun with --resume to continue or --no-resume to restart"
        )));
    }

    loop {
        print!(
            "Detected {saved} completed item(s) from an unfinished task. Continue [r], restart [f], or cancel [c]? "
        );
        io::stdout().flush()?;
        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;
        match choice.trim().to_ascii_lowercase().as_str() {
            "r" | "resume" => return Ok(true),
            "f" | "fresh" => {
                Manager::discard_saved_progress(target_dir)?;
                return Ok(false);
            }
            "c" | "cancel" | "" => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "resume decision cancelled; saved checkpoint was preserved",
                ));
            }
            _ => eprintln!("Choose r (resume), f (fresh), or c (cancel)."),
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        if let Err(err) = self.release_lock() {
            crate::media_conversion_gate::delivery_checkpoint_path_audit(
                "checkpoint_lock",
                &self.lock_file,
                format!(
                    "Failed to release lock on drop {}: {}",
                    self.lock_file.display(),
                    err
                ),
            );
        }
    }
}

/// # Errors
///
/// Returns an error if the output file is missing, empty, or smaller than
/// `min_size`.
pub fn verify_output_integrity(output: &Path, min_size: u64) -> Result<(), String> {
    if !output.exists() {
        return Err("Output file does not exist".to_string());
    }

    let metadata =
        fs::symlink_metadata(output).map_err(|e| format!("Cannot read output metadata: {e}"))?;
    if metadata.file_type().is_symlink() {
        return Err("Output path is a symbolic link".to_string());
    }
    if !metadata.file_type().is_file() {
        return Err("Output path is not a regular file".to_string());
    }

    if metadata.len() == 0 {
        return Err("Output file is empty (0 bytes)".to_string());
    }

    if metadata.len() < min_size {
        return Err(format!(
            "Output file too small: {} < {} bytes",
            metadata.len(),
            min_size
        ));
    }

    let mut file = File::open(output).map_err(|e| format!("Cannot open output file: {e}"))?;

    let mut buffer = [0u8; 16];
    file.read(&mut buffer)
        .map_err(|e| format!("Cannot read output file: {e}"))?;

    Ok(())
}

/// Attempt to safely delete the original source file.
///
/// # Errors
///
/// Returns an error if the source file does not exist, is not a regular file,
/// or is protected.
pub fn safe_delete_original(input: &Path, output: &Path, min_output_size: u64) -> io::Result<()> {
    if let Err(reason) = verify_output_integrity(output, min_output_size) {
        crate::media_conversion_gate::delivery_checkpoint_path_audit(
            "checkpoint_output_integrity",
            input,
            format!("Output integrity check FAILED: {reason}"),
        );
        crate::media_conversion_gate::delivery_checkpoint_path_audit(
            "checkpoint_source_protected",
            input,
            format!("Original file PROTECTED: {}", input.display()),
        );
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Output integrity check failed: {reason}"),
        ));
    }

    if files_alias_same_inode(input, output)? {
        crate::media_conversion_gate::delivery_checkpoint_path_audit(
            "checkpoint_output_integrity",
            input,
            format!(
                "Original file PROTECTED: output aliases source inode (input={}, output={})",
                input.display(),
                output.display()
            ),
        );
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Refusing to delete original because output aliases the same file: {}",
                output.display()
            ),
        ));
    }

    verify_strict_delete_proof(input, output)?;

    let companion_xmp = crate::metadata::find_xmp_sidecar(input);

    fs::remove_file(input)?;

    if let Some(xmp) = companion_xmp
        && let Err(e) = fs::remove_file(&xmp)
    {
        crate::media_conversion_gate::delivery_checkpoint_batch_audit(
            "checkpoint_fallback",
            format!("XMP sidecar cleanup failed for {}: {}", xmp.display(), e),
        );
    }
    Ok(())
}

fn verify_strict_delete_proof(input: &Path, output: &Path) -> io::Result<()> {
    let input_format = crate::image::format_detect::detect_true_format(input).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Cannot detect source format before deleting {}: {err}",
                input.display()
            ),
        )
    })?;
    let output_format = crate::image::format_detect::detect_true_format(output).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Cannot detect output format before deleting {}: {err}",
                output.display()
            ),
        )
    })?;

    if input_format == crate::image::format_detect::FormatKind::Jpeg
        && output_format == crate::image::format_detect::FormatKind::Jxl
    {
        crate::image::fast_img::verify_final_jxl_delivery_integrity(input, output).map_err(
            |err| {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_strict_delete_proof",
                    input,
                    format!(
                        "JPEG→JXL strict delete proof FAILED for output {}: {err}",
                        output.display()
                    ),
                );
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "JPEG→JXL strict delete proof failed for {} -> {}: {err}",
                        input.display(),
                        output.display()
                    ),
                )
            },
        )?;
    }

    Ok(())
}

fn files_alias_same_inode(input: &Path, output: &Path) -> io::Result<bool> {
    if input == output {
        return Ok(true);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let input_meta = fs::metadata(input)?;
        let output_meta = fs::metadata(output)?;
        Ok(input_meta.dev() == output_meta.dev() && input_meta.ino() == output_meta.ino())
    }

    #[cfg(not(unix))]
    {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as std_mutex;
    use tempfile::TempDir;
    static TEST_LOCK: std_mutex<()> = std_mutex::new(());

    fn setup_test_env() -> anyhow::Result<(TempDir, TempDir, std::sync::MutexGuard<'static, ()>)> {
        let guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_target = TempDir::new().map_err(|e| anyhow::anyhow!("target temp dir: {e}"))?;
        let temp_progress =
            TempDir::new().map_err(|e| anyhow::anyhow!("progress temp dir: {e}"))?;
        // SAFETY: Test setup, sequential context.
        unsafe { std::env::set_var("MFB_PROGRESS_DIR", temp_progress.path()) };
        Ok((temp_target, temp_progress, guard))
    }

    fn teardown_test_env(_guard: std::sync::MutexGuard<'static, ()>) {
        // SAFETY: Test teardown, sequential context.
        unsafe { std::env::remove_var("MFB_PROGRESS_DIR") };
    }

    fn create_test_file(path: &Path) -> io::Result<()> {
        fs::write(path, b"checkpoint-test")
    }

    #[test]
    fn test_checkpoint_new_creates_progress_dir() -> anyhow::Result<()> {
        let (target, progress, guard) = setup_test_env()?;
        let checkpoint = Manager::new(target.path())?;
        assert!(checkpoint.progress_dir().exists());
        assert!(progress.path().exists());
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_mark_and_check_completed() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();

        let checkpoint = Manager::new(target)?;

        let file1 = target.join("test1.mp4");
        let file2 = target.join("test2.mp4");
        create_test_file(&file1)?;
        create_test_file(&file2)?;

        assert!(!checkpoint.is_completed(&file1));
        assert!(!checkpoint.is_completed(&file2));

        checkpoint.mark_completed(&file1)?;

        assert!(checkpoint.is_completed(&file1));
        assert!(!checkpoint.is_completed(&file2));
        assert_eq!(checkpoint.completed_count(), 1);

        checkpoint.mark_completed(&file2)?;

        assert!(checkpoint.is_completed(&file1));
        assert!(checkpoint.is_completed(&file2));
        assert_eq!(checkpoint.completed_count(), 2);
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_resume_mode() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();

        {
            let checkpoint = Manager::new(target)?;
            let _ = create_test_file(&target.join("file1.mp4"));
            let _ = create_test_file(&target.join("file2.mp4"));
            checkpoint.mark_completed(&target.join("file1.mp4"))?;
            checkpoint.mark_completed(&target.join("file2.mp4"))?;
        }

        {
            let checkpoint = Manager::new_resuming(target)?;

            assert!(checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 2);
            assert!(checkpoint.is_completed(&target.join("file1.mp4")));
            assert!(checkpoint.is_completed(&target.join("file2.mp4")));
            assert!(!checkpoint.is_completed(&target.join("file3.mp4")));
        }
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn saved_progress_is_inspected_without_consuming_it() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();
        let input = target.join("file1.mp4");
        create_test_file(&input)?;
        {
            let checkpoint = Manager::new(target)?;
            checkpoint.mark_completed(&input)?;
        }

        assert_eq!(Manager::saved_entry_count(target, None)?, 1);
        assert_eq!(Manager::saved_entry_count(target, None)?, 1);
        let decision_error = resolve_resume_choice(target, None, false, false)
            .expect_err("non-interactive caller must choose explicitly");
        assert!(
            decision_error
                .to_string()
                .contains("MFB_RESUME_DECISION_REQUIRED")
        );
        assert_eq!(Manager::saved_entry_count(target, None)?, 1);
        Manager::discard_saved_progress(target)?;
        assert_eq!(Manager::saved_entry_count(target, None)?, 0);
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_new_ignores_saved_resume_state_by_default() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();
        let input = target.join("file1.mp4");
        create_test_file(&input)?;

        {
            let checkpoint = Manager::new(target)?;
            checkpoint.mark_completed(&input)?;
            assert!(checkpoint.is_resume_mode());
        }

        let checkpoint = Manager::new(target)?;

        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
        assert!(!checkpoint.is_completed(&input));
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_clear_progress() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();

        let checkpoint = Manager::new(target)?;
        create_test_file(&target.join("file1.mp4"))?;
        create_test_file(&target.join("file2.mp4"))?;
        checkpoint.mark_completed(&target.join("file1.mp4"))?;
        checkpoint.mark_completed(&target.join("file2.mp4"))?;

        assert_eq!(checkpoint.completed_count(), 2);

        checkpoint.clear_progress()?;
        assert_eq!(checkpoint.completed_count(), 0);
        assert!(!checkpoint.is_completed(&target.join("file1.mp4")));
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_mark_completed_enables_resume_mode_for_current_run() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();

        let checkpoint = Manager::new(target)?;
        assert!(!checkpoint.is_resume_mode());
        create_test_file(&target.join("file1.mp4"))?;

        checkpoint.mark_completed(&target.join("file1.mp4"))?;

        assert!(checkpoint.is_resume_mode());
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_reset_if_output_root_missing_clears_stale_resume_state() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();
        let missing_output = target.join("deleted_optimized");

        {
            let checkpoint = Manager::new(target)?;
            let _ = create_test_file(&target.join("file1.mp4"));
            let _ = create_test_file(&target.join("file2.mp4"));
            checkpoint.mark_completed(&target.join("file1.mp4"))?;
            checkpoint.mark_completed(&target.join("file2.mp4"))?;
        }

        let checkpoint = Manager::new_resuming(target)?;
        assert!(checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 2);

        let cleared = checkpoint.reset_if_output_root_missing(Some(&missing_output))?;

        assert!(cleared);
        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
        assert!(
            crate::mfb_sqlite_store::blob_get(
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                &checkpoint.checkpoint_key,
                CHECKPOINT_BLOB_SCHEMA,
            )
            .expect("blob_get must not fail in test")
            .is_none(),
            "checkpoint SQLite blob should be cleared after output-root reset"
        );
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_new_with_context_isolation_by_output_root() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();
        let output_a = target.join("optimized_a");
        let output_b = target.join("optimized_b");
        let input = target.join("file1.mp4");
        create_test_file(&input)?;

        {
            let checkpoint = Manager::new_with_context(target, Some(&output_a))?;
            checkpoint.mark_completed(&input)?;
        }

        let checkpoint = Manager::new_resuming_with_context(target, Some(&output_b))?;
        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_new_with_context_drops_entries_when_input_signature_changes() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();
        let output = target.join("optimized");
        let input = target.join("file1.mp4");
        fs::write(&input, b"aaaaaaaaaaaaaaa")?;

        {
            let checkpoint = Manager::new_with_context(target, Some(&output))?;
            checkpoint.mark_completed(&input)?;
        }

        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(&input, b"bbbbbbbbbbbbbbb")?;

        let checkpoint = Manager::new_resuming_with_context(target, Some(&output))?;
        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
        assert!(!checkpoint.is_completed(&input));
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_cleanup() -> anyhow::Result<()> {
        let temp_target = TempDir::new().map_err(|e| anyhow::anyhow!("temp dir: {e}"))?;
        let target = temp_target.path();

        let (progress_temp, _, guard) = setup_test_env()?;

        {
            let checkpoint = Manager::new(target)?;
            checkpoint.acquire_lock()?;
            create_test_file(&target.join("file1.mp4"))?;
            checkpoint.mark_completed(&target.join("file1.mp4"))?;

            checkpoint.cleanup()?;
        }

        assert!(!progress_temp.path().join("completed.txt").exists());
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_lock_acquire_release() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();

        let checkpoint = Manager::new(target)?;

        assert!(checkpoint.check_lock()?.is_none());

        checkpoint.acquire_lock()?;
        assert!(checkpoint.lock_file.exists());

        checkpoint.release_lock()?;
        assert!(!checkpoint.lock_file.exists());
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_verify_output_integrity_valid_file() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let output = temp.path().join("output.mp4");

        fs::write(&output, b"This is test content for integrity check")?;

        assert!(verify_output_integrity(&output, 10).is_ok());
        Ok(())
    }

    #[test]
    fn test_verify_output_integrity_empty_file() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let output = temp.path().join("empty.mp4");

        fs::write(&output, b"")?;

        let result = verify_output_integrity(&output, 10);
        assert!(result.is_err());
        assert!(
            result
                .expect_err("empty file should produce an error")
                .contains("empty")
        );
        Ok(())
    }

    #[test]
    fn test_verify_output_integrity_too_small() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let output = temp.path().join("small.mp4");

        fs::write(&output, b"tiny")?;

        let result = verify_output_integrity(&output, 100);
        assert!(result.is_err());
        assert!(
            result
                .expect_err("small file should produce an error")
                .contains("too small")
        );
        Ok(())
    }

    #[test]
    fn test_verify_output_integrity_nonexistent() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let output = temp.path().join("nonexistent.mp4");

        let result = verify_output_integrity(&output, 10);
        assert!(result.is_err());
        assert!(
            result
                .expect_err("missing file should produce an error")
                .contains("does not exist")
        );
        Ok(())
    }

    #[test]
    fn test_safe_delete_original_success() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("output.mp4");

        fs::write(&input, b"original content")?;
        fs::write(&output, b"converted content that is valid")?;

        assert!(safe_delete_original(&input, &output, 10).is_ok());

        assert!(!input.exists());
        assert!(output.exists());
        Ok(())
    }

    #[test]
    fn test_safe_delete_original_protects_on_invalid_output() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("output.mp4");

        fs::write(&input, b"original content")?;
        fs::write(&output, b"")?;

        assert!(safe_delete_original(&input, &output, 10).is_err());

        assert!(input.exists());
        Ok(())
    }

    #[test]
    fn test_safe_delete_original_protects_on_missing_output() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("nonexistent.mp4");

        fs::write(&input, b"original content")?;

        assert!(safe_delete_original(&input, &output, 10).is_err());

        assert!(input.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_safe_delete_original_protects_on_hardlink_alias() -> anyhow::Result<()> {
        use std::fs::hard_link;

        let temp = TempDir::new()?;
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("output.mp4");
        fs::write(&input, b"hello world")?;
        hard_link(&input, &output)?;

        let err = safe_delete_original(&input, &output, 4)
            .err()
            .ok_or_else(|| anyhow::anyhow!("hardlink alias should be rejected"))?;
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(input.exists(), "source should remain protected");
        Ok(())
    }

    #[test]
    fn test_safe_delete_original_protects_jpeg_source_when_jxl_proof_invalid() -> anyhow::Result<()>
    {
        let temp = TempDir::new()?;
        let input = temp.path().join("input.jpg");
        let output = temp.path().join("output.jxl");
        fs::write(&input, [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10])?;
        fs::write(
            &output,
            [
                0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A, 0x00, 0x00,
                0x00, 0x00,
            ],
        )?;

        let err = safe_delete_original(&input, &output, 10)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid JXL proof must block source deletion"))?;

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(input.exists(), "source should remain protected");
        Ok(())
    }

    #[test]
    fn test_full_workflow_with_interruption() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();

        let files: Vec<PathBuf> = (1_i32..=5_i32)
            .map(|i| {
                let path = target.join(format!("video{i}.mp4"));
                fs::write(&path, format!("content {i}"))?;
                Ok(path)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        {
            let checkpoint = Manager::new(target)?;
            checkpoint.acquire_lock()?;

            for file in files.iter().take(2) {
                checkpoint.mark_completed(file)?;
            }

            checkpoint.release_lock()?;
        }

        {
            let checkpoint = Manager::new_resuming(target)?;

            assert!(checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 2);

            checkpoint.acquire_lock()?;

            let mut processed = 0_i32;
            let mut skipped = 0_i32;

            for file in &files {
                if checkpoint.is_completed(file) {
                    skipped += 1_i32;
                    continue;
                }
                checkpoint.mark_completed(file)?;
                processed += 1_i32;
            }

            assert_eq!(skipped, 2_i32);
            assert_eq!(processed, 3_i32);
            assert_eq!(checkpoint.completed_count(), 5);

            checkpoint.cleanup()?;
        }

        {
            let checkpoint = Manager::new(target)?;
            assert!(!checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 0);
        }
        teardown_test_env(guard);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_ps_etime_to_secs_short_format() {
        assert_eq!(parse_ps_etime_to_secs("03:15"), Some(195));
        assert_eq!(parse_ps_etime_to_secs("01:02:03"), Some(3723));
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_ps_etime_to_secs_day_format() {
        assert_eq!(parse_ps_etime_to_secs("2-03:04:05"), Some(183_845));
        assert_eq!(parse_ps_etime_to_secs(""), None);
    }
}
