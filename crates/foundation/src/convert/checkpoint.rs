//! Checkpoint & Resume Module (Progress Tracking)
//!
//! Provides atomic operation protection and resume capability for all
//! conversion tools:
//! - Progress tracking: Record completed files for resume after interruption
//! - Atomic delete: Verify output integrity before deleting original
//! - Kernel file ownership: Prevent concurrent checkpoint access for the same directory
//!
//! Construction acquires exclusive ownership before loading or changing saved state.
//! Release disables state access until reacquisition reloads the latest committed state.
//! Lock files remain in the central directory; closing the owned handle releases the lock.
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

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
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
/// Current version of the checkpoint file format.
///
/// Increment this when the checkpoint structure changes to invalidate
/// old checkpoint files and force regeneration.
const CHECKPOINT_FORMAT_VERSION: u32 = crate::constants::CHECKPOINT_FORMAT_VERSION;

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
    /// Last modification time in Unix nanoseconds.
    mtime: i64,
    /// Unix status-change time in nanoseconds, or Windows last-write ticks.
    ctime: i64,
    /// Birth time in Unix nanoseconds (if available).
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
            crate::numeric_cast::unix_parts_to_nanos(metadata.ctime(), metadata.ctime_nsec())?
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
        // Cache v5 changes database integrity tags only; v4 checkpoint entries
        // retain the same format and validation semantics.
        self.format_version == expected.format_version
            && self.target_dir == expected.target_dir
            && self.output_root == expected.output_root
            && self.cache_algorithm_version == expected.cache_algorithm_version
            && (self.cache_schema_version == expected.cache_schema_version
                || (self.cache_schema_version == 4 && expected.cache_schema_version == 5))
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
const LEGACY_CHECKPOINT_BLOB_SCHEMA: i32 = 2;
const CHECKPOINT_BLOB_SCHEMA: i32 = 3;

fn checkpoint_entry_key(key: &str, path: &str) -> String {
    format!("{key}/{}", blake3::hash(path.as_bytes()).to_hex())
}

fn checkpoint_transaction<R>(
    behavior: TransactionBehavior,
    operation: impl FnOnce(&rusqlite::Transaction<'_>) -> anyhow::Result<R>,
) -> io::Result<R> {
    (|| -> anyhow::Result<R> {
        let mut connection = crate::mfb_sqlite_store::open_store_connection()?;
        let transaction = connection.transaction_with_behavior(behavior)?;
        let value = operation(&transaction)?;
        transaction.commit()?;
        Ok(value)
    })()
    .map_err(io::Error::other)
}

fn checkpoint_entry_keys(
    connection: &rusqlite::Connection,
    key: &str,
) -> anyhow::Result<Vec<String>> {
    let prefix = format!("{key}/");
    let mut statement = connection.prepare(
        "SELECT cache_key FROM blob_store WHERE namespace = ?1
         AND substr(cache_key, 1, length(?2)) = ?2 ORDER BY cache_key",
    )?;
    let rows = statement.query_map(
        params![crate::mfb_sqlite_store::NS_CHECKPOINT, prefix],
        |row| row.get(0),
    )?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn load_progress_from_sqlite(key: &str) -> io::Result<Option<LoadedCheckpointState>> {
    checkpoint_transaction(TransactionBehavior::Deferred, |transaction| {
        let schema: Option<i32> = transaction
            .query_row(
                "SELECT schema_version FROM blob_store WHERE namespace = ?1 AND cache_key = ?2",
                params![crate::mfb_sqlite_store::NS_CHECKPOINT, key],
                |row| row.get(0),
            )
            .optional()?;
        let entry_keys = checkpoint_entry_keys(transaction, key)?;
        let Some(schema) = schema else {
            anyhow::ensure!(
                entry_keys.is_empty(),
                "checkpoint entries have no header; state retained"
            );
            return Ok(None);
        };
        anyhow::ensure!(
            matches!(
                schema,
                LEGACY_CHECKPOINT_BLOB_SCHEMA | CHECKPOINT_BLOB_SCHEMA
            ),
            "unsupported checkpoint SQLite schema {schema}; state retained"
        );
        let bytes = crate::mfb_sqlite_store::read_blob(
            transaction,
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            key,
            schema,
        )?
        .ok_or_else(|| anyhow::anyhow!("checkpoint header disappeared from snapshot"))?;
        if schema == LEGACY_CHECKPOINT_BLOB_SCHEMA {
            anyhow::ensure!(
                entry_keys.is_empty(),
                "legacy checkpoint has unexpected entry rows; state retained"
            );
            let blob: CheckpointBlob = serde_json::from_slice(&bytes)?;
            return Ok(Some(LoadedCheckpointState {
                header: Some(blob.header),
                entries: blob.entries,
            }));
        }
        let header: CheckpointHeader = serde_json::from_slice(&bytes)?;
        let mut entries = HashMap::new();
        for entry_key in entry_keys {
            let payload = crate::mfb_sqlite_store::read_blob(
                transaction,
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                &entry_key,
                CHECKPOINT_BLOB_SCHEMA,
            )?
            .ok_or_else(|| anyhow::anyhow!("checkpoint entry disappeared from snapshot"))?;
            let entry: CheckpointEntry = serde_json::from_slice(&payload)?;
            anyhow::ensure!(
                entry_key == checkpoint_entry_key(key, &entry.path),
                "checkpoint entry identity mismatch; state retained"
            );
            anyhow::ensure!(
                entries.insert(entry.path.clone(), entry).is_none(),
                "duplicate checkpoint source identity; state retained"
            );
        }
        Ok(Some(LoadedCheckpointState {
            header: Some(header),
            entries,
        }))
    })
}

fn save_progress_to_sqlite(
    key: &str,
    header: &CheckpointHeader,
    entries: &HashMap<String, CheckpointEntry>,
    superseded_key: Option<&str>,
) -> io::Result<()> {
    checkpoint_transaction(TransactionBehavior::Immediate, |transaction| {
        delete_checkpoint_rows(transaction, key)?;
        if let Some(old_key) = superseded_key.filter(|old_key| *old_key != key) {
            delete_checkpoint_rows(transaction, old_key)?;
        }
        let updated_at = i64::try_from(current_unix_secs()?)?;
        crate::mfb_sqlite_store::upsert_blob(
            transaction,
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            key,
            CHECKPOINT_BLOB_SCHEMA,
            None,
            &serde_json::to_vec(header)?,
            updated_at,
        )?;
        for entry in entries.values() {
            crate::mfb_sqlite_store::upsert_blob(
                transaction,
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                &checkpoint_entry_key(key, &entry.path),
                CHECKPOINT_BLOB_SCHEMA,
                None,
                &serde_json::to_vec(entry)?,
                updated_at,
            )?;
        }
        Ok(())
    })
}

fn delete_checkpoint_rows(connection: &rusqlite::Connection, key: &str) -> anyhow::Result<usize> {
    let prefix = format!("{key}/");
    Ok(connection.execute(
        "DELETE FROM blob_store WHERE namespace = ?1
         AND (cache_key = ?2 OR substr(cache_key, 1, length(?3)) = ?3)",
        params![crate::mfb_sqlite_store::NS_CHECKPOINT, key, prefix],
    )?)
}

fn delete_progress_from_sqlite(key: &str) -> io::Result<usize> {
    checkpoint_transaction(TransactionBehavior::Immediate, |transaction| {
        delete_checkpoint_rows(transaction, key)
    })
}

fn save_checkpoint_entry(
    key: &str,
    header: &CheckpointHeader,
    entry: &CheckpointEntry,
) -> io::Result<()> {
    checkpoint_transaction(TransactionBehavior::Immediate, |transaction| {
        let updated_at = i64::try_from(current_unix_secs()?)?;
        if let Some(saved) = crate::mfb_sqlite_store::read_blob(
            transaction,
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            key,
            CHECKPOINT_BLOB_SCHEMA,
        )? {
            let current: CheckpointHeader = serde_json::from_slice(&saved)?;
            anyhow::ensure!(
                current.is_compatible_with(header),
                "checkpoint header changed; state retained"
            );
        } else {
            anyhow::ensure!(
                checkpoint_entry_keys(transaction, key)?.is_empty(),
                "checkpoint entries have no header; state retained"
            );
            crate::mfb_sqlite_store::upsert_blob(
                transaction,
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                key,
                CHECKPOINT_BLOB_SCHEMA,
                None,
                &serde_json::to_vec(header)?,
                updated_at,
            )?;
        }
        let row_key = checkpoint_entry_key(key, &entry.path);
        if let Some(saved) = crate::mfb_sqlite_store::read_blob(
            transaction,
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &row_key,
            CHECKPOINT_BLOB_SCHEMA,
        )? {
            let current: CheckpointEntry = serde_json::from_slice(&saved)?;
            anyhow::ensure!(
                current.path == entry.path,
                "checkpoint entry identity changed; state retained"
            );
        }
        crate::mfb_sqlite_store::upsert_blob(
            transaction,
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &row_key,
            CHECKPOINT_BLOB_SCHEMA,
            None,
            &serde_json::to_vec(entry)?,
            updated_at,
        )?;
        Ok(())
    })
}

fn delete_checkpoint_entry(
    key: &str,
    header: &CheckpointHeader,
    entry: &CheckpointEntry,
) -> io::Result<()> {
    checkpoint_transaction(TransactionBehavior::Immediate, |transaction| {
        let saved_header = crate::mfb_sqlite_store::read_blob(
            transaction,
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            key,
            CHECKPOINT_BLOB_SCHEMA,
        )?
        .ok_or_else(|| anyhow::anyhow!("checkpoint header is missing; state retained"))?;
        let saved_header: CheckpointHeader = serde_json::from_slice(&saved_header)?;
        anyhow::ensure!(
            saved_header.is_compatible_with(header),
            "checkpoint header changed; state retained"
        );
        let row_key = checkpoint_entry_key(key, &entry.path);
        let saved = crate::mfb_sqlite_store::read_blob(
            transaction,
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &row_key,
            CHECKPOINT_BLOB_SCHEMA,
        )?
        .ok_or_else(|| anyhow::anyhow!("checkpoint entry is missing; state retained"))?;
        let current: CheckpointEntry = serde_json::from_slice(&saved)?;
        anyhow::ensure!(
            current == *entry,
            "checkpoint entry changed; state retained"
        );
        let deleted = transaction.execute(
            "DELETE FROM blob_store WHERE namespace = ?1 AND cache_key = ?2",
            params![crate::mfb_sqlite_store::NS_CHECKPOINT, row_key],
        )?;
        anyhow::ensure!(deleted == 1, "checkpoint entry was not deleted");
        Ok(())
    })
}

fn open_checkpoint_lock(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        // Keep the path stable while its ownership handle remains open.
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options.open(path)?;
    validate_checkpoint_lock_path(&file, path)?;
    Ok(file)
}

fn validate_checkpoint_lock_path(file: &File, path: &Path) -> io::Result<()> {
    let opened = file.metadata()?;
    let current = fs::symlink_metadata(path)?;
    if !opened.is_file() || !current.is_file() || current.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "checkpoint lock must be a regular file, not a symbolic link",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != current.dev() || opened.ino() != current.ino() || opened.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "checkpoint lock path changed or has multiple hard links; state retained",
            ));
        }
    }
    Ok(())
}

fn acquire_checkpoint_lock(path: &Path) -> io::Result<File> {
    let file = open_checkpoint_lock(path)?;
    file.try_lock().map_err(|error| {
        let error: io::Error = error.into();
        io::Error::new(
            error.kind(),
            format!("acquire checkpoint lock {}: {error}", path.display()),
        )
    })?;
    validate_checkpoint_lock_path(&file, path)?;
    Ok(file)
}

pub struct Manager {
    progress_dir: PathBuf,
    lock_file: PathBuf,
    owned_lock: Mutex<Option<File>>,
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
        let (loaded, _) = Self::load_progress_for_path(&canonical_target, &checkpoint_key)?;
        let (valid, _, _) = Self::validate_loaded_state(&loaded, &header, output_root);
        Ok(valid.len())
    }

    /// Explicitly discard the saved checkpoint for a user-requested fresh run.
    pub fn discard_saved_progress(target_dir: &Path) -> io::Result<()> {
        let canonical_target = Self::normalize_path_to_buf(target_dir);
        let checkpoint_key = Self::hash_path(&canonical_target);
        let progress = get_central_progress_dir()?;
        fs::create_dir_all(&progress)?;
        let _owned_lock =
            acquire_checkpoint_lock(&progress.join(format!("{checkpoint_key}.lock")))?;
        Self::discard_progress_for_fresh_run(&checkpoint_key)?;
        let legacy_key = Self::legacy_hash_path(&canonical_target);
        if legacy_key != checkpoint_key {
            Self::discard_progress_for_fresh_run(&legacy_key)?;
        }
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
        let legacy_key = Self::legacy_hash_path(&canonical_target);
        let header = CheckpointHeader::new(&canonical_target, output_root)?;

        let central_dir = get_central_progress_dir()?;
        fs::create_dir_all(&central_dir)?;

        let checkpoint_key = dir_hash;
        let lock_file = central_dir.join(format!("{checkpoint_key}.lock"));
        let owned_lock = acquire_checkpoint_lock(&lock_file)?;

        let (completed_set, resume_mode, reset_reason, loaded_legacy_key, persist_loaded_state) =
            if resume_enabled {
                let (loaded, loaded_legacy_key) =
                    Self::load_progress_for_path(&canonical_target, &checkpoint_key)?;
                let (completed_set, resume_mode, reset_reason) =
                    Self::validate_loaded_state(&loaded, &header, output_root);
                let persist_loaded_state = loaded.header.is_some() && reset_reason.is_none();
                (
                    completed_set,
                    resume_mode,
                    reset_reason,
                    loaded_legacy_key,
                    persist_loaded_state,
                )
            } else {
                Self::discard_progress_for_fresh_run(&checkpoint_key)?;
                if legacy_key != checkpoint_key {
                    Self::discard_progress_for_fresh_run(&legacy_key)?;
                }
                crate::conversion::clear_processed_list();
                (HashMap::new(), false, None, None, false)
            };

        if let Some(reason) = reset_reason.as_deref() {
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_fallback",
                reason,
            );
            let stale_key = loaded_legacy_key.as_deref().unwrap_or(&checkpoint_key);
            if let Err(err) = delete_progress_from_sqlite(stale_key) {
                let message = format!(
                    "Failed to remove invalidated checkpoint blob for key {stale_key}: {err}"
                );
                crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                    "checkpoint_progress",
                    &message,
                );
                return Err(io::Error::other(message));
            }
        }

        // Keep the owned handle alive across validation and every required commit.
        if persist_loaded_state
            && let Err(err) = save_progress_to_sqlite(
                &checkpoint_key,
                &header,
                &completed_set,
                loaded_legacy_key.as_deref(),
            )
        {
            let message = format!("Failed to compact validated checkpoint state: {err}");
            crate::media_conversion_gate::delivery_checkpoint_batch_audit(
                "checkpoint_progress",
                &message,
            );
            return Err(io::Error::other(message));
        }

        Ok(Self {
            progress_dir: central_dir,
            lock_file,
            owned_lock: Mutex::new(Some(owned_lock)),
            checkpoint_key,
            header,
            completed: Mutex::new(completed_set),
            resume_mode: AtomicBool::new(resume_mode),
        })
    }

    fn discard_progress_for_fresh_run(checkpoint_key: &str) -> io::Result<()> {
        match delete_progress_from_sqlite(checkpoint_key) {
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

    /// # Errors
    ///
    /// Reports whether another handle owns the lock, without guessing its PID.
    /// Returns an error if lock ownership or the lock path cannot be checked.
    pub fn check_lock(&self) -> io::Result<bool> {
        let owned = self
            .owned_lock
            .lock()
            .map_err(|_| io::Error::other("checkpoint ownership mutex poisoned; state retained"))?;
        if let Some(file) = owned.as_ref() {
            validate_checkpoint_lock_path(file, &self.lock_file)?;
            return Ok(false);
        }
        let file = open_checkpoint_lock(&self.lock_file)?;
        let result = match file.try_lock() {
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(error)) => Err(error),
        };
        drop(owned);
        result
    }

    /// # Errors
    ///
    /// Reacquires a released lock and reloads current durable state. An already
    /// owned lock is unchanged. Incompatible state is retained and rejected.
    pub fn acquire_lock(&self) -> io::Result<()> {
        let mut owned = self
            .owned_lock
            .lock()
            .map_err(|_| io::Error::other("checkpoint ownership mutex poisoned; state retained"))?;
        if let Some(file) = owned.as_ref() {
            return validate_checkpoint_lock_path(file, &self.lock_file);
        }
        let file = acquire_checkpoint_lock(&self.lock_file)?;
        let (loaded, loaded_legacy_key) =
            Self::load_progress_for_path(Path::new(&self.header.target_dir), &self.checkpoint_key)?;
        let (completed, resume_mode, reset_reason) = Self::validate_loaded_state(
            &loaded,
            &self.header,
            self.header.output_root.as_deref().map(Path::new),
        );
        if let Some(reason) = reset_reason {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("cannot reuse released checkpoint manager: {reason}"),
            ));
        }
        if loaded.header.is_some() {
            save_progress_to_sqlite(
                &self.checkpoint_key,
                &self.header,
                &completed,
                loaded_legacy_key.as_deref(),
            )?;
        }
        *crate::media_conversion_gate::mutex_guard_or_recover(
            "checkpoint_completed",
            self.completed.lock(),
        ) = completed;
        self.resume_mode.store(resume_mode, Ordering::Relaxed);
        *owned = Some(file);
        drop(owned);
        Ok(())
    }

    /// # Errors
    ///
    /// Releases only this manager's ownership handle. The fixed lock file remains
    /// so competing openers cannot acquire different inodes for the same key.
    /// Returns an error if ownership cannot be released.
    pub fn release_lock(&self) -> io::Result<()> {
        let mut owned = self
            .owned_lock
            .lock()
            .map_err(|_| io::Error::other("checkpoint ownership mutex poisoned; state retained"))?;
        if let Some(file) = owned.as_ref() {
            file.unlock()?;
        }
        owned.take();
        drop(owned);
        Ok(())
    }

    fn require_lock(&self) -> io::Result<MutexGuard<'_, Option<File>>> {
        let owned = self
            .owned_lock
            .lock()
            .map_err(|_| io::Error::other("checkpoint ownership mutex poisoned; state retained"))?;
        let file = owned.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "checkpoint ownership was released; reacquire before accessing saved state",
            )
        })?;
        validate_checkpoint_lock_path(file, &self.lock_file)?;
        Ok(owned)
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
        let owned = match self.require_lock() {
            Ok(owned) => owned,
            Err(error) => {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_lock",
                    path,
                    format!("Cannot reuse checkpoint without ownership: {error}"),
                );
                return false;
            }
        };
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
                // The helper rechecks ownership after releasing this mutex, not the file lock.
                drop(owned);
                if let Err(err) = self.drop_completed_entry(&key, &entry) {
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
        let _owned = self.require_lock()?;
        let entry = CheckpointEntry::from_path(path)?;
        let key = entry.path.clone();
        let mut completed = crate::media_conversion_gate::mutex_guard_or_recover(
            "checkpoint_completed",
            self.completed.lock(),
        );
        if completed.get(&key) == Some(&entry) {
            return Ok(());
        }
        save_checkpoint_entry(&self.checkpoint_key, &self.header, &entry)?;
        completed.insert(key, entry);
        self.resume_mode.store(true, Ordering::Relaxed);

        // Also sync to the global processed list in conversion module
        crate::conversion::mark_as_processed(path);
        drop(completed);
        Ok(())
    }

    pub fn sync_to_processed_list(&self) {
        let _owned = match self.require_lock() {
            Ok(owned) => owned,
            Err(error) => {
                crate::media_conversion_gate::delivery_checkpoint_path_audit(
                    "checkpoint_lock",
                    &self.lock_file,
                    format!("Cannot sync checkpoint without ownership: {error}"),
                );
                return;
            }
        };
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
        let _owned = self.require_lock()?;
        self.clear_progress_locked()
    }

    // Callers retain the ownership mutex until the committed state is applied.
    fn clear_progress_locked(&self) -> io::Result<()> {
        let mut completed = crate::media_conversion_gate::mutex_guard_or_recover(
            "checkpoint_completed",
            self.completed.lock(),
        );
        delete_progress_from_sqlite(&self.checkpoint_key)?;
        completed.clear();
        self.resume_mode.store(false, Ordering::Relaxed);
        drop(completed);
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if progress clearing fails.
    pub fn reset_if_output_root_missing(&self, output_root: Option<&Path>) -> io::Result<bool> {
        let Some(output_root) = output_root else {
            return Ok(false);
        };
        let _owned = self.require_lock()?;

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
        self.clear_progress_locked()?;
        crate::conversion::clear_processed_list();
        Ok(true)
    }

    /// # Errors
    ///
    /// Clears progress and releases this manager's handle under one ownership guard.
    /// Returns an error if clearing progress or releasing ownership fails.
    pub fn cleanup(&self) -> io::Result<()> {
        let mut owned = self.require_lock()?;
        self.clear_progress_locked()?;
        if let Some(file) = owned.as_ref() {
            file.unlock()?;
        }
        owned.take();
        drop(owned);
        Ok(())
    }

    fn normalize_path_to_buf(path: &Path) -> PathBuf {
        crate::media_conversion_gate::canonicalize_for_checkpoint_path(path)
    }

    pub fn progress_dir(&self) -> &Path {
        &self.progress_dir
    }

    fn hash_path(path: &Path) -> String {
        blake3::hash(path.to_string_lossy().as_bytes())
            .to_hex()
            .to_string()
    }

    /// Read-only compatibility key for checkpoints written before BLAKE3 keys.
    fn legacy_hash_path(path: &Path) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        path.to_string_lossy().hash(&mut hasher);
        format!("{:x}", hasher.finish())[..8].to_string()
    }

    fn load_progress_for_path(
        path: &Path,
        checkpoint_key: &str,
    ) -> io::Result<(LoadedCheckpointState, Option<String>)> {
        if let Some(loaded) = load_progress_from_sqlite(checkpoint_key)? {
            return Ok((loaded, None));
        }
        let legacy_key = Self::legacy_hash_path(path);
        if let Some(loaded) = load_progress_from_sqlite(&legacy_key)? {
            return Ok((loaded, Some(legacy_key)));
        }
        Ok((
            LoadedCheckpointState {
                header: None,
                entries: HashMap::new(),
            },
            None,
        ))
    }

    fn normalize_path(path: &Path) -> String {
        match Self::normalize_path_to_buf(path).to_str() {
            Some(s) => s.to_string(),
            None => path.display().to_string(),
        }
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

    fn drop_completed_entry(&self, key: &str, expected: &CheckpointEntry) -> io::Result<()> {
        let _owned = self.require_lock()?;
        let mut completed = crate::media_conversion_gate::mutex_guard_or_recover(
            "checkpoint_completed",
            self.completed.lock(),
        );
        if completed.get(key) != Some(expected) {
            return Ok(());
        }
        delete_checkpoint_entry(&self.checkpoint_key, &self.header, expected)?;
        completed.remove(key);
        self.resume_mode
            .store(!completed.is_empty(), Ordering::Relaxed);
        drop(completed);
        Ok(())
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
    resolve_resume_choice_for_terminal(
        target_dir,
        output_root,
        resume,
        no_resume,
        io::stdin().is_terminal(),
    )
}

fn resolve_resume_choice_for_terminal(
    target_dir: &Path,
    output_root: Option<&Path>,
    resume: bool,
    no_resume: bool,
    stdin_is_terminal: bool,
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
    if !stdin_is_terminal {
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
    if let Some(xmp) = &companion_xmp
        && crate::image::format_detect::detect_true_format(output)
            .map_err(|error| io::Error::other(error.to_string()))?
            == crate::image::format_detect::FormatKind::Jxl
    {
        crate::metadata::verify_jxl_xmp_sidecar_custody(xmp, output)?;
    }

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

pub(crate) fn files_alias_same_inode(input: &Path, output: &Path) -> io::Result<bool> {
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
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn checkpoint_keeps_full_status_change_time() {
        use std::os::unix::fs::MetadataExt;
        let file = tempfile::NamedTempFile::new().unwrap();
        let metadata = file.as_file().metadata().unwrap();
        let entry = CheckpointEntry::from_path(file.path()).unwrap();
        assert_eq!(
            i128::from(entry.ctime),
            i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec())
        );
    }

    struct TestEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        _store: crate::mfb_sqlite_store::TestStoreGuard,
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            // SAFETY: The test environment lock remains held until this guard is dropped.
            unsafe { std::env::remove_var("MFB_PROGRESS_DIR") };
        }
    }

    fn setup_test_env() -> anyhow::Result<(TempDir, TempDir, TestEnvGuard)> {
        let guard = crate::conversion::TEST_PROCESSED_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_target = TempDir::new().map_err(|e| anyhow::anyhow!("target temp dir: {e}"))?;
        let temp_progress =
            TempDir::new().map_err(|e| anyhow::anyhow!("progress temp dir: {e}"))?;
        // SAFETY: Test setup, sequential context.
        unsafe { std::env::set_var("MFB_PROGRESS_DIR", temp_progress.path()) };
        let store = crate::mfb_sqlite_store::set_test_store_path_for_tests(
            temp_progress.path().join("mfb_store.sqlite"),
        );
        Ok((
            temp_target,
            temp_progress,
            TestEnvGuard {
                _lock: guard,
                _store: store,
            },
        ))
    }

    fn teardown_test_env(_guard: TestEnvGuard) {}

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
    fn checkpoint_write_and_delete_failures_leave_both_states_unchanged() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let input = temp.path().join("saved.mp4");
        create_test_file(&input)?;
        let manager = Manager::new(temp.path())?;
        let conn = crate::mfb_sqlite_store::open_store_connection()?;
        conn.execute_batch(
            "CREATE TRIGGER reject_checkpoint_write BEFORE INSERT ON blob_store
             WHEN NEW.namespace = 'checkpoint' BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;",
        )?;
        for _ in 0..2 {
            assert!(manager.mark_completed(&input).is_err());
            assert_eq!(manager.completed_count(), 0);
            assert!(!manager.is_resume_mode());
            assert!(!crate::conversion::is_already_processed(&input));
            assert!(load_progress_from_sqlite(&manager.checkpoint_key)?.is_none());
        }
        conn.execute_batch("DROP TRIGGER reject_checkpoint_write;")?;
        manager.mark_completed(&input)?;
        let saved = load_progress_from_sqlite(&manager.checkpoint_key)?
            .unwrap()
            .entries;
        conn.execute_batch(
            "CREATE TRIGGER reject_checkpoint_write BEFORE INSERT ON blob_store
             WHEN NEW.namespace = 'checkpoint' BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;
             CREATE TRIGGER reject_checkpoint_delete BEFORE DELETE ON blob_store
             WHEN OLD.namespace = 'checkpoint' BEGIN SELECT RAISE(ABORT, 'fixture delete failure'); END;",
        )?;
        let key = Manager::normalize_path(&input);
        assert!(manager.drop_completed_entry(&key, &saved[&key]).is_err());
        assert!(manager.clear_progress().is_err());
        assert!(manager.cleanup().is_err());
        assert_eq!(manager.completed_count(), 1);
        assert!(manager.is_resume_mode());
        assert!(manager.is_completed(&input));
        assert_eq!(
            load_progress_from_sqlite(&manager.checkpoint_key)?
                .unwrap()
                .entries,
            saved
        );

        std::fs::write(&input, b"changed checkpoint input")?;
        assert!(manager.mark_completed(&input).is_err());
        assert_eq!(
            load_progress_from_sqlite(&manager.checkpoint_key)?
                .unwrap()
                .entries,
            saved
        );
        conn.execute_batch(
            "DROP TRIGGER reject_checkpoint_write; DROP TRIGGER reject_checkpoint_delete;",
        )?;
        manager.mark_completed(&input)?;
        assert!(manager.is_completed(&input));
        assert_ne!(
            load_progress_from_sqlite(&manager.checkpoint_key)?
                .unwrap()
                .entries,
            saved
        );
        manager.drop_completed_entry(&key, &saved[&key])?;
        assert!(
            manager.is_completed(&input),
            "stale validation must not remove a newer entry"
        );
        manager.cleanup()?;
        assert_eq!(manager.completed_count(), 0);
        assert!(!manager.is_resume_mode());
        assert!(load_progress_from_sqlite(&manager.checkpoint_key)?.is_none());
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn resume_initialization_rejects_required_persistence_failures() -> anyhow::Result<()> {
        let (target, progress, guard) = setup_test_env()?;
        let input = target.path().join("saved.mp4");
        create_test_file(&input)?;
        let output_a = target.path().join("output-a");
        let output_b = target.path().join("output-b");
        fs::create_dir_all(&output_a)?;
        fs::create_dir_all(&output_b)?;
        let manager = Manager::new_with_context(target.path(), Some(&output_a))?;
        manager.mark_completed(&input)?;
        let key = manager.checkpoint_key.clone();
        drop(manager);
        crate::conversion::clear_processed_list();
        let saved = crate::mfb_sqlite_store::blob_get(
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &key,
            CHECKPOINT_BLOB_SCHEMA,
        )?;
        let source = fs::read(&input)?;
        let lock = progress.path().join(format!("{key}.lock"));
        fs::write(&lock, b"pre-existing fixture lock")?;
        let conn = crate::mfb_sqlite_store::open_store_connection()?;
        for (operation, output) in [("INSERT", &output_a), ("DELETE", &output_b)] {
            let namespace = if operation == "INSERT" { "NEW" } else { "OLD" };
            conn.execute_batch(&format!(
                "CREATE TRIGGER reject_resume_update BEFORE {operation} ON blob_store
                 WHEN {namespace}.namespace = 'checkpoint'
                 BEGIN SELECT RAISE(ABORT, 'fixture resume failure'); END;"
            ))?;
            assert!(Manager::new_resuming_with_context(target.path(), Some(output)).is_err());
            assert_eq!(
                crate::mfb_sqlite_store::blob_get(
                    crate::mfb_sqlite_store::NS_CHECKPOINT,
                    &key,
                    CHECKPOINT_BLOB_SCHEMA,
                )?,
                saved
            );
            assert_eq!(fs::read(&input)?, source);
            assert_eq!(fs::read(&lock)?, b"pre-existing fixture lock");
            assert!(!crate::conversion::is_already_processed(&input));
            conn.execute_batch("DROP TRIGGER reject_resume_update;")?;
        }
        fs::remove_file(&lock)?;
        let resumed = Manager::new_resuming_with_context(target.path(), Some(&output_a))?;
        assert!(resumed.is_completed(&input));
        assert_eq!(resumed.completed_count(), 1);
        drop(resumed);
        let reset = Manager::new_resuming_with_context(target.path(), Some(&output_b))?;
        assert_eq!(reset.completed_count(), 0);
        assert!(!reset.is_resume_mode());
        assert!(load_progress_from_sqlite(&key)?.is_none());
        assert_eq!(fs::read(&input)?, source);
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn concurrent_checkpoint_writes_keep_every_committed_entry() -> anyhow::Result<()> {
        let (temp, progress, guard) = setup_test_env()?;
        let manager = Manager::new(temp.path())?;
        let store = progress.path().join("mfb_store.sqlite");
        let paths: Vec<_> = (0..12)
            .map(|index| temp.path().join(format!("{index}.mp4")))
            .collect();
        for path in &paths {
            create_test_file(path)?;
        }
        let barrier = std::sync::Barrier::new(paths.len());
        std::thread::scope(|scope| {
            for path in &paths {
                let manager = &manager;
                let store = &store;
                let barrier = &barrier;
                scope.spawn(move || {
                    let _store =
                        crate::mfb_sqlite_store::set_test_store_path_for_tests(store.clone());
                    barrier.wait();
                    manager.mark_completed(path).unwrap();
                });
            }
        });
        assert_eq!(manager.completed_count(), paths.len());
        let loaded = load_progress_from_sqlite(&manager.checkpoint_key)?.unwrap();
        assert_eq!(loaded.entries.len(), paths.len());
        assert!(
            paths
                .iter()
                .all(|p| loaded.entries.contains_key(&Manager::normalize_path(p)))
        );
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn checkpoint_entry_commits_do_not_rewrite_other_rows() -> anyhow::Result<()> {
        let (target, _progress, guard) = setup_test_env()?;
        let first = target.path().join("first.mp4");
        let second = target.path().join("second.mp4");
        create_test_file(&first)?;
        create_test_file(&second)?;
        let manager = Manager::new(target.path())?;
        manager.mark_completed(&first)?;
        let connection = crate::mfb_sqlite_store::open_store_connection()?;
        connection.execute(
            "UPDATE blob_store SET updated_at = 1 WHERE namespace = 'checkpoint'",
            [],
        )?;
        let row = |key: &str| {
            connection.query_row(
                "SELECT payload, payload_blake3, updated_at FROM blob_store
                 WHERE namespace = 'checkpoint' AND cache_key = ?1",
                [key],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
        };
        let first_key =
            checkpoint_entry_key(&manager.checkpoint_key, &Manager::normalize_path(&first));
        let header = row(&manager.checkpoint_key)?;
        let saved_first = row(&first_key)?;
        manager.mark_completed(&second)?;
        assert_eq!(row(&manager.checkpoint_key)?, header);
        assert_eq!(row(&first_key)?, saved_first);
        let loaded = load_progress_from_sqlite(&manager.checkpoint_key)?.unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(
            checkpoint_entry_keys(&connection, &manager.checkpoint_key)?.len(),
            2
        );
        manager.drop_completed_entry(
            &Manager::normalize_path(&first),
            &loaded.entries[&Manager::normalize_path(&first)],
        )?;
        assert_eq!(row(&manager.checkpoint_key)?, header);
        assert_eq!(
            load_progress_from_sqlite(&manager.checkpoint_key)?
                .unwrap()
                .entries
                .len(),
            1
        );
        manager.cleanup()?;
        assert_eq!(
            checkpoint_entry_keys(&connection, &manager.checkpoint_key)?,
            Vec::<String>::new()
        );
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn empty_legacy_checkpoint_migrates_before_first_entry_without_creating_absent_state()
    -> anyhow::Result<()> {
        let (root, _progress, guard) = setup_test_env()?;
        let cases = [(false, false), (false, true), (true, false), (true, true)];
        for (reacquire, saved_header) in cases {
            let target = root.path().join(format!("{reacquire}-{saved_header}"));
            fs::create_dir_all(&target)?;
            let input = target.join("first.mp4");
            create_test_file(&input)?;
            let released = if reacquire {
                let manager = Manager::new(&target)?;
                manager.release_lock()?;
                Some(manager)
            } else {
                None
            };
            let canonical_target = Manager::normalize_path_to_buf(&target);
            let key = Manager::hash_path(&canonical_target);
            if saved_header {
                let legacy = CheckpointBlob {
                    header: CheckpointHeader::new(&canonical_target, None)?,
                    entries: HashMap::new(),
                };
                crate::mfb_sqlite_store::blob_put(
                    crate::mfb_sqlite_store::NS_CHECKPOINT,
                    &key,
                    LEGACY_CHECKPOINT_BLOB_SCHEMA,
                    None,
                    &serde_json::to_vec(&legacy)?,
                )?;
            }
            let manager = if let Some(manager) = released {
                manager.acquire_lock()?;
                manager
            } else {
                Manager::new_resuming(&target)?
            };
            assert!(!manager.is_resume_mode());
            assert_eq!(manager.completed_count(), 0);
            assert_eq!(
                crate::mfb_sqlite_store::blob_get(
                    crate::mfb_sqlite_store::NS_CHECKPOINT,
                    &key,
                    CHECKPOINT_BLOB_SCHEMA,
                )?
                .is_some(),
                saved_header
            );
            manager.mark_completed(&input)?;
            assert!(manager.is_completed(&input));
            assert_eq!(load_progress_from_sqlite(&key)?.unwrap().entries.len(), 1);
            manager.cleanup()?;
        }
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn checkpoint_v2_migration_rolls_back_header_and_entries_together() -> anyhow::Result<()> {
        let (target, _progress, guard) = setup_test_env()?;
        let input = target.path().join("saved.mp4");
        create_test_file(&input)?;
        let canonical_target = Manager::normalize_path_to_buf(target.path());
        let key = Manager::hash_path(&canonical_target);
        let header = CheckpointHeader::new(&canonical_target, None)?;
        let entry = CheckpointEntry::from_path(&input)?;
        let legacy = serde_json::to_vec(&CheckpointBlob {
            header,
            entries: HashMap::from([(entry.path.clone(), entry)]),
        })?;
        crate::mfb_sqlite_store::blob_put(
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &key,
            LEGACY_CHECKPOINT_BLOB_SCHEMA,
            None,
            &legacy,
        )?;
        let connection = crate::mfb_sqlite_store::open_store_connection()?;
        connection.execute_batch(
            "CREATE TRIGGER reject_checkpoint_entry BEFORE INSERT ON blob_store
             WHEN NEW.namespace = 'checkpoint' AND instr(NEW.cache_key, '/') > 0
             BEGIN SELECT RAISE(ABORT, 'fixture migration failure'); END;",
        )?;
        assert!(Manager::new_resuming(target.path()).is_err());
        assert_eq!(
            crate::mfb_sqlite_store::blob_get(
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                &key,
                LEGACY_CHECKPOINT_BLOB_SCHEMA
            )?,
            Some(legacy)
        );
        assert_eq!(
            checkpoint_entry_keys(&connection, &key)?,
            Vec::<String>::new()
        );
        assert!(!crate::conversion::is_already_processed(&input));
        assert_eq!(fs::read(&input)?, b"checkpoint-test");
        connection.execute_batch("DROP TRIGGER reject_checkpoint_entry;")?;
        let resumed = Manager::new_resuming(target.path())?;
        assert!(resumed.is_completed(&input));
        assert_eq!(checkpoint_entry_keys(&connection, &key)?.len(), 1);
        assert!(
            crate::mfb_sqlite_store::blob_get(
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                &key,
                CHECKPOINT_BLOB_SCHEMA
            )?
            .is_some()
        );
        resumed.cleanup()?;
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn malformed_checkpoint_entry_rows_are_retained_and_block_resume() -> anyhow::Result<()> {
        let (target, _progress, guard) = setup_test_env()?;
        let input = target.path().join("saved.mp4");
        create_test_file(&input)?;
        let manager = Manager::new(target.path())?;
        manager.mark_completed(&input)?;
        let key = manager.checkpoint_key.clone();
        let entry_key = checkpoint_entry_key(&key, &Manager::normalize_path(&input));
        drop(manager);
        let connection = crate::mfb_sqlite_store::open_store_connection()?;
        let original = crate::mfb_sqlite_store::blob_get(
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &entry_key,
            CHECKPOINT_BLOB_SCHEMA,
        )?
        .unwrap();
        let mut wrong: CheckpointEntry = serde_json::from_slice(&original)?;
        wrong.path = Manager::normalize_path(&target.path().join("other.mp4"));
        let wrong = serde_json::to_vec(&wrong)?;
        for payload in [&original, &wrong] {
            crate::mfb_sqlite_store::blob_put(
                crate::mfb_sqlite_store::NS_CHECKPOINT,
                &entry_key,
                CHECKPOINT_BLOB_SCHEMA,
                None,
                payload,
            )?;
            if payload == &original {
                connection.execute("UPDATE blob_store SET payload_blake3 = X'00' WHERE namespace = 'checkpoint' AND cache_key = ?1", [&entry_key])?;
            }
            assert!(Manager::saved_entry_count(target.path(), None).is_err());
            assert!(Manager::new_resuming(target.path()).is_err());
            let retained: Vec<u8> = connection.query_row(
                "SELECT payload FROM blob_store WHERE namespace = 'checkpoint' AND cache_key = ?1",
                [&entry_key],
                |row| row.get(0),
            )?;
            assert_eq!(&retained, payload);
        }
        connection.execute(
            "DELETE FROM blob_store WHERE namespace = 'checkpoint' AND cache_key = ?1",
            [&key],
        )?;
        assert!(Manager::saved_entry_count(target.path(), None).is_err());
        assert!(Manager::new_resuming(target.path()).is_err());
        assert_eq!(checkpoint_entry_keys(&connection, &key)?.len(), 1);
        assert_eq!(fs::read(&input)?, b"checkpoint-test");
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn legacy_checkpoint_key_is_migrated_to_blake3() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();
        let input = target.join("legacy.mp4");
        create_test_file(&input)?;

        let canonical_target = Manager::normalize_path_to_buf(target);
        let checkpoint_key = Manager::hash_path(&canonical_target);
        let legacy_key = Manager::legacy_hash_path(&canonical_target);
        assert_eq!(checkpoint_key.len(), 64);
        assert_ne!(checkpoint_key, legacy_key);

        let mut header = CheckpointHeader::new(&canonical_target, None)?;
        header.cache_schema_version = 4;
        let entry = CheckpointEntry::from_path(&input)?;
        let entries = HashMap::from([(entry.path.clone(), entry)]);
        crate::mfb_sqlite_store::blob_put(
            crate::mfb_sqlite_store::NS_CHECKPOINT,
            &legacy_key,
            LEGACY_CHECKPOINT_BLOB_SCHEMA,
            None,
            &serde_json::to_vec(&CheckpointBlob { header, entries })?,
        )?;

        let checkpoint = Manager::new_resuming(target)?;
        assert!(checkpoint.is_completed(&input));
        assert!(load_progress_from_sqlite(&checkpoint_key)?.is_some());
        assert!(load_progress_from_sqlite(&legacy_key)?.is_none());
        drop(checkpoint);

        Manager::discard_saved_progress(target)?;
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn corrupt_saved_state_blocks_inspection_and_resume_without_overwriting_it()
    -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();
        let input = target.join("saved.mp4");
        create_test_file(&input)?;
        let manager = Manager::new(target)?;
        manager.mark_completed(&input)?;
        let key = manager.checkpoint_key.clone();
        drop(manager);

        let conn = crate::mfb_sqlite_store::open_store_connection()?;
        for mutation in ["payload_blake3 = X'00'", "schema_version = 999"] {
            conn.execute(
                &format!("UPDATE blob_store SET {mutation} WHERE namespace = 'checkpoint' AND cache_key = ?1"),
                [&key],
            )?;
            let read_row = || {
                conn.query_row(
                "SELECT schema_version, payload, payload_blake3 FROM blob_store WHERE namespace = 'checkpoint' AND cache_key = ?1",
                [&key],
                |r| Ok((r.get::<_, i32>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?)),
            )
            };
            let before = read_row()?;
            assert!(Manager::saved_entry_count(target, None).is_err());
            assert!(Manager::new_resuming(target).is_err());
            assert_eq!(read_row()?, before);
            assert_eq!(std::fs::read(&input)?, b"checkpoint-test");
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
        let decision_error = resolve_resume_choice_for_terminal(target, None, false, false, false)
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

        let (_unused_target, progress_temp, guard) = setup_test_env()?;

        {
            let checkpoint = Manager::new(target)?;
            checkpoint.acquire_lock()?;
            create_test_file(&target.join("file1.mp4"))?;
            checkpoint.mark_completed(&target.join("file1.mp4"))?;

            checkpoint.cleanup()?;
            assert!(load_progress_from_sqlite(&checkpoint.checkpoint_key)?.is_none());
            assert!(checkpoint.lock_file.exists());
        }

        assert!(!progress_temp.path().join("completed.txt").exists());
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn checkpoint_lock_excludes_other_managers_before_state_changes() -> anyhow::Result<()> {
        let (target, _progress, guard) = setup_test_env()?;
        let input = target.path().join("saved.mp4");
        create_test_file(&input)?;
        let first = Manager::new(target.path())?;
        first.mark_completed(&input)?;
        let saved = load_progress_from_sqlite(&first.checkpoint_key)?
            .unwrap()
            .entries;
        for resume in [false, true] {
            let error = Manager::new_with_context_inner(target.path(), None, resume)
                .err()
                .expect("another manager must not touch an owned checkpoint");
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            assert_eq!(
                load_progress_from_sqlite(&first.checkpoint_key)?
                    .unwrap()
                    .entries,
                saved
            );
        }
        assert!(Manager::discard_saved_progress(target.path()).is_err());
        first.release_lock()?;
        let second = Manager::new_resuming(target.path())?;
        assert!(first.check_lock()?);
        first.release_lock()?;
        drop(first);
        assert!(second.is_completed(&input));
        assert_eq!(
            Manager::new_resuming(target.path()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        second.cleanup()?;
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn checkpoint_lock_reacquire_reloads_state_and_rejects_released_writes() -> anyhow::Result<()> {
        let (target, _progress, guard) = setup_test_env()?;
        let inputs: Vec<_> = (0..3)
            .map(|i| target.path().join(format!("{i}.mp4")))
            .collect();
        for input in &inputs {
            create_test_file(input)?;
        }
        let first = Manager::new(target.path())?;
        first.mark_completed(&inputs[0])?;
        first.release_lock()?;
        let second = Manager::new_resuming(target.path())?;
        second.mark_completed(&inputs[1])?;
        let saved = load_progress_from_sqlite(&first.checkpoint_key)?
            .unwrap()
            .entries;
        assert!(first.mark_completed(&inputs[2]).is_err());
        assert!(first.clear_progress().is_err());
        assert!(first.cleanup().is_err());
        assert!(first.acquire_lock().is_err());
        assert!(!first.is_completed(&inputs[0]));
        assert_eq!(
            load_progress_from_sqlite(&first.checkpoint_key)?
                .unwrap()
                .entries,
            saved
        );
        second.release_lock()?;
        first.acquire_lock()?;
        assert_eq!(first.completed_count(), 2);
        assert!(first.is_completed(&inputs[1]));
        first.mark_completed(&inputs[2])?;
        assert_eq!(
            load_progress_from_sqlite(&first.checkpoint_key)?
                .unwrap()
                .entries
                .len(),
            3
        );
        drop(second);
        assert!(Manager::new_resuming(target.path()).is_err());
        first.cleanup()?;
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn checkpoint_lock_rejects_replaced_and_linked_paths() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;
        let (target, progress, guard) = setup_test_env()?;
        let input = target.path().join("saved.mp4");
        create_test_file(&input)?;
        let first = Manager::new(target.path())?;
        first.mark_completed(&input)?;
        let key = first.checkpoint_key.clone();
        let lock = first.lock_file.clone();
        fs::rename(&lock, progress.path().join("replaced.lock"))?;
        fs::write(&lock, b"replacement lock")?;
        assert!(first.mark_completed(&input).is_err());
        assert!(first.clear_progress().is_err());
        let second = Manager::new_resuming(target.path())?;
        drop(first);
        assert_eq!(fs::read(&lock)?, b"replacement lock");
        assert!(Manager::new_resuming(target.path()).is_err());
        second.release_lock()?;
        let saved = load_progress_from_sqlite(&key)?.unwrap().entries;
        fs::remove_file(&lock)?;
        let linked = progress.path().join("linked.lock");
        fs::write(&linked, b"linked lock")?;
        symlink(&linked, &lock)?;
        assert!(Manager::new(target.path()).is_err());
        fs::remove_file(&lock)?;
        fs::hard_link(&linked, &lock)?;
        assert!(Manager::new(target.path()).is_err());
        assert_eq!(load_progress_from_sqlite(&key)?.unwrap().entries, saved);
        assert_eq!(fs::read(&input)?, b"checkpoint-test");
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn checkpoint_lock_blocks_other_processes_and_recovers_after_exit() -> anyhow::Result<()> {
        const CHILD: &str = "MFB_CHECKPOINT_LOCK_TEST_CHILD";
        const TARGET: &str = "MFB_CHECKPOINT_LOCK_TEST_TARGET";
        const STORE: &str = "MFB_CHECKPOINT_LOCK_TEST_STORE";
        let mode = match std::env::var(CHILD) {
            Ok(mode) => Some(mode),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => anyhow::bail!("read {CHILD}: {error}"),
        };
        if let Some(mode) = mode {
            let target = PathBuf::from(std::env::var_os(TARGET).unwrap());
            let _store = crate::mfb_sqlite_store::set_test_store_path_for_tests(PathBuf::from(
                std::env::var_os(STORE).unwrap(),
            ));
            if mode == "blocked" {
                assert_eq!(
                    Manager::new_resuming(&target).err().unwrap().kind(),
                    io::ErrorKind::WouldBlock
                );
                return Ok(());
            }
            let manager = Manager::new_resuming(&target)?;
            manager.mark_completed(&target.join("child.mp4"))?;
            // Exit without Manager::drop: the kernel must release the owned handle.
            std::process::exit(0);
        }
        let (target, progress, guard) = setup_test_env()?;
        create_test_file(&target.path().join("parent.mp4"))?;
        create_test_file(&target.path().join("child.mp4"))?;
        let manager = Manager::new(target.path())?;
        manager.mark_completed(&target.path().join("parent.mp4"))?;
        let test_name = concat!(
            module_path!(),
            "::checkpoint_lock_blocks_other_processes_and_recovers_after_exit"
        )
        .split_once("::")
        .unwrap()
        .1;
        for mode in ["blocked", "write-and-exit"] {
            if mode == "write-and-exit" {
                manager.release_lock()?;
            }
            let output = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", test_name, "--nocapture"])
                .env(CHILD, mode)
                .env(TARGET, target.path())
                .env(STORE, progress.path().join("mfb_store.sqlite"))
                .env("MFB_PROGRESS_DIR", progress.path())
                .output()?;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
        }
        manager.acquire_lock()?;
        assert_eq!(manager.completed_count(), 2);
        assert!(manager.is_completed(&target.path().join("child.mp4")));
        manager.cleanup()?;
        teardown_test_env(guard);
        Ok(())
    }

    #[test]
    fn test_checkpoint_lock_acquire_release() -> anyhow::Result<()> {
        let (temp, _progress, guard) = setup_test_env()?;
        let target = temp.path();

        let checkpoint = Manager::new(target)?;

        assert!(!checkpoint.check_lock()?);

        checkpoint.acquire_lock()?;
        assert!(checkpoint.lock_file.exists());

        checkpoint.release_lock()?;
        assert!(checkpoint.lock_file.exists());
        assert!(!checkpoint.check_lock()?);
        checkpoint.acquire_lock()?;
        checkpoint.acquire_lock()?;
        assert!(Manager::new(target).is_err());
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
}
