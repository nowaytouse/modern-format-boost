use anyhow::{Context, Result, anyhow};
use std::fs::{self, File};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Initializes Ghost Mode (Zero Pollution) by setting up the process lock.
/// This ensures zero-pollution even when the binary is used independently of any scripts.
///
/// # Errors
/// Returns an error if the MFB temporary directory cannot be created.
pub fn init_ghost_mode() -> Result<()> {
    let tmp = get_mfb_tmp_dir()?;
    // SAFETY: Single-threaded initialization context.
    unsafe { std::env::set_var("TMPDIR", &tmp) };
    Ok(())
}

/// Returns the central home for MFB metadata and transient files (~/.`modern_format_boost`).
///
/// # Errors
/// Returns an error if the home directory cannot be determined.
pub fn get_mfb_root() -> Result<PathBuf> {
    if let Ok(root) = std::env::var("MFB_HOME_ROOT") {
        let root = PathBuf::from(root);
        fs::create_dir_all(&root).context("Failed to create MFB_HOME_ROOT directory")?;
        return Ok(root);
    }

    std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| anyhow!("Could not find home directory environment variable"))
        .map(|h| h.join(".modern_format_boost"))
}

/// Returns the central temporary storage for MFB, ensuring it exists.
///
/// # Errors
/// Returns an error if the MFB root cannot be determined or the temporary directory cannot be created.
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
/// The lock file is stored in a central location (~/.`modern_format_boost/locks`/)
/// hashed by the directory's absolute path to avoid polluting the user's data.
///
/// # Errors
/// Returns an error if the lock file cannot be created or the lock is already held.
pub fn acquire_dir_lock(dir_path: &Path) -> Result<File> {
    // 1. Get absolute, canonical path to ensure unique hashing
    let abs_path = fs::canonicalize(dir_path)
        .with_context(|| format!("Failed to canonicalize path: {}", dir_path.display()))?;
    let path_str = abs_path.to_string_lossy();

    // 2. Generate a unique hash for this path using blake3
    let hash = blake3::hash(path_str.as_bytes()).to_hex();

    // 3. Prepare global lock directory (non-polluting)
    let lock_dir = get_mfb_root()?.join("locks");
    fs::create_dir_all(&lock_dir).context("Failed to create lock directory")?;

    let lock_file_path = lock_dir.join(format!("{hash}.lock"));

    // 4. Open/Create the lock file
    let file = File::create(&lock_file_path)
        .with_context(|| format!("Failed to create lock file at {}", lock_file_path.display()))?;

    // 5. Apply flock (Exclusive, Non-blocking)
    let fd = file.as_raw_fd();
    // SAFETY: Using libc directly for lightweight advisory locking.
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    if result != 0_i32 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(anyhow!(
                "This directory is already being processed by another Modern Format Boost instance.\nLocked path: {}",
                abs_path.display()
            ));
        }
        return Err(anyhow!("Failed to acquire lock: {err}"));
    }

    Ok(file)
}
