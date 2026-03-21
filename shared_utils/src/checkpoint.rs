//! Checkpoint & Resume Module (断点续传)
//!
//! Provides atomic operation protection and resume capability for all conversion tools:
//! - Progress tracking: Record completed files for resume after interruption
//! - Atomic delete: Verify output integrity before deleting original
//! - Lock file: Prevent concurrent processing of same directory
//!
//! # Usage
//! ```no_run
//! use shared_utils::checkpoint::{CheckpointManager, safe_delete_original, MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE};
//! use std::path::Path;
//!
//! fn main() -> anyhow::Result<()> {
//!     let target_dir = Path::new("/tmp/test");
//!     let file_path = Path::new("/tmp/test/file.jpg");
//!     let input = Path::new("/tmp/test/input.jpg");
//!     let output = Path::new("/tmp/test/output.jxl");
//!
//!     // Initialize checkpoint for a directory
//!     let mut checkpoint = CheckpointManager::new(target_dir)?;
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
//!     safe_delete_original(&input, &output, MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE)?;
//!     Ok(())
//! }
//! ```

/// Minimum output size (bytes) to allow deleting the original for **image** conversions.
/// Outputs smaller than this are treated as invalid; original is protected.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE: u64 = 100;

/// Minimum output size (bytes) to allow deleting the original for **video** conversions.
/// Video containers typically need at least ~1KB to be valid; use this to avoid deleting original on corrupt/tiny output.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO: u64 = 1000;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::version::{cache_algorithm_version, CACHE_SCHEMA_VERSION};

/// The central location for all MFB progress tracking to avoid polluting user directories.
fn get_central_progress_dir() -> PathBuf {
    if let Ok(path) = std::env::var("MFB_PROGRESS_DIR") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".mfb_progress")
}
const LOCK_STALE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
const CHECKPOINT_FORMAT_VERSION: u32 = 2;

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CheckpointEntry {
    path: String,
    size: i64,
    mtime: i64,
    ctime: i64,
    btime: i64,
}

impl CheckpointEntry {
    fn from_path(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let size = metadata.len() as i64;
        let mtime = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as i64;

        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(unix)]
        let ctime = metadata.ctime_nsec();
        #[cfg(windows)]
        use std::os::windows::fs::MetadataExt;
        #[cfg(windows)]
        let ctime = metadata.last_write_time() as i64;
        #[cfg(not(any(unix, windows)))]
        let ctime = mtime;

        let btime = match metadata.created() {
            Ok(t) => t
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(ctime),
            Err(_) => ctime,
        };

        Ok(Self {
            path: CheckpointManager::normalize_path(path),
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
    fn new(target_dir: &Path, output_root: Option<&Path>) -> Self {
        Self {
            format_version: CHECKPOINT_FORMAT_VERSION,
            target_dir: CheckpointManager::normalize_path(target_dir),
            output_root: output_root.map(CheckpointManager::normalize_path),
            cache_algorithm_version: cache_algorithm_version(),
            cache_schema_version: CACHE_SCHEMA_VERSION,
            created_at: current_unix_secs(),
        }
    }

    fn is_compatible_with(&self, expected: &Self) -> bool {
        self.format_version == expected.format_version
            && self.target_dir == expected.target_dir
            && self.output_root == expected.output_root
            && self.cache_algorithm_version == expected.cache_algorithm_version
            && self.cache_schema_version == expected.cache_schema_version
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CheckpointRecord {
    Header(CheckpointHeader),
    Entry(CheckpointEntry),
}

#[derive(Debug, Default)]
struct LoadedCheckpointState {
    header: Option<CheckpointHeader>,
    entries: HashMap<String, CheckpointEntry>,
    legacy_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockInfo {
    pid: u32,
    start_time: u64,
    created_at: u64,
    hostname: String,
}

impl LockInfo {
    fn new() -> Self {
        let now = current_unix_secs();
        Self {
            pid: std::process::id(),
            start_time: get_process_start_time().unwrap_or(now),
            created_at: now,
            hostname: get_hostname(),
        }
    }

    fn is_stale(&self) -> bool {
        let now = current_unix_secs();
        now.saturating_sub(self.created_at) > LOCK_STALE_TIMEOUT_SECS
    }
}

#[cfg(unix)]
fn get_process_start_time() -> Option<u64> {
    get_process_start_time_for_pid(std::process::id())
}

#[cfg(not(unix))]
fn get_process_start_time() -> Option<u64> {
    Some(current_unix_secs())
}

#[cfg(unix)]
fn get_process_start_time_for_pid(pid: u32) -> Option<u64> {
    use std::process::Command;
    let mut saw_real_error = false;

    for field in ["etimes", "etime"] {
        let output = match Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", &format!("{}=", field)])
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                eprintln!(
                    "⚠️ [checkpoint] Failed to query process age for PID {} via ps {}: {}",
                    pid, field, err
                );
                return None;
            }
        };

        if output.status.success() {
            let stdout = match String::from_utf8(output.stdout) {
                Ok(stdout) => stdout,
                Err(err) => {
                    eprintln!(
                        "⚠️ [checkpoint] Non-UTF-8 process age output for PID {} via ps {}: {}",
                        pid, field, err
                    );
                    return None;
                }
            };

            let elapsed_secs = match field {
                "etimes" => stdout.trim().parse::<u64>().ok(),
                "etime" => parse_ps_etime_to_secs(&stdout),
                _ => None,
            };

            if let Some(elapsed_secs) = elapsed_secs {
                return Some(current_unix_secs().saturating_sub(elapsed_secs));
            }

            eprintln!(
                "⚠️ [checkpoint] Failed to parse process age for PID {} from ps {} output: {}",
                pid,
                field,
                stdout.trim()
            );
            saw_real_error = true;
            continue;
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if ps_field_unsupported(stderr.trim()) {
            continue;
        }

        eprintln!(
            "⚠️ [checkpoint] ps {} returned non-zero while querying PID {}: {}",
            field,
            pid,
            stderr.trim()
        );
        saw_real_error = true;
    }

    if saw_real_error {
        None
    } else {
        None
    }
}

#[cfg(unix)]
fn ps_field_unsupported(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("keyword not found")
        || lower.contains("no valid keywords")
        || lower.contains("invalid keyword")
}

#[cfg(unix)]
fn parse_ps_etime_to_secs(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (days, clock) = match trimmed.split_once('-') {
        Some((days, rest)) => (days.trim().parse::<u64>().ok()?, rest.trim()),
        None => (0, trimmed),
    };

    let parts: Vec<_> = clock.split(':').collect();
    let clock_secs = match parts.as_slice() {
        [minutes, seconds] => minutes.trim().parse::<u64>().ok()? * 60
            + seconds.trim().parse::<u64>().ok()?,
        [hours, minutes, seconds] => {
            hours.trim().parse::<u64>().ok()? * 3600
                + minutes.trim().parse::<u64>().ok()? * 60
                + seconds.trim().parse::<u64>().ok()?
        }
        _ => return None,
    };

    Some(days * 24 * 3600 + clock_secs)
}

#[cfg(not(unix))]
fn get_process_start_time_for_pid(_pid: u32) -> Option<u64> {
    None
}

fn get_hostname() -> String {
    #[cfg(unix)]
    {
        use std::process::Command;
        match Command::new("hostname").output() {
            Ok(output) if output.status.success() => String::from_utf8(output.stdout)
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|err| {
                    eprintln!("⚠️ [checkpoint] Non-UTF-8 hostname output: {}", err);
                    "unknown".to_string()
                }),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "⚠️ [checkpoint] hostname returned non-zero status: {}",
                    stderr.trim()
                );
                "unknown".to_string()
            }
            Err(err) => {
                eprintln!("⚠️ [checkpoint] Failed to query hostname: {}", err);
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

pub struct CheckpointManager {
    progress_dir: PathBuf,
    lock_file: PathBuf,
    progress_file: PathBuf,
    header: CheckpointHeader,
    completed: Mutex<HashMap<String, CheckpointEntry>>,
    resume_mode: AtomicBool,
}

impl CheckpointManager {
    pub fn new(target_dir: &Path) -> io::Result<Self> {
        Self::new_with_context(target_dir, None)
    }

    pub fn new_with_context(target_dir: &Path, output_root: Option<&Path>) -> io::Result<Self> {
        let canonical_target = Self::normalize_path_to_buf(target_dir);
        let dir_hash = Self::hash_path(&canonical_target);
        let header = CheckpointHeader::new(&canonical_target, output_root);

        let central_dir = get_central_progress_dir();
        fs::create_dir_all(&central_dir)?;

        let progress_file = central_dir.join(format!("{}.txt", dir_hash));
        let lock_file = central_dir.join(format!("{}.lock", dir_hash));

        let loaded = Self::load_progress(&progress_file)?;
        let (completed_set, resume_mode, reset_reason) =
            Self::validate_loaded_state(&loaded, &header, output_root);

        if let Some(reason) = reset_reason.as_deref() {
            eprintln!("⚠️ [checkpoint] {}", reason);
            if progress_file.exists() {
                if let Err(err) = fs::remove_file(&progress_file) {
                    eprintln!(
                        "⚠️ [checkpoint] Failed to remove invalidated checkpoint file {}: {}",
                        progress_file.display(),
                        err
                    );
                }
            }
        }

        let manager = Self {
            progress_dir: central_dir,
            lock_file,
            progress_file,
            header,
            completed: Mutex::new(completed_set),
            resume_mode: AtomicBool::new(resume_mode),
        };

        if manager.resume_mode.load(Ordering::Relaxed) {
            if let Err(err) = manager.rewrite_progress_file() {
                eprintln!(
                    "⚠️ [checkpoint] Failed to compact validated checkpoint state: {}",
                    err
                );
            }
        }

        Ok(manager)
    }

    pub fn check_lock(&self) -> io::Result<Option<u32>> {
        if !self.lock_file.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.lock_file)?;

        if let Ok(lock_info) = serde_json::from_str::<LockInfo>(&content) {
            if lock_info.pid == std::process::id() {
                if let Err(e) = fs::remove_file(&self.lock_file) {
                    eprintln!("⚠️ [checkpoint] Failed to remove own lock file: {}", e);
                }
                return Ok(None);
            }

            if lock_info.is_stale() {
                eprintln!("⚠️ LOCK STALE: Lock file older than 24 hours, removing");
                if let Err(e) = fs::remove_file(&self.lock_file) {
                    eprintln!("⚠️ [checkpoint] Failed to remove stale lock file: {}", e);
                }
                return Ok(None);
            }

            #[cfg(unix)]
            {
                use std::process::Command;
                let exists = match Command::new("kill")
                    .args(["-0", &lock_info.pid.to_string()])
                    .status()
                {
                    Ok(status) => status.success(),
                    Err(err) => {
                        eprintln!(
                            "⚠️ [checkpoint] Failed to probe lock owner PID {} via kill -0: {}",
                            lock_info.pid, err
                        );
                        false
                    }
                };

                if !exists {
                    eprintln!(
                        "⚠️ LOCK STALE: PID {} no longer exists, removing",
                        lock_info.pid
                    );
                    if let Err(e) = fs::remove_file(&self.lock_file) {
                        eprintln!("⚠️ [checkpoint] Failed to remove stale lock file: {}", e);
                    }
                    return Ok(None);
                }

                if let Some(current_start) = get_process_start_time_for_pid(lock_info.pid) {
                    if current_start != lock_info.start_time {
                        eprintln!(
                            "⚠️ LOCK STALE: PID {} reused (start time mismatch), removing",
                            lock_info.pid
                        );
                        if let Err(e) = fs::remove_file(&self.lock_file) {
                            eprintln!("⚠️ [checkpoint] Failed to remove stale lock file: {}", e);
                        }
                        return Ok(None);
                    }
                }

                return Ok(Some(lock_info.pid));
            }

            #[cfg(not(unix))]
            {
                return Ok(Some(lock_info.pid));
            }
        }

        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid == std::process::id() {
                if let Err(e) = fs::remove_file(&self.lock_file) {
                    eprintln!("⚠️ [checkpoint] Failed to remove own lock file: {}", e);
                }
                return Ok(None);
            }
            if let Ok(meta) = fs::metadata(&self.lock_file) {
                if let Ok(modified) = meta.modified() {
                    if let Ok(elapsed) = modified.elapsed() {
                        if elapsed.as_secs() > LOCK_STALE_TIMEOUT_SECS {
                            if let Err(e) = fs::remove_file(&self.lock_file) {
                                eprintln!(
                                    "⚠️ [checkpoint] Failed to remove stale lock file: {}",
                                    e
                                );
                            }
                            return Ok(None);
                        }
                    }
                }
            }
            return Ok(Some(pid));
        }

        eprintln!("⚠️ LOCK INVALID: Cannot parse lock file, removing");
        if let Err(e) = fs::remove_file(&self.lock_file) {
            eprintln!("⚠️ [checkpoint] Failed to remove invalid lock file: {}", e);
        }
        Ok(None)
    }

    pub fn acquire_lock(&self) -> io::Result<()> {
        let lock_info = LockInfo::new();
        let json = serde_json::to_string_pretty(&lock_info)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        loop {
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
                            format!("Checkpoint lock already held by PID {}", pid),
                        ));
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }

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
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        completed.len()
    }

    pub fn is_completed(&self, path: &Path) -> bool {
        let key = Self::normalize_path(path);
        let maybe_entry = {
            let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
            completed.get(&key).cloned()
        };

        let Some(entry) = maybe_entry else {
            return false;
        };

        match entry.matches_current_file(path) {
            Ok(true) => true,
            Ok(false) => {
                eprintln!(
                    "⚠️ [checkpoint] Resume entry became stale after input changed: {}. Reprocessing.",
                    path.display()
                );
                self.drop_completed_entry(&key);
                false
            }
            Err(err) => {
                eprintln!(
                    "⚠️ [checkpoint] Failed to validate checkpoint entry {}: {}. Reprocessing.",
                    path.display(),
                    err
                );
                false
            }
        }
    }

    pub fn mark_completed(&self, path: &Path) -> io::Result<()> {
        let entry = CheckpointEntry::from_path(path)?;
        let key = entry.path.clone();
        {
            let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
            if completed.contains_key(&key) {
                return Ok(());
            }
            completed.insert(key.clone(), entry.clone());
        }
        self.resume_mode.store(true, Ordering::Relaxed);
        self.ensure_progress_header_exists()?;

        // Append to file outside the lock if possible, but actually we need to preserve order/integrity
        // Using a manual lock for the file append
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.progress_file)?;
        let record = serde_json::to_string(&CheckpointRecord::Entry(entry))
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        writeln!(file, "{}", record)?;

        // Also sync to the global processed list in conversion module
        crate::conversion::mark_as_processed(path);
        Ok(())
    }

    pub fn sync_to_processed_list(&self) {
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        for path_str in completed.keys() {
            crate::conversion::mark_as_processed(Path::new(path_str));
        }
    }

    pub fn clear_progress(&self) -> io::Result<()> {
        let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        completed.clear();
        self.resume_mode.store(false, Ordering::Relaxed);
        if self.progress_file.exists() {
            fs::remove_file(&self.progress_file)?;
        }
        Ok(())
    }

    pub fn reset_if_output_root_missing(&self, output_root: Option<&Path>) -> io::Result<bool> {
        let Some(output_root) = output_root else {
            return Ok(false);
        };

        if !self.is_resume_mode() || output_root.exists() {
            return Ok(false);
        }

        let completed = self.completed_count();
        eprintln!(
            "⚠️ [checkpoint] Found {} saved resume entries, but output root {} is missing. Assuming the optimized folder was intentionally removed; clearing old resume state and restarting full processing.",
            completed,
            output_root.display()
        );
        self.clear_progress()?;
        crate::conversion::clear_processed_list();
        Ok(true)
    }

    pub fn cleanup(&self) -> io::Result<()> {
        if self.progress_file.exists() {
            if let Err(err) = fs::remove_file(&self.progress_file) {
                eprintln!(
                    "⚠️ [checkpoint] Failed to remove progress file {}: {}",
                    self.progress_file.display(),
                    err
                );
            }
        }
        if self.lock_file.exists() {
            if let Err(err) = fs::remove_file(&self.lock_file) {
                eprintln!(
                    "⚠️ [checkpoint] Failed to remove lock file {}: {}",
                    self.lock_file.display(),
                    err
                );
            }
        }
        Ok(())
    }

    fn normalize_path_to_buf(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(path)
            }
        })
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
        Self::normalize_path_to_buf(path)
            .to_str()
            .map(String::from)
            .unwrap_or_else(|| path.display().to_string())
    }

    fn load_progress(progress_file: &Path) -> io::Result<LoadedCheckpointState> {
        if !progress_file.exists() {
            return Ok(LoadedCheckpointState::default());
        }

        let file = File::open(progress_file)?;
        let reader = BufReader::new(file);
        let mut state = LoadedCheckpointState::default();

        let mut read_error = None;
        for line in reader.lines() {
            match line {
                Ok(path) => {
                    let trimmed = path.trim();
                    if !trimmed.is_empty() {
                        if trimmed.starts_with('{') {
                            match serde_json::from_str::<CheckpointRecord>(trimmed) {
                                Ok(CheckpointRecord::Header(header)) => {
                                    state.header = Some(header);
                                }
                                Ok(CheckpointRecord::Entry(entry)) => {
                                    state.entries.insert(entry.path.clone(), entry);
                                }
                                Err(err) => {
                                    if read_error.is_none() {
                                        read_error = Some(io::Error::new(
                                            io::ErrorKind::InvalidData,
                                            format!("Failed to parse checkpoint record: {}", err),
                                        ));
                                    }
                                }
                            }
                        } else {
                            state.legacy_entries += 1;
                        }
                    }
                }
                Err(err) => {
                    if read_error.is_none() {
                        read_error = Some(err);
                    }
                }
            }
        }

        if let Some(err) = read_error {
            return Err(err);
        }

        Ok(state)
    }

    fn validate_loaded_state(
        loaded: &LoadedCheckpointState,
        expected_header: &CheckpointHeader,
        output_root: Option<&Path>,
    ) -> (HashMap<String, CheckpointEntry>, bool, Option<String>) {
        if loaded.legacy_entries > 0 {
            return (
                HashMap::new(),
                false,
                Some(format!(
                    "Legacy checkpoint format detected ({} path-only entries). Clearing old resume state because it lacks cache/signature validation metadata.",
                    loaded.legacy_entries
                )),
            );
        }

        let Some(header) = loaded.header.as_ref() else {
            if loaded.entries.is_empty() {
                return (HashMap::new(), false, None);
            }
            return (
                HashMap::new(),
                false,
                Some("Checkpoint entries were found without a header. Clearing invalid resume state.".to_string()),
            );
        };

        if !header.is_compatible_with(expected_header) {
            return (
                HashMap::new(),
                false,
                Some(format!(
                    "Checkpoint context changed (target/output/cache version mismatch). Clearing stale resume state for {}.",
                    expected_header.target_dir
                )),
            );
        }

        if let Some(output_root) = output_root {
            if !output_root.exists() && !loaded.entries.is_empty() {
                return (
                    HashMap::new(),
                    false,
                    Some(format!(
                        "Found {} saved resume entries, but output root {} is missing. Assuming the optimized folder was intentionally removed; clearing old resume state and restarting full processing.",
                        loaded.entries.len(),
                        output_root.display()
                    )),
                );
            }
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
                    "All saved checkpoint entries became invalid during startup validation (changed: {}, missing: {}, unreadable: {}). Clearing stale resume state.",
                    changed, missing, unreadable
                )),
            );
        }

        if changed > 0 || missing > 0 || unreadable > 0 {
            eprintln!(
                "⚠️ [checkpoint] Dropped stale resume entries during validation (changed: {}, missing: {}, unreadable: {}).",
                changed, missing, unreadable
            );
        }

        let resume_mode = !valid.is_empty();
        (valid, resume_mode, None)
    }

    fn ensure_progress_header_exists(&self) -> io::Result<()> {
        if self.progress_file.exists() {
            return Ok(());
        }
        self.rewrite_progress_file()
    }

    fn rewrite_progress_file(&self) -> io::Result<()> {
        let temp_path = self.progress_file.with_extension("txt.tmp");
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = File::create(&temp_path)?;
        let header = serde_json::to_string(&CheckpointRecord::Header(self.header.clone()))
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        writeln!(file, "{}", header)?;
        for entry in completed.values() {
            let line = serde_json::to_string(&CheckpointRecord::Entry(entry.clone()))
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            writeln!(file, "{}", line)?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(temp_path, &self.progress_file)?;
        Ok(())
    }

    fn drop_completed_entry(&self, key: &str) {
        let became_empty = {
            let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
            completed.remove(key);
            completed.is_empty()
        };
        if became_empty {
            self.resume_mode.store(false, Ordering::Relaxed);
        }
        if let Err(err) = self.rewrite_progress_file() {
            eprintln!(
                "⚠️ [checkpoint] Failed to rewrite checkpoint state after dropping stale entry: {}",
                err
            );
        }
    }
}

impl Drop for CheckpointManager {
    fn drop(&mut self) {
        if let Err(err) = self.release_lock() {
            eprintln!(
                "⚠️ [checkpoint] Failed to release lock on drop {}: {}",
                self.lock_file.display(),
                err
            );
        }
    }
}

pub fn verify_output_integrity(output: &Path, min_size: u64) -> Result<(), String> {
    if !output.exists() {
        return Err("Output file does not exist".to_string());
    }

    let metadata =
        fs::metadata(output).map_err(|e| format!("Cannot read output metadata: {}", e))?;

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

    let mut file = File::open(output).map_err(|e| format!("Cannot open output file: {}", e))?;

    let mut buffer = [0u8; 16];
    file.read(&mut buffer)
        .map_err(|e| format!("Cannot read output file: {}", e))?;

    Ok(())
}

pub fn safe_delete_original(input: &Path, output: &Path, min_output_size: u64) -> io::Result<()> {
    if let Err(reason) = verify_output_integrity(output, min_output_size) {
        eprintln!("   ⚠️  Output integrity check FAILED: {}", reason);
        eprintln!("   🛡️  Original file PROTECTED: {}", input.display());
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Output integrity check failed: {}", reason),
        ));
    }

    fs::remove_file(input)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as std_mutex;
    use tempfile::TempDir;
    static TEST_LOCK: std_mutex<()> = std_mutex::new(());

    fn setup_test_env() -> (TempDir, TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp_target = TempDir::new().unwrap();
        let temp_progress = TempDir::new().unwrap();
        std::env::set_var("MFB_PROGRESS_DIR", temp_progress.path());
        (temp_target, temp_progress, guard)
    }

    fn teardown_test_env(_guard: std::sync::MutexGuard<'static, ()>) {
        std::env::remove_var("MFB_PROGRESS_DIR");
    }

    fn create_test_file(path: &Path) {
        fs::write(path, b"checkpoint-test").unwrap();
    }

    #[test]
    fn test_checkpoint_new_creates_progress_dir() {
        let (_target, progress, guard) = setup_test_env();
        let checkpoint = CheckpointManager::new(_target.path()).unwrap();
        assert!(checkpoint.progress_dir().exists());
        assert!(progress.path().exists());
        teardown_test_env(guard);
    }

    #[test]
    fn test_checkpoint_mark_and_check_completed() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();

        let checkpoint = CheckpointManager::new(target).unwrap();

        let file1 = target.join("test1.mp4");
        let file2 = target.join("test2.mp4");
        create_test_file(&file1);
        create_test_file(&file2);

        assert!(!checkpoint.is_completed(&file1));
        assert!(!checkpoint.is_completed(&file2));

        checkpoint.mark_completed(&file1).unwrap();

        assert!(checkpoint.is_completed(&file1));
        assert!(!checkpoint.is_completed(&file2));
        assert_eq!(checkpoint.completed_count(), 1);

        checkpoint.mark_completed(&file2).unwrap();

        assert!(checkpoint.is_completed(&file1));
        assert!(checkpoint.is_completed(&file2));
        assert_eq!(checkpoint.completed_count(), 2);
        teardown_test_env(guard);
    }

    #[test]
    fn test_checkpoint_resume_mode() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();

        {
            let checkpoint = CheckpointManager::new(target).unwrap();
            create_test_file(&target.join("file1.mp4"));
            create_test_file(&target.join("file2.mp4"));
            checkpoint
                .mark_completed(&target.join("file1.mp4"))
                .unwrap();
            checkpoint
                .mark_completed(&target.join("file2.mp4"))
                .unwrap();
        }

        {
            let checkpoint = CheckpointManager::new(target).unwrap();

            assert!(checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 2);
            assert!(checkpoint.is_completed(&target.join("file1.mp4")));
            assert!(checkpoint.is_completed(&target.join("file2.mp4")));
            assert!(!checkpoint.is_completed(&target.join("file3.mp4")));
        }
        teardown_test_env(guard);
    }

    #[test]
    fn test_checkpoint_clear_progress() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();

        let checkpoint = CheckpointManager::new(target).unwrap();
        create_test_file(&target.join("file1.mp4"));
        create_test_file(&target.join("file2.mp4"));
        checkpoint
            .mark_completed(&target.join("file1.mp4"))
            .unwrap();
        checkpoint
            .mark_completed(&target.join("file2.mp4"))
            .unwrap();

        assert_eq!(checkpoint.completed_count(), 2);

        checkpoint.clear_progress().unwrap();

        assert_eq!(checkpoint.completed_count(), 0);
        assert!(!checkpoint.is_resume_mode());
        teardown_test_env(guard);
    }

    #[test]
    fn test_checkpoint_mark_completed_enables_resume_mode_for_current_run() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();

        let checkpoint = CheckpointManager::new(target).unwrap();
        assert!(!checkpoint.is_resume_mode());
        create_test_file(&target.join("file1.mp4"));

        checkpoint
            .mark_completed(&target.join("file1.mp4"))
            .unwrap();

        assert!(checkpoint.is_resume_mode());
        teardown_test_env(guard);
    }

    #[test]
    fn test_reset_if_output_root_missing_clears_stale_resume_state() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();
        let missing_output = target.join("deleted_optimized");

        {
            let checkpoint = CheckpointManager::new(target).unwrap();
            create_test_file(&target.join("file1.mp4"));
            create_test_file(&target.join("file2.mp4"));
            checkpoint
                .mark_completed(&target.join("file1.mp4"))
                .unwrap();
            checkpoint
                .mark_completed(&target.join("file2.mp4"))
                .unwrap();
        }

        let checkpoint = CheckpointManager::new(target).unwrap();
        assert!(checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 2);

        let cleared = checkpoint
            .reset_if_output_root_missing(Some(&missing_output))
            .unwrap();

        assert!(cleared);
        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
        assert!(!checkpoint.progress_file.exists());
        teardown_test_env(guard);
    }

    #[test]
    fn test_new_with_context_clears_resume_state_when_output_root_changes() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();
        let output_a = target.join("optimized_a");
        let output_b = target.join("optimized_b");
        let input = target.join("file1.mp4");
        create_test_file(&input);

        {
            let checkpoint = CheckpointManager::new_with_context(target, Some(&output_a)).unwrap();
            checkpoint.mark_completed(&input).unwrap();
        }

        let checkpoint = CheckpointManager::new_with_context(target, Some(&output_b)).unwrap();
        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
        teardown_test_env(guard);
    }

    #[test]
    fn test_new_with_context_drops_entries_when_input_signature_changes() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();
        let output = target.join("optimized");
        let input = target.join("file1.mp4");
        fs::write(&input, b"aaaaaaaaaaaaaaa").unwrap();

        {
            let checkpoint = CheckpointManager::new_with_context(target, Some(&output)).unwrap();
            checkpoint.mark_completed(&input).unwrap();
        }

        std::thread::sleep(Duration::from_millis(2));
        fs::write(&input, b"bbbbbbbbbbbbbbb").unwrap();

        let checkpoint = CheckpointManager::new_with_context(target, Some(&output)).unwrap();
        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
        assert!(!checkpoint.is_completed(&input));
        teardown_test_env(guard);
    }

    #[test]
    fn test_checkpoint_cleanup() {
        let temp_target = TempDir::new().unwrap();
        let target = temp_target.path();

        let (progress_temp, _, guard) = setup_test_env();

        {
            let checkpoint = CheckpointManager::new(target).unwrap();
            checkpoint.acquire_lock().unwrap();
            create_test_file(&target.join("file1.mp4"));
            checkpoint
                .mark_completed(&target.join("file1.mp4"))
                .unwrap();

            checkpoint.cleanup().unwrap();
        }

        assert!(!progress_temp.path().join("completed.txt").exists());
        teardown_test_env(guard);
    }

    #[test]
    fn test_checkpoint_lock_acquire_release() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();

        let checkpoint = CheckpointManager::new(target).unwrap();

        assert!(checkpoint.check_lock().unwrap().is_none());

        checkpoint.acquire_lock().unwrap();
        assert!(checkpoint.lock_file.exists());

        checkpoint.release_lock().unwrap();
        assert!(!checkpoint.lock_file.exists());
        teardown_test_env(guard);
    }

    #[test]
    fn test_verify_output_integrity_valid_file() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("output.mp4");

        fs::write(&output, b"This is test content for integrity check").unwrap();

        assert!(verify_output_integrity(&output, 10).is_ok());
    }

    #[test]
    fn test_verify_output_integrity_empty_file() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("empty.mp4");

        fs::write(&output, b"").unwrap();

        let result = verify_output_integrity(&output, 10);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_verify_output_integrity_too_small() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("small.mp4");

        fs::write(&output, b"tiny").unwrap();

        let result = verify_output_integrity(&output, 100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too small"));
    }

    #[test]
    fn test_verify_output_integrity_nonexistent() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("nonexistent.mp4");

        let result = verify_output_integrity(&output, 10);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_safe_delete_original_success() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("output.mp4");

        fs::write(&input, b"original content").unwrap();
        fs::write(&output, b"converted content that is valid").unwrap();

        assert!(safe_delete_original(&input, &output, 10).is_ok());

        assert!(!input.exists());
        assert!(output.exists());
    }

    #[test]
    fn test_safe_delete_original_protects_on_invalid_output() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("output.mp4");

        fs::write(&input, b"original content").unwrap();
        fs::write(&output, b"").unwrap();

        assert!(safe_delete_original(&input, &output, 10).is_err());

        assert!(input.exists());
    }

    #[test]
    fn test_safe_delete_original_protects_on_missing_output() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("nonexistent.mp4");

        fs::write(&input, b"original content").unwrap();

        assert!(safe_delete_original(&input, &output, 10).is_err());

        assert!(input.exists());
    }

    #[test]
    fn test_full_workflow_with_interruption() {
        let (temp, _progress, guard) = setup_test_env();
        let target = temp.path();

        let files: Vec<PathBuf> = (1..=5)
            .map(|i| {
                let path = target.join(format!("video{}.mp4", i));
                fs::write(&path, format!("content {}", i)).unwrap();
                path
            })
            .collect();

        {
            let checkpoint = CheckpointManager::new(target).unwrap();
            checkpoint.acquire_lock().unwrap();

            for file in files.iter().take(2) {
                checkpoint.mark_completed(file).unwrap();
            }

            checkpoint.release_lock().unwrap();
        }

        {
            let checkpoint = CheckpointManager::new(target).unwrap();

            assert!(checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 2);

            checkpoint.acquire_lock().unwrap();

            let mut processed = 0;
            let mut skipped = 0;

            for file in &files {
                if checkpoint.is_completed(file) {
                    skipped += 1;
                    continue;
                }
                checkpoint.mark_completed(file).unwrap();
                processed += 1;
            }

            assert_eq!(skipped, 2);
            assert_eq!(processed, 3);
            assert_eq!(checkpoint.completed_count(), 5);

            checkpoint.cleanup().unwrap();
        }

        {
            let checkpoint = CheckpointManager::new(target).unwrap();
            assert!(!checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 0);
        }
        teardown_test_env(guard);
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
