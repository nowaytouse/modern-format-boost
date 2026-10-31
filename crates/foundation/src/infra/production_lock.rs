//! Per-user, cross-process exclusion for real media-processing tasks.

use anyhow::Result;
#[cfg(unix)]
use anyhow::{Context, bail};
#[cfg(unix)]
use std::fs::{self, File, OpenOptions};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[cfg(unix)]
const LEASE_FD_ENV: &str = "MFB_PRODUCTION_TASK_LEASE_FD";
const BUSY_MARKER: &str = "MFB_PRODUCTION_TASK_BUSY";

#[derive(Debug)]
pub struct ProductionTaskBusy;

impl std::fmt::Display for ProductionTaskBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{BUSY_MARKER}: another Modern Format Boost production task is active"
        )
    }
}

impl std::error::Error for ProductionTaskBusy {}

#[must_use]
pub fn is_production_task_busy(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ProductionTaskBusy>().is_some()
}

#[cfg(unix)]
struct LeaseInner {
    file: File,
    inherited_file: Option<File>,
}

#[cfg(unix)]
impl LeaseInner {
    fn descriptor(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.inherited_file
            .as_ref()
            .unwrap_or(&self.file)
            .as_raw_fd()
    }
}

/// A task lease whose lock remains held while any inheriting worker is alive.
pub struct ProductionLease {
    #[cfg(unix)]
    inner: Arc<LeaseInner>,
}

#[cfg(unix)]
static ACTIVE_LEASE: OnceLock<Mutex<Weak<LeaseInner>>> = OnceLock::new();

#[cfg(unix)]
fn active_lease_slot() -> &'static Mutex<Weak<LeaseInner>> {
    ACTIVE_LEASE.get_or_init(|| Mutex::new(Weak::new()))
}

#[cfg(unix)]
fn register_active(inner: &Arc<LeaseInner>) -> Result<()> {
    *active_lease_slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("production task lease registry poisoned"))? =
        Arc::downgrade(inner);
    Ok(())
}

/// Acquire the per-user production lease, or validate an explicitly inherited
/// lease from an internal worker parent.
pub fn acquire_production_lease() -> Result<ProductionLease> {
    #[cfg(unix)]
    {
        let path = stable_lock_path()?;
        let lease = match std::env::var_os(LEASE_FD_ENV) {
            Some(value) => acquire_inherited(&path, &value)?,
            None => acquire_new(&path, &native_writer_lock_path()?)?,
        };
        register_active(&lease.inner)?;
        Ok(lease)
    }
    #[cfg(not(unix))]
    {
        Ok(ProductionLease {})
    }
}

/// Stable per-user lock file retained by the detached native `PhotoKit` helper.
#[cfg(unix)]
pub(crate) fn native_writer_lock_path() -> Result<PathBuf> {
    Ok(stable_lock_dir()?.join("native-photos.lock"))
}

/// Attach the active lease to one explicitly managed worker command.
///
/// Other commands inherit no marker; the selected descriptor is made
/// inheritable only in the child immediately before `exec`.
pub fn inherit_active_lease(command: &mut Command) -> Result<()> {
    #[cfg(unix)]
    {
        command.env_remove(LEASE_FD_ENV);
        let active = active_lease_slot()
            .lock()
            .map_err(|_| anyhow::anyhow!("production task lease registry poisoned"))?
            .upgrade();
        let Some(active) = active else {
            return Ok(());
        };
        attach_lease(command, &active);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        Ok(())
    }
}

#[cfg(unix)]
fn attach_lease(command: &mut Command, active: &LeaseInner) {
    use std::os::unix::process::CommandExt;

    let descriptor = active.descriptor();
    // `pre_exec` changes only FD_CLOEXEC for the lease descriptor. It never
    // assigns a fixed descriptor number or overwrites an unrelated FD.
    unsafe {
        command.pre_exec(move || {
            let flags = libc::fcntl(descriptor, libc::F_GETFD);
            if flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.env(LEASE_FD_ENV, descriptor.to_string());
}

#[cfg(unix)]
fn stable_lock_path() -> Result<PathBuf> {
    Ok(stable_lock_dir()?.join("production-task.lock"))
}

#[cfg(unix)]
fn stable_lock_dir() -> Result<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::DirBuilderExt;

    let uid = unsafe { libc::getuid() };
    let recommended = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let initial = usize::try_from(recommended).unwrap_or(1024).max(1024);
    let mut buffer = vec![0_u8; initial];
    let (entry, result) = loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::zeroed();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &raw mut result,
            )
        };
        if status == libc::ERANGE {
            let next = buffer
                .len()
                .checked_mul(2)
                .context("passwd buffer size overflow while resolving production lock path")?;
            buffer
                .try_reserve_exact(next - buffer.len())
                .context("grow passwd buffer for production lock path")?;
            buffer.resize(next, 0);
            continue;
        }
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status))
                .context("resolve real user home for production task lock");
        }
        break (unsafe { entry.assume_init() }, result);
    };
    if result.is_null() {
        bail!("resolve real user home for production task lock: uid has no passwd entry");
    }
    if entry.pw_dir.is_null() {
        bail!("resolve real user home for production task lock: passwd home is empty");
    }
    let home = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe {
        CStr::from_ptr(entry.pw_dir).to_bytes()
    }));
    anyhow::ensure!(home.is_absolute(), "real user home is not absolute");

    #[cfg(target_os = "macos")]
    let app_data = home.join("Library/Application Support/Modern Format Boost");
    #[cfg(not(target_os = "macos"))]
    let app_data = home.join(".local/state/modern_format_boost");

    let lock_dir = app_data.join("locks");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    match builder.create(&lock_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("create per-user production task lock directory"),
    }
    ensure_private_directory(&app_data)?;
    ensure_private_directory(&lock_dir)?;
    Ok(lock_dir)
}

#[cfg(unix)]
fn ensure_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect production task lock directory {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "production task lock path is not a real directory: {}",
        path.display()
    );
    anyhow::ensure!(
        metadata.uid() == unsafe { libc::getuid() },
        "production task lock directory has a different owner: {}",
        path.display()
    );
    anyhow::ensure!(
        metadata.permissions().mode().trailing_zeros() >= 6,
        "production task lock directory is not private: {}",
        path.display()
    );
    Ok(())
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open production task lock {}", path.display()))?;
    let metadata = file
        .metadata()
        .context("inspect production task lock file")?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::getuid() }
            && metadata.nlink() == 1
            && metadata.permissions().mode().trailing_zeros() >= 6,
        "production task lock is not a regular file owned by this user"
    );
    verify_path_identity(path, &metadata)?;
    Ok(file)
}

#[cfg(unix)]
fn verify_path_identity(path: &Path, opened: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let current = fs::symlink_metadata(path)
        .with_context(|| format!("recheck production task lock path {}", path.display()))?;
    anyhow::ensure!(
        current.is_file()
            && !current.file_type().is_symlink()
            && current.dev() == opened.dev()
            && current.ino() == opened.ino(),
        "production task lock path changed during acquisition"
    );
    Ok(())
}

#[cfg(unix)]
fn try_lock(fd: std::os::fd::RawFd) -> std::io::Result<bool> {
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) || error.raw_os_error() == Some(libc::EAGAIN)
    {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn native_lock_short<T: TryInto<libc::c_short>>(value: T) -> std::io::Result<libc::c_short> {
    value.try_into().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "native record-lock constant does not fit c_short",
        )
    })
}

#[cfg(unix)]
fn try_native_writer_lock(fd: std::os::fd::RawFd) -> std::io::Result<bool> {
    let mut lock = std::mem::MaybeUninit::<libc::flock>::zeroed();
    let lock = unsafe {
        let lock = lock.assume_init_mut();
        lock.l_type = native_lock_short(libc::F_WRLCK)?;
        lock.l_whence = native_lock_short(libc::SEEK_SET)?;
        lock
    };
    if unsafe { libc::fcntl(fd, libc::F_SETLK, lock) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EACCES) || error.raw_os_error() == Some(libc::EAGAIN) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn unlock_native_writer_lock(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    let mut lock = std::mem::MaybeUninit::<libc::flock>::zeroed();
    let lock = unsafe {
        let lock = lock.assume_init_mut();
        lock.l_type = native_lock_short(libc::F_UNLCK)?;
        lock.l_whence = native_lock_short(libc::SEEK_SET)?;
        lock
    };
    if unsafe { libc::fcntl(fd, libc::F_SETLK, lock) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn ensure_native_writer_idle(path: &Path) -> Result<()> {
    use std::os::fd::AsRawFd;

    let file = open_lock_file(path)?;
    if !try_native_writer_lock(file.as_raw_fd()).context("check native PhotoKit writer lock")? {
        return Err(anyhow::Error::new(ProductionTaskBusy));
    }
    unlock_native_writer_lock(file.as_raw_fd()).context("release native PhotoKit writer probe")?;
    Ok(())
}

#[cfg(unix)]
fn acquire_at(path: &Path) -> Result<ProductionLease> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let file = open_lock_file(path)?;
    if !try_lock(file.as_raw_fd()).context("acquire production task flock")? {
        return Err(anyhow::Error::new(ProductionTaskBusy));
    }
    if let Err(error) = verify_path_identity(path, &file.metadata()?) {
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        return Err(error);
    }
    let inherited_fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if inherited_fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("duplicate production task descriptor for worker inheritance");
    }
    Ok(ProductionLease {
        inner: Arc::new(LeaseInner {
            file,
            inherited_file: Some(unsafe { File::from_raw_fd(inherited_fd) }),
        }),
    })
}

#[cfg(unix)]
fn acquire_new(global_path: &Path, native_writer_path: &Path) -> Result<ProductionLease> {
    let lease = acquire_at(global_path)?;
    ensure_native_writer_idle(native_writer_path)?;
    Ok(lease)
}

#[cfg(unix)]
fn acquire_inherited(path: &Path, value: &std::ffi::OsStr) -> Result<ProductionLease> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let text = std::str::from_utf8(value.as_bytes())
        .context("invalid inherited production task descriptor marker")?;
    let fd: std::os::fd::RawFd = text
        .parse()
        .context("invalid inherited production task descriptor marker")?;
    anyhow::ensure!(fd >= 3, "invalid inherited production task descriptor");

    let mut inherited_stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    if unsafe { libc::fstat(fd, inherited_stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("inherited production task descriptor is not open");
    }
    let inherited_stat = unsafe { inherited_stat.assume_init() };
    anyhow::ensure!(
        inherited_stat.st_mode & libc::S_IFMT == libc::S_IFREG
            && inherited_stat.st_uid == unsafe { libc::getuid() },
        "inherited production task descriptor is not a user-owned regular file"
    );

    let probe = open_lock_file(path)?;
    let mut expected = std::mem::MaybeUninit::<libc::stat>::zeroed();
    if unsafe { libc::fstat(probe.as_raw_fd(), expected.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("inspect production task lock identity");
    }
    let expected = unsafe { expected.assume_init() };
    anyhow::ensure!(
        inherited_stat.st_dev == expected.st_dev && inherited_stat.st_ino == expected.st_ino,
        "inherited production task descriptor does not identify the stable lock"
    );
    if try_lock(probe.as_raw_fd()).context("verify inherited production task lock")? {
        unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_UN) };
        bail!("inherited production task descriptor does not hold the lock");
    }
    if !try_lock(fd).context("confirm inherited production task lock ownership")? {
        bail!("inherited production task descriptor is not the active lock owner");
    }

    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("restrict inherited production descriptor to this worker");
    }
    let file = unsafe { File::from_raw_fd(fd) };
    verify_path_identity(path, &file.metadata()?)?;
    Ok(ProductionLease {
        inner: Arc::new(LeaseInner {
            file,
            inherited_file: None,
        }),
    })
}

#[cfg(not(unix))]
impl ProductionLease {}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const TEST_PATH_ENV: &str = "MFB_TEST_PRODUCTION_LOCK_PATH";
    const TEST_READY_ENV: &str = "MFB_TEST_PRODUCTION_LOCK_READY";
    const TEST_RELEASE_ENV: &str = "MFB_TEST_PRODUCTION_LOCK_RELEASE";
    const TEST_NATIVE_PATH_ENV: &str = "MFB_TEST_NATIVE_WRITER_LOCK_PATH";
    const TEST_NATIVE_READY_ENV: &str = "MFB_TEST_NATIVE_WRITER_LOCK_READY";
    const TEST_NATIVE_RELEASE_ENV: &str = "MFB_TEST_NATIVE_WRITER_LOCK_RELEASE";

    fn child_helper() -> Option<ProductionLease> {
        let path = std::env::var_os(TEST_PATH_ENV)?;
        let lease = match std::env::var_os(LEASE_FD_ENV) {
            Some(fd) => acquire_inherited(Path::new(&path), &fd).ok()?,
            None => acquire_at(Path::new(&path)).ok()?,
        };
        if let Some(ready) = std::env::var_os(TEST_READY_ENV) {
            fs::write(ready, b"ready").unwrap();
        }
        if let Some(release) = std::env::var_os(TEST_RELEASE_ENV) {
            while !Path::new(&release).exists() {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Some(lease)
    }

    #[test]
    fn production_lease_subprocess_helper() {
        if std::env::var_os(TEST_PATH_ENV).is_some() {
            let _lease = child_helper().expect("test child lease acquisition");
        }
    }

    #[test]
    fn native_photo_helper_lock_subprocess() {
        let Some(path) = std::env::var_os(TEST_NATIVE_PATH_ENV) else {
            return;
        };
        use std::os::fd::AsRawFd;

        let file = open_lock_file(Path::new(&path)).expect("open native writer lock in helper");
        assert!(try_native_writer_lock(file.as_raw_fd()).expect("lock native writer file"));
        if let Some(ready) = std::env::var_os(TEST_NATIVE_READY_ENV) {
            fs::write(ready, b"ready").expect("publish native helper readiness");
        }
        if let Some(release) = std::env::var_os(TEST_NATIVE_RELEASE_ENV) {
            while !Path::new(&release).exists() {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn spawn_lease_child(
        path: &Path,
        ready: &Path,
        release: &Path,
        lease: &ProductionLease,
    ) -> crate::process_runner::ManagedProcess {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg("infra::production_lock::tests::production_lease_subprocess_helper")
            .arg("--nocapture")
            .env(TEST_PATH_ENV, path)
            .env(TEST_READY_ENV, ready)
            .env(TEST_RELEASE_ENV, release)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        register_active(&lease.inner).unwrap();
        crate::process_runner::ManagedProcess::spawn_captured(&mut command).unwrap()
    }

    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() {
            assert!(Instant::now() < deadline, "child did not publish readiness");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn inherited_subprocess_keeps_lease_after_parent_drop_and_releases_on_exit() {
        let temp = tempfile::tempdir().unwrap();
        let lock_path = temp.path().join("production.lock");
        let ready = temp.path().join("ready");
        let release = temp.path().join("release");
        let lease = acquire_at(&lock_path).unwrap();
        let child = spawn_lease_child(&lock_path, &ready, &release, &lease);
        wait_for_file(&ready);
        drop(lease);

        assert!(acquire_at(&lock_path).is_err(), "child must retain flock");
        fs::write(&release, b"exit").unwrap();
        assert!(child.wait().unwrap().status.success());
        assert!(acquire_at(&lock_path).is_ok(), "child exit releases flock");
    }

    #[test]
    fn native_helper_writer_lock_blocks_new_global_lease_until_helper_exit() {
        let temp = tempfile::tempdir().unwrap();
        let global_path = temp.path().join("production.lock");
        let native_path = temp.path().join("native-photos.lock");
        let ready = temp.path().join("helper-ready");
        let release = temp.path().join("helper-release");

        // Model a production parent which held the global lease while the
        // LaunchServices helper starts, then exits while it owns an fcntl lock.
        let parent_lease = acquire_new(&global_path, &native_path).unwrap();
        let mut helper = Command::new(std::env::current_exe().unwrap());
        helper
            .arg("--exact")
            .arg("infra::production_lock::tests::native_photo_helper_lock_subprocess")
            .arg("--nocapture")
            .env(TEST_NATIVE_PATH_ENV, &native_path)
            .env(TEST_NATIVE_READY_ENV, &ready)
            .env(TEST_NATIVE_RELEASE_ENV, &release)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let helper = crate::process_runner::ManagedProcess::spawn_captured(&mut helper).unwrap();
        wait_for_file(&ready);
        drop(parent_lease);

        let busy = match acquire_new(&global_path, &native_path) {
            Ok(lease) => {
                drop(lease);
                panic!("native helper writer lock must block a new lease");
            }
            Err(error) => error,
        };
        assert!(is_production_task_busy(&busy));

        fs::write(&release, b"exit").unwrap();
        assert!(helper.wait().unwrap().status.success());
        assert!(acquire_new(&global_path, &native_path).is_ok());
    }

    #[test]
    fn malformed_inherited_lease_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("production.lock");
        let _file = open_lock_file(&path).unwrap();
        let result = acquire_inherited(&path, std::ffi::OsStr::new("999999"));
        assert!(result.is_err());
    }

    #[test]
    fn unlocked_same_inode_descriptor_is_not_accepted_as_inherited_lease() {
        use std::os::fd::AsRawFd;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("production.lock");
        let file = open_lock_file(&path).unwrap();
        let fd = file.as_raw_fd().to_string();
        let result = acquire_inherited(&path, std::ffi::OsStr::new(&fd));
        assert!(result.is_err());
        drop(file);
    }
}
