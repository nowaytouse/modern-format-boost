use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::os::unix::io::AsRawFd;
use anyhow::{Context, Result, anyhow};

/// Attempts to acquire an exclusive advisory lock for a specific directory.
/// The lock file is stored in a central location (~/.modern_format_boost/locks/)
/// hashed by the directory's absolute path to avoid polluting the user's data.
pub fn acquire_dir_lock(dir_path: &Path) -> Result<File> {
    // 1. Get absolute, canonical path to ensure unique hashing
    // canonicalize requires the path to exist. 
    let abs_path = fs::canonicalize(dir_path)
        .with_context(|| format!("Failed to canonicalize path: {:?}", dir_path))?;
    let path_str = abs_path.to_string_lossy();

    // 2. Generate a unique hash for this path using blake3
    let hash = blake3::hash(path_str.as_bytes()).to_hex();
    
    // 3. Prepare global lock directory (non-polluting)
    let home = std::env::var("HOME").map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| anyhow!("Could not find home directory environment variable"))?;
    
    let lock_dir = home.join(".modern_format_boost").join("locks");
    fs::create_dir_all(&lock_dir).context("Failed to create lock directory")?;

    let lock_file_path = lock_dir.join(format!("{}.lock", hash));

    // 4. Open/Create the lock file
    let file = File::create(&lock_file_path)
        .with_context(|| format!("Failed to create lock file at {:?}", lock_file_path))?;

    // 5. Apply flock (Exclusive, Non-blocking)
    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    if result != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(anyhow!(
                "This directory is already being processed by another Modern Format Boost instance.\nLocked path: {:?}", 
                abs_path
            ));
        }
        return Err(anyhow!("Failed to acquire lock: {}", err));
    }

    Ok(file)
}
