use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::ops::Deref;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};

static INIT_GHOST_ENV: Once = Once::new();

struct BoundMfbRoot {
    path: PathBuf,
    identity: Option<MfbRootIdentity>,
    write_checked: bool,
}

struct MfbRootIdentity {
    canonical_path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

static BOUND_MFB_ROOT: Mutex<Option<BoundMfbRoot>> = Mutex::new(None);

impl BoundMfbRoot {
    fn new(path: PathBuf) -> Result<Self> {
        ensure_mfb_root_usable(&path)
            .with_context(|| format!("prepare formal MFB root {}", path.display()))?;
        let identity = MfbRootIdentity::capture(&path)?;
        Ok(Self {
            path,
            identity: Some(identity),
            write_checked: true,
        })
    }

    fn inspect(path: PathBuf) -> Result<Self> {
        let identity = inspect_mfb_root_identity(&path)?;
        Ok(Self {
            path,
            identity,
            write_checked: false,
        })
    }

    fn inspect_bound(&mut self) -> Result<()> {
        if let Some(identity) = &self.identity {
            identity.validate(&self.path)
        } else {
            self.identity = inspect_mfb_root_identity(&self.path)?;
            Ok(())
        }
    }

    fn prepare_write(&mut self) -> Result<()> {
        if let Some(identity) = &self.identity {
            identity.validate(&self.path)?;
            if !self.write_checked {
                // A read-bound directory must not be recreated if it disappears.
                probe_mfb_root_write(&self.path).with_context(|| {
                    format!("prepare inspected formal MFB root {}", self.path.display())
                })?;
                identity.validate(&self.path)?;
                self.write_checked = true;
            }
        } else {
            // A read-only lookup selected this absent path without creating it.
            // Only its first writer may initialize it; environment changes are
            // never consulted again for this process's selected authority.
            let initialized = Self::new(self.path.clone())?;
            *self = initialized;
        }
        Ok(())
    }
}

impl MfbRootIdentity {
    fn capture(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("resolve formal MFB root {}", path.display()))?;
        let metadata = fs::metadata(&canonical_path)
            .with_context(|| format!("inspect formal MFB root {}", path.display()))?;
        anyhow::ensure!(metadata.is_dir(), "formal MFB root is not a directory");
        Ok(Self {
            canonical_path,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }

    fn validate(&self, path: &Path) -> Result<()> {
        // Never create or select another directory after binding: a new inode
        // at the same path would split both durable state and directory locks.
        let canonical = path.canonicalize().with_context(|| {
            format!(
                "bound formal MFB root is unavailable; state and locks retained: {}",
                path.display()
            )
        })?;
        anyhow::ensure!(
            canonical == self.canonical_path,
            "bound formal MFB root changed its target; refusing another state or lock root: {}",
            path.display()
        );
        let metadata = fs::metadata(path)
            .with_context(|| format!("inspect bound formal MFB root {}", path.display()))?;
        anyhow::ensure!(
            metadata.is_dir(),
            "bound formal MFB root is no longer a directory: {}",
            path.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                metadata.dev() == self.device && metadata.ino() == self.inode,
                "bound formal MFB root was replaced; refusing another state or lock root: {}",
                path.display()
            );
        }
        Ok(())
    }
}

fn inspect_mfb_root_identity(path: &Path) -> Result<Option<MfbRootIdentity>> {
    match fs::symlink_metadata(path) {
        Ok(_) => MfbRootIdentity::capture(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Distinguish an absent subtree from an unreadable ancestor, a
            // regular-file ancestor, or a dangling symlink. None authorizes no
            // creation; it only preserves the selected path for a future writer.
            for ancestor in path.ancestors().skip(1) {
                match fs::symlink_metadata(ancestor) {
                    Ok(_) => {
                        let metadata = fs::metadata(ancestor).with_context(|| {
                            format!("inspect formal MFB root ancestor {}", ancestor.display())
                        })?;
                        anyhow::ensure!(
                            metadata.is_dir(),
                            "formal MFB root ancestor is not a directory: {}",
                            ancestor.display()
                        );
                        return Ok(None);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("inspect formal MFB root ancestor {}", ancestor.display())
                        });
                    }
                }
            }
            Err(anyhow!(
                "formal MFB root has no accessible existing ancestor: {}",
                path.display()
            ))
        }
        Err(error) => {
            Err(error).with_context(|| format!("inspect formal MFB root {}", path.display()))
        }
    }
}

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

/// Return the writable formal MFB root selected for this process.
///
/// The configured absolute path spelling is preserved. Later environment changes
/// cannot redirect existing state or locks; a missing or replaced bound directory
/// is an error, never a reason to create another root or use temporary storage.
///
/// # Errors
/// Rejects invalid configuration, missing home, inaccessible storage, replacement,
/// or a poisoned binding lock. An initial error does not establish a binding;
/// an earlier read-only selection remains fixed even if its first write fails.
pub fn get_mfb_root() -> Result<PathBuf> {
    let mut binding = BOUND_MFB_ROOT
        .lock()
        .map_err(|_| anyhow!("formal MFB root binding lock is poisoned; state retained"))?;
    if let Some(root) = binding.as_mut() {
        root.prepare_write()?;
        return Ok(root.path.clone());
    }
    let root = BoundMfbRoot::new(configured_mfb_root()?)?;
    let path = root.path.clone();
    *binding = Some(root);
    drop(binding);
    Ok(path)
}

/// Inspect the selected formal root without creating directories or write probes.
///
/// An absent root still fixes this process's selected path for a later writer.
/// An existing root also fixes its directory identity, but is not treated as
/// write-checked until `get_mfb_root` successfully performs the initial probe.
///
/// # Errors
/// Rejects invalid configuration, unreadable/non-directory paths, changed bound
/// identities, or a poisoned binding lock; none is reported as absent state.
pub fn inspect_mfb_root_path() -> Result<PathBuf> {
    let mut binding = BOUND_MFB_ROOT
        .lock()
        .map_err(|_| anyhow!("formal MFB root binding lock is poisoned; state retained"))?;
    if let Some(root) = binding.as_mut() {
        root.inspect_bound()?;
        return Ok(root.path.clone());
    }
    let root = BoundMfbRoot::inspect(configured_mfb_root()?)?;
    let path = root.path.clone();
    *binding = Some(root);
    drop(binding);
    Ok(path)
}

/// Locate startup environment configuration before formal state is selected.
///
/// This neither creates nor binds a state directory. Only single-threaded
/// bootstrap may use it; normal state and lock readers must use the bound APIs.
///
/// # Errors
/// Rejects invalid settings or an attempt to bootstrap after state was bound.
pub fn configuration_bootstrap_root() -> Result<PathBuf> {
    let binding = BOUND_MFB_ROOT
        .lock()
        .map_err(|_| anyhow!("formal MFB root binding lock is poisoned; state retained"))?;
    anyhow::ensure!(
        binding.is_none(),
        "local environment must be loaded before formal state or locks are bound"
    );
    // Keep the bootstrap check and configuration read under the same binding
    // lock; another thread must not establish an authority between them.
    let root = configured_mfb_root();
    drop(binding);
    root
}

fn absolute_root_setting(name: &str, value: String) -> Result<PathBuf> {
    anyhow::ensure!(!value.trim().is_empty(), "{name} must not be empty");
    let path = PathBuf::from(value);
    anyhow::ensure!(path.is_absolute(), "{name} must be an absolute path");
    Ok(path)
}

fn configured_mfb_root() -> Result<PathBuf> {
    // Do not call root-dependent audit logging while the binding mutex is held.
    match std::env::var(crate::constants::ENV_MFB_HOME_ROOT) {
        Ok(root) => return absolute_root_setting(crate::constants::ENV_MFB_HOME_ROOT, root),
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => return Err(error).context("MFB_HOME_ROOT must contain valid Unicode"),
    }
    let home = match std::env::var(crate::constants::ENV_HOME) {
        Ok(home) => absolute_root_setting(crate::constants::ENV_HOME, home)?,
        #[cfg(windows)]
        Err(std::env::VarError::NotPresent) => absolute_root_setting(
            crate::constants::ENV_USERPROFILE,
            std::env::var(crate::constants::ENV_USERPROFILE)
                .context("formal MFB root requires HOME or USERPROFILE")?,
        )?,
        #[cfg(not(windows))]
        Err(std::env::VarError::NotPresent) => {
            return Err(anyhow!("formal MFB root requires MFB_HOME_ROOT or HOME"));
        }
        Err(error) => return Err(error).context("HOME must contain valid Unicode"),
    };
    Ok(home.join(crate::constants::MFB_DEFAULT_HOME_DIRNAME))
}

fn finish_root_probe(probe: &Path, write: std::io::Result<()>) -> Result<()> {
    match (write, fs::remove_file(probe)) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(write), Err(cleanup)) => Err(anyhow!(
            "formal MFB root write probe failed: {write}; probe cleanup also failed: {cleanup}"
        )),
        (Err(error), Ok(())) => Err(error).context("write formal MFB root probe"),
        (Ok(()), Err(error)) => Err(error).context("remove formal MFB root probe"),
    }
}

fn ensure_mfb_root_usable(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    probe_mfb_root_write(root)
}

fn probe_mfb_root_write(root: &Path) -> Result<()> {
    static WRITE_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    // This check is called from image workers. A shared probe name lets one
    // worker remove another worker's probe and falsely marks a healthy root as
    // unavailable. PID + sequence also keeps concurrent MFB processes apart.
    let probe = root.join(format!(
        ".mfb_write_probe.{}.{}",
        std::process::id(),
        WRITE_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    let written = file.write_all(b"probe");
    drop(file);
    finish_root_probe(&probe, written)
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
    acquire_canonical_dir_lock(abs_path)
}

/// Lock an adjacent output before creating it, using the same identity as an existing directory.
pub fn acquire_output_dir_lock(dir_path: &Path) -> Result<DirLock> {
    let absolute = match fs::symlink_metadata(dir_path) {
        Ok(metadata) if metadata.file_type().is_dir() => dir_path.canonicalize()?,
        Ok(_) => {
            return Err(anyhow!(
                "Output lock target is not a real directory: {}",
                dir_path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = dir_path
                .parent()
                .context("Output lock target has no parent")?;
            let name = dir_path
                .file_name()
                .context("Output lock target has no file name")?;
            parent.canonicalize()?.join(name)
        }
        Err(error) => return Err(error).context("Inspect prospective output lock target"),
    };
    acquire_canonical_dir_lock(absolute)
}

fn acquire_canonical_dir_lock(abs_path: PathBuf) -> Result<DirLock> {
    let path_str = abs_path.to_string_lossy();

    // 2. Generate a unique hash for this path using blake3
    let hash = blake3::hash(path_str.as_bytes()).to_hex();

    // 3. Prepare global lock directory (non-polluting)
    let lock_dir = get_mfb_root()?.join("locks");
    fs::create_dir_all(&lock_dir).context("Failed to create lock directory")?;

    let lock_file_path = lock_dir.join(format!("{hash}.lock"));

    // Register only after every fallible lock-directory setup step. Otherwise a
    // transient MFB_HOME_ROOT failure poisons this process with a lock that was
    // never acquired.
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
        if crate::common_utils::isolated_test_process() {
            return;
        }
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
    #[serial_test::serial]
    fn mfb_root_rejects_missing_home_without_temporary_state() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let saved_root = std::env::var_os("MFB_HOME_ROOT");
        let saved_home = std::env::var_os("HOME");
        let saved_profile = std::env::var_os("USERPROFILE");
        unsafe {
            std::env::remove_var("MFB_HOME_ROOT");
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }

        let error = get_mfb_root().unwrap_err();
        assert!(error.to_string().contains("formal MFB root requires"));

        unsafe {
            match saved_root {
                Some(value) => std::env::set_var("MFB_HOME_ROOT", value),
                None => std::env::remove_var("MFB_HOME_ROOT"),
            }
            match saved_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match saved_profile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn mfb_root_bootstrap_configuration_precedes_binding_without_creating_storage() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let bootstrap = temp.path().join("configuration-location");
        let selected = temp.path().join("resolved-state");
        let _initial =
            crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", bootstrap.to_str().unwrap());
        assert_eq!(configuration_bootstrap_root().unwrap(), bootstrap);
        assert!(!bootstrap.exists());
        let _resolved =
            crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", selected.to_str().unwrap());
        assert_eq!(get_mfb_root().unwrap(), selected);
        assert!(configuration_bootstrap_root().is_err());
        assert!(!bootstrap.exists());
        let _late =
            crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", bootstrap.to_str().unwrap());
        assert_eq!(get_mfb_root().unwrap(), selected);
    }

    #[test]
    #[serial_test::serial]
    fn mfb_root_rejects_invalid_explicit_settings_without_binding_or_fallback() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let _home = crate::common_utils::EnvGuard::set("HOME", home.to_str().unwrap());
        let blocked = temp.path().join("blocked");
        fs::write(&blocked, b"unrelated user-owned bytes").unwrap();
        for value in ["", "   ", "relative-state", blocked.to_str().unwrap()] {
            let _root = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", value);
            assert!(get_mfb_root().is_err(), "invalid setting {value:?}");
            assert!(fs::read_dir(&home).unwrap().next().is_none());
            assert_eq!(fs::read(&blocked).unwrap(), b"unrelated user-owned bytes");
        }
        let valid = temp.path().join("valid-state");
        let _root = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", valid.to_str().unwrap());
        assert_eq!(get_mfb_root().unwrap(), valid);
        assert!(fs::read_dir(&valid).unwrap().next().is_none());
    }

    #[test]
    #[serial_test::serial]
    fn mfb_root_inspection_fixes_absent_path_without_creating_state() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let selected = temp.path().join("missing/state");
        let other = temp.path().join("other-state");
        let _selected =
            crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", selected.to_str().unwrap());
        assert_eq!(inspect_mfb_root_path().unwrap(), selected);
        assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
        let _other = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", other.to_str().unwrap());
        assert_eq!(inspect_mfb_root_path().unwrap(), selected);
        assert!(!selected.exists());
        assert_eq!(get_mfb_root().unwrap(), selected);
        assert!(!other.exists());
        let modified = fs::metadata(&selected).unwrap().modified().unwrap();
        for _ in 0..4 {
            assert_eq!(inspect_mfb_root_path().unwrap(), selected);
        }
        assert!(fs::read_dir(&selected).unwrap().next().is_none());
        assert_eq!(
            fs::metadata(&selected).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    #[serial_test::serial]
    fn mfb_root_inspection_of_existing_state_does_not_claim_write_probe() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("state");
        fs::create_dir(&root).unwrap();
        let _root = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", root.to_str().unwrap());
        let before = fs::metadata(&root).unwrap().modified().unwrap();
        assert_eq!(inspect_mfb_root_path().unwrap(), root);
        assert_eq!(fs::metadata(&root).unwrap().modified().unwrap(), before);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        // The isolated process has not issued a write probe. Reserve its first
        // probe name to prove a subsequent writer still checks create_new and
        // never overwrites a pre-existing file.
        let collision = root.join(format!(".mfb_write_probe.{}.0", std::process::id()));
        fs::write(&collision, b"preserve existing bytes").unwrap();
        assert!(get_mfb_root().is_err());
        assert_eq!(fs::read(&collision).unwrap(), b"preserve existing bytes");
        assert_eq!(inspect_mfb_root_path().unwrap(), root);
        fs::remove_file(collision).unwrap();
        assert_eq!(get_mfb_root().unwrap(), root);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
    }

    #[test]
    #[serial_test::serial]
    fn mfb_root_inspection_rejects_non_directory_without_switching_selected_path() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let selected = temp.path().join("state");
        let other = temp.path().join("other-state");
        let _selected =
            crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", selected.to_str().unwrap());
        assert_eq!(inspect_mfb_root_path().unwrap(), selected);
        fs::write(&selected, b"user-owned file").unwrap();
        let _other = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", other.to_str().unwrap());
        assert!(inspect_mfb_root_path().is_err());
        assert!(get_mfb_root().is_err());
        assert!(!other.exists());
        assert_eq!(fs::read(&selected).unwrap(), b"user-owned file");
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn mfb_root_inspection_distinguishes_broken_ancestors_from_absence() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("not-directory");
        fs::write(&file, b"preserve ancestor").unwrap();
        let alias = temp.path().join("dangling");
        std::os::unix::fs::symlink(temp.path().join("unavailable"), &alias).unwrap();
        for selected in [file, alias.clone(), alias.join("child")] {
            let _root =
                crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", selected.to_str().unwrap());
            assert!(inspect_mfb_root_path().is_err());
        }
        assert!(!temp.path().join("unavailable").exists());
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn mfb_root_rejects_non_unicode_override_instead_of_using_home() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        use std::os::unix::ffi::OsStringExt;
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let _home = crate::common_utils::EnvGuard::set("HOME", home.to_str().unwrap());
        let previous = std::env::var_os("MFB_HOME_ROOT");
        // SAFETY: This test runs alone in its isolated subprocess and preserves
        // the original OS string without lossy conversion.
        unsafe {
            std::env::set_var(
                "MFB_HOME_ROOT",
                std::ffi::OsString::from_vec(vec![b'/', 0xff]),
            );
        }
        let result = get_mfb_root();
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("MFB_HOME_ROOT", previous);
            } else {
                std::env::remove_var("MFB_HOME_ROOT");
            }
        }
        assert!(result.unwrap_err().to_string().contains("valid Unicode"));
        assert!(fs::read_dir(&home).unwrap().next().is_none());
    }

    #[test]
    #[serial_test::serial]
    fn mfb_root_binding_ignores_later_environment_and_does_not_probe_again() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("first");
        let alternate = temp.path().join("alternate");
        let _first = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", first.to_str().unwrap());
        assert_eq!(get_mfb_root().unwrap(), first);
        let initial = fs::metadata(&first).unwrap().modified().unwrap();
        let _alternate =
            crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", alternate.to_str().unwrap());
        for _ in 0..16 {
            assert_eq!(get_mfb_root().unwrap(), first);
        }
        assert!(!alternate.exists());
        assert!(fs::read_dir(&first).unwrap().next().is_none());
        assert_eq!(fs::metadata(&first).unwrap().modified().unwrap(), initial);
    }

    #[test]
    #[serial_test::serial]
    fn mfb_root_binding_rejects_missing_and_replaced_directory_without_new_locks() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("state");
        let original = temp.path().join("retained-original");
        let target = temp.path().join("media");
        fs::create_dir(&target).unwrap();
        let _root = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", root.to_str().unwrap());
        let lock = acquire_dir_lock(&target).unwrap();
        fs::rename(&root, &original).unwrap();
        assert!(get_mfb_root().is_err());
        assert!(
            !root.exists(),
            "a missing bound directory must not be recreated"
        );
        fs::create_dir(&root).unwrap();
        let error = get_mfb_root().unwrap_err();
        assert!(error.to_string().contains("replaced"));
        assert!(acquire_output_dir_lock(&temp.path().join("new-output")).is_err());
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        drop(lock);
        fs::remove_dir(&root).unwrap();
        fs::rename(&original, &root).unwrap();
        assert_eq!(get_mfb_root().unwrap(), root);
        assert!(acquire_dir_lock(&target).is_ok());
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn mfb_root_binding_preserves_alias_spelling_but_rejects_retargeting() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let alias = temp.path().join("configured");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let _root = crate::common_utils::EnvGuard::set("MFB_HOME_ROOT", alias.to_str().unwrap());
        assert_eq!(get_mfb_root().unwrap(), alias);
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        assert!(
            get_mfb_root()
                .unwrap_err()
                .to_string()
                .contains("changed its target")
        );
        assert!(fs::read_dir(&second).unwrap().next().is_none());
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
        if crate::common_utils::isolated_test_process() {
            return;
        }
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
        if crate::common_utils::isolated_test_process() {
            return;
        }
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

    #[test]
    #[serial_test::serial]
    fn output_lock_precedes_creation_and_keeps_existing_directory_identity() {
        if crate::common_utils::isolated_test_process() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let old = std::env::var_os("MFB_HOME_ROOT");
        unsafe {
            std::env::set_var("MFB_HOME_ROOT", temp.path().join("state"));
        }
        let output = temp.path().join("output");
        let lock = acquire_output_dir_lock(&output).unwrap();
        assert!(!output.exists());
        fs::create_dir(&output).unwrap();
        assert!(acquire_dir_lock(&output).is_err());
        drop(lock);
        assert!(acquire_dir_lock(&output).is_ok());
        unsafe {
            if let Some(old) = old {
                std::env::set_var("MFB_HOME_ROOT", old);
            } else {
                std::env::remove_var("MFB_HOME_ROOT");
            }
        }
    }
}
