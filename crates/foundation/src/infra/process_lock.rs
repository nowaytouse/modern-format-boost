use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::fs::{self, File};
use std::ops::Deref;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};

static INIT_GHOST_ENV: Once = Once::new();

fn held_dir_locks() -> &'static Mutex<HashSet<PathBuf>> {
    static HELD_DIR_LOCKS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    HELD_DIR_LOCKS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn held_dir_locks_guard<'a>(branch: &'static str) -> std::sync::MutexGuard<'a, HashSet<PathBuf>> {
    crate::media_conversion_gate::mutex_guard_or_recover(branch, held_dir_locks().lock())
}

/// RAII guard for a directory lock held by the current process.
#[derive(Debug)]
pub struct DirLock {
    file: File,
    locked_path: PathBuf,
}

impl Deref for DirLock {
    type Target = File;

    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        let mut held_locks = held_dir_locks_guard("process_lock_registry_drop");
        held_locks.remove(&self.locked_path);
    }
}

/// Initializes Ghost Mode (Zero Pollution) by setting up the MFB tmp layout.
///
/// **Call exactly once at process startup** (`img`/`vid` `main` before `rayon`
/// or thread pools). `TMPDIR` and `PATH` are mutated inside a
/// [`std::sync::Once`] because [`std::env::set_var`] is unsafe under concurrent
/// writers (Rust 2024).
///
/// # Errors
/// Returns an error if the MFB temporary directory cannot be created.
pub fn init_ghost_mode() -> Result<()> {
    let tmp = get_mfb_tmp_dir()?;
    INIT_GHOST_ENV.call_once(move || {
        // SAFETY: Runs once before worker threads; see [`init_ghost_mode`] contract.
        unsafe {
            std::env::set_var("TMPDIR", &tmp);
        }
        ensure_tool_path();
    });
    Ok(())
}

/// Augment `PATH` with standard locations for external tools.
///
/// When the binary is launched from a `.app` bundle (macOS Finder, Dock) or
/// any non-interactive context, `PATH` inherits a minimal
/// `/usr/bin:/bin:/usr/sbin:/sbin` and misses Homebrew's install roots.
/// Commands like `ffprobe`, `magick`, `cjxl`, `exiftool` installed via Homebrew
/// end up "missing" even when present.
///
/// Prepends the well-known Homebrew and `MacPorts` bin directories if they
/// exist and are not already on `PATH`. Harmless on other platforms
/// (directories just don't exist and are skipped).
fn ensure_tool_path() {
    const KNOWN_DIRS: &[&str] = &[
        "/opt/homebrew/bin",  // Apple Silicon Homebrew
        "/opt/homebrew/sbin", // Apple Silicon Homebrew
        "/usr/local/bin",     // Intel Homebrew
        "/usr/local/sbin",    // Intel Homebrew
        "/opt/local/bin",     // MacPorts
        "/opt/local/sbin",    // MacPorts
    ];

    let current = crate::media_conversion_gate::delivery_path_env_or_empty();
    let mut entries: Vec<PathBuf> = std::env::split_paths(&current).collect();
    let mut changed = false;

    for dir in KNOWN_DIRS {
        let p = PathBuf::from(dir);
        if !p.is_dir() {
            continue;
        }
        if entries.iter().any(|e| e == &p) {
            continue;
        }
        // Prepend so Homebrew-installed tools win over any system stub (e.g.
        // /usr/bin/ffprobe).
        entries.insert(0, p);
        changed = true;
    }

    if changed {
        match std::env::join_paths(&entries) {
            Ok(joined) => {
                // SAFETY: Only invoked from [`init_ghost_mode`] inside [`INIT_GHOST_ENV`]
                // `Once`.
                unsafe {
                    std::env::set_var("PATH", joined);
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "process_lock_path",
                    format!("failed to join PATH entries for ghost-mode tools: {e}"),
                );
            }
        }
    }
}

/// Returns the central home for MFB metadata and transient files
/// (~/.`modern_format_boost`).
///
/// # Errors
/// Returns an error if the home directory cannot be determined.
pub fn get_mfb_root() -> Result<PathBuf> {
    match std::env::var(crate::constants::ENV_MFB_HOME_ROOT) {
        Ok(root) => {
            return usable_mfb_root_or_fallback(
                PathBuf::from(root),
                "MFB_HOME_ROOT",
                "Failed to create MFB_HOME_ROOT directory",
            );
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "process_lock_root",
                format!(
                    "failed to read {}: {e}; falling back to HOME",
                    crate::constants::ENV_MFB_HOME_ROOT
                ),
            );
        }
    }

    let home_root = std::env::var(crate::constants::ENV_HOME)
        .map(PathBuf::from)
        .or_else(|_| std::env::var(crate::constants::ENV_USERPROFILE).map(PathBuf::from))
        .map_err(|e| anyhow!("Could not find home directory environment variable: {e}"))
        .map(|h| h.join(".modern_format_boost"))?;
    usable_mfb_root_or_fallback(home_root, "home", "Failed to create MFB home directory")
}

fn usable_mfb_root_or_fallback(root: PathBuf, context: &str, create_msg: &str) -> Result<PathBuf> {
    match ensure_mfb_root_usable(&root) {
        Ok(()) => Ok(root),
        Err(primary_err) => {
            let fallback = crate::media_conversion_gate::delivery_temp_mfb_root_ssot();
            ensure_mfb_root_usable(&fallback).with_context(|| {
                format!(
                    "{create_msg}; primary {} unavailable at {} ({primary_err}); fallback {} also \
                     unavailable",
                    context,
                    root.display(),
                    fallback.display()
                )
            })?;
            crate::media_conversion_gate::delivery_runtime_path_audit(
                "mfb_root_fallback",
                &root,
                format!(
                    "{context} MFB root unavailable ({primary_err}); using {}",
                    fallback.display()
                ),
            );
            Ok(fallback)
        }
    }
}

fn ensure_mfb_root_usable(root: &Path) -> Result<()> {
    static WRITE_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fs::create_dir_all(root)?;
    // This check is called from image workers. A shared probe name lets one
    // worker remove another worker's probe and falsely marks a healthy root as
    // unavailable. PID + sequence also keeps concurrent MFB processes apart.
    let probe = root.join(format!(
        ".mfb_write_probe.{}.{}",
        std::process::id(),
        WRITE_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&probe, b"probe")?;
    fs::remove_file(&probe)?;
    Ok(())
}

/// Returns the central temporary storage for MFB, ensuring it exists.
///
/// # Errors
/// Returns an error if the MFB root cannot be determined or the temporary
/// directory cannot be created.
pub fn get_mfb_tmp_dir() -> Result<PathBuf> {
    let tmp = get_mfb_root()?.join("tmp");
    fs::create_dir_all(&tmp).context("Failed to create MFB tmp directory")?;
    Ok(tmp)
}

/// Generates a unique hex hash for a directory's canonical path using BLAKE3.
///
/// # Errors
/// Returns an error if the path cannot be canonicalized.
pub fn hash_path_to_hex(path: &Path) -> Result<String> {
    let abs_path = fs::canonicalize(path).with_context(|| {
        format!(
            "Failed to canonicalize path for hashing: {}",
            path.display()
        )
    })?;
    let path_str = abs_path.to_string_lossy();
    Ok(blake3::hash(path_str.as_bytes()).to_hex().to_string())
}

/// Attempts to acquire an exclusive advisory lock for a specific directory.
///
/// The lock file is stored in a central location
/// (~/.`modern_format_boost/locks`/) hashed by the directory's absolute path to
/// avoid polluting the user's data.
///
/// # Errors
/// Returns an error if the lock file cannot be created or the lock is already
/// held.
pub fn acquire_dir_lock(dir_path: &Path) -> Result<DirLock> {
    // 1. Get absolute, canonical path to ensure unique hashing
    let abs_path = fs::canonicalize(dir_path)
        .with_context(|| format!("Failed to canonicalize path: {}", dir_path.display()))?;
    let path_str = abs_path.to_string_lossy();

    {
        let mut held_locks = held_dir_locks_guard("process_lock_registry_acquire");
        if held_locks.contains(&abs_path) {
            return Err(anyhow!(
                "This directory is already being processed by this Modern Format Boost \
                 instance.\nLocked path: {}",
                abs_path.display()
            ));
        }
        held_locks.insert(abs_path.clone());
    }

    // 2. Generate a unique hash for this path using blake3
    let hash = blake3::hash(path_str.as_bytes()).to_hex();

    // 3. Prepare global lock directory (non-polluting)
    let lock_dir = get_mfb_root()?.join("locks");
    fs::create_dir_all(&lock_dir).context("Failed to create lock directory")?;

    let lock_file_path = lock_dir.join(format!("{hash}.lock"));

    // 4. Open/Create the lock file (use open+create+write without truncate to avoid
    //    breaking advisory locks held by other fds on the same inode)
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_file_path)
        .with_context(|| format!("Failed to create lock file at {}", lock_file_path.display()));
    let file = match file {
        Ok(file) => file,
        Err(err) => {
            held_dir_locks_guard("process_lock_registry_open_rollback").remove(&abs_path);
            return Err(err);
        }
    };

    // 5. Apply flock (Exclusive, Non-blocking)
    let fd = file.as_raw_fd();
    // SAFETY: Using libc directly for lightweight advisory locking.
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    if result != 0_i32 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            held_dir_locks_guard("process_lock_registry_flock_busy_rollback").remove(&abs_path);
            return Err(anyhow!(
                "This directory is already being processed by another Modern Format Boost \
                 instance.\nLocked path: {}",
                abs_path.display()
            ));
        }
        held_dir_locks_guard("process_lock_registry_flock_err_rollback").remove(&abs_path);
        return Err(anyhow!("Failed to acquire lock: {err}"));
    }

    Ok(DirLock {
        file,
        locked_path: abs_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    #[serial_test::serial]
    fn test_get_mfb_root() {
        let temp = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("MFB_HOME_ROOT", temp.path());
        }
        let root = get_mfb_root().unwrap();
        assert_eq!(root, temp.path());
        unsafe {
            std::env::remove_var("MFB_HOME_ROOT");
        }
    }

    #[test]
    fn test_hash_path_to_hex() {
        let temp = TempDir::new().unwrap();
        let path = temp.path();
        let hash = hash_path_to_hex(path).unwrap();
        assert_eq!(hash.len(), 64); // BLAKE3 hex hash length

        let hash2 = hash_path_to_hex(path).unwrap();
        assert_eq!(hash, hash2);
    }

    #[test]
    fn mfb_root_write_probe_is_concurrency_safe() {
        let temp = TempDir::new().unwrap();
        let root = Arc::new(temp.path().join("mfb_home"));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let root = Arc::clone(&root);
                std::thread::spawn(move || {
                    for _ in 0..64 {
                        ensure_mfb_root_usable(&root).unwrap();
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.join().unwrap();
        }
        assert!(
            fs::read_dir(root.as_ref()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".mfb_write_probe")
            }),
            "successful write probes must clean up their temporary files"
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_acquire_dir_lock() {
        let temp = TempDir::new().unwrap();
        let dir_to_lock = temp.path();

        // Mock MFB_HOME_ROOT to avoid polluting actual user home
        let mfb_home = temp.path().join("mfb_home");
        unsafe {
            std::env::set_var("MFB_HOME_ROOT", &mfb_home);
        }

        let _lock = acquire_dir_lock(dir_to_lock).expect("Failed to acquire first lock");

        // Attempting to lock the same directory again should fail
        let second_lock = acquire_dir_lock(dir_to_lock);
        assert!(second_lock.is_err());
        assert!(
            second_lock
                .unwrap_err()
                .to_string()
                .contains("already being processed")
        );

        unsafe {
            std::env::remove_var("MFB_HOME_ROOT");
        }
    }

    #[test]
    #[serial_test::serial]
    fn test_acquire_dir_lock_releases_on_drop() {
        let temp = TempDir::new().unwrap();
        let dir_to_lock = temp.path();

        let mfb_home = temp.path().join("mfb_home");
        unsafe {
            std::env::set_var("MFB_HOME_ROOT", &mfb_home);
        }

        {
            let _lock = acquire_dir_lock(dir_to_lock).expect("Failed to acquire first lock");
        }

        let second_lock = acquire_dir_lock(dir_to_lock);
        assert!(
            second_lock.is_ok(),
            "lock should be reacquirable after drop"
        );

        unsafe {
            std::env::remove_var("MFB_HOME_ROOT");
        }
    }
}
