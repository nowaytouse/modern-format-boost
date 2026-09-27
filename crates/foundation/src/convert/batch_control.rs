//! GUI-only cooperative control at durable batch boundaries.
//!
//! The GUI atomically replaces `MFB_BATCH_CONTROL_FILE` with `state\ntoken`.
//! A matching `<file>.ack` is written only after every guarded transaction has
//! completed. Ordinary CLI runs have no control file and remain unchanged.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const CONTROL_ENV: &str = "MFB_BATCH_CONTROL_FILE";
const POLL_INTERVAL: Duration = Duration::from_millis(100);
static ACTIVE: Mutex<usize> = Mutex::new(0);
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Paused,
    Cancelled,
}

#[derive(Debug)]
struct Request {
    state: State,
    token: String,
}

fn control_path() -> io::Result<Option<PathBuf>> {
    let Some(path) = std::env::var_os(CONTROL_ENV) else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "batch control path must be an absolute file path",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "batch control path has no parent",
        )
    })?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "batch control parent must be a private directory, not a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "batch control parent must have private permissions",
            ));
        }
    }
    Ok(Some(path))
}

fn read_request(path: &Path) -> io::Result<Request> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "batch control must be a small regular file, not a symlink",
        ));
    }
    let mut text = String::new();
    File::open(path)?.take(257).read_to_string(&mut text)?;
    let (state, token) = text.split_once('\n').ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed batch control request",
        )
    })?;
    if text.len() > 256
        || token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid batch control token",
        ));
    }
    let state = match state {
        "running" => State::Running,
        "paused" => State::Paused,
        "cancelled" => State::Cancelled,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid batch control state",
            ));
        }
    };
    Ok(Request {
        state,
        token: token.to_owned(),
    })
}

fn write_pause_ack(path: &Path, token: &str) -> io::Result<()> {
    let mut ack_name = path.as_os_str().to_os_string();
    ack_name.push(".ack");
    let ack = PathBuf::from(ack_name);
    let expected = format!("paused\n{token}");
    match fs::symlink_metadata(&ack) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 256 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid batch pause acknowledgment file",
                ));
            }
            if fs::read_to_string(&ack)? == expected {
                return Ok(());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut temp_name = ack.as_os_str().to_os_string();
    temp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = PathBuf::from(temp_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(expected.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, &ack)
    })();
    if let Err(error) = &result {
        match fs::remove_file(&temp) {
            Ok(()) => {}
            Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => {}
            Err(cleanup) => {
                return Err(io::Error::other(format!(
                    "pause acknowledgment failed: {error}; temporary file cleanup failed: {cleanup}"
                )));
            }
        }
    }
    result
}

fn interrupted() -> io::Error {
    io::Error::new(
        io::ErrorKind::Interrupted,
        "batch cancelled at a safe boundary",
    )
}

fn enter(transaction: bool) -> io::Result<()> {
    let Some(path) = control_path()? else {
        return Ok(());
    };
    loop {
        let mut active = ACTIVE
            .lock()
            .map_err(|_| io::Error::other("batch control lock poisoned"))?;
        let request = read_request(&path)?;
        match request.state {
            State::Running => {
                if transaction {
                    *active += 1;
                }
                return Ok(());
            }
            State::Cancelled => return Err(interrupted()),
            State::Paused => {
                if *active == 0 {
                    write_pause_ack(&path, &request.token)?;
                }
            }
        }
        drop(active);
        thread::sleep(POLL_INTERVAL);
    }
}

/// Wait while the GUI requests pause; reject cancellation at a safe boundary.
pub fn checkpoint() -> io::Result<()> {
    enter(false)
}

/// Start a file-level transaction. Keep the guard through durable delivery and
/// checkpoint writes; a pause acknowledgment waits for all guards to drop.
pub fn begin_transaction() -> io::Result<TransactionGuard> {
    let controlled = control_path()?.is_some();
    enter(controlled)?;
    Ok(TransactionGuard { controlled })
}

pub struct TransactionGuard {
    controlled: bool,
}

impl Drop for TransactionGuard {
    fn drop(&mut self) {
        if self.controlled {
            let mut active = crate::media_conversion_gate::mutex_guard_or_recover(
                "batch_control_active",
                ACTIVE.lock(),
            );
            *active -= 1;
        }
    }
}

/// Distinguish a GUI cancellation from an ordinary per-file failure.
pub fn is_cancelled() -> io::Result<bool> {
    control_path()?.map_or(Ok(false), |path| {
        read_request(&path).map(|request| request.state == State::Cancelled)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    fn replace_request(path: &Path, state: &str, token: &str) {
        let temp = path.with_extension("next");
        fs::write(&temp, format!("{state}\n{token}")).unwrap();
        fs::rename(temp, path).unwrap();
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn subprocess_worker() {
        let Some(dir) = std::env::var_os("MFB_BATCH_CONTROL_TEST_CHILD") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let first = begin_transaction().unwrap();
        fs::write(dir.join("first-started"), "ready").unwrap();
        assert!(wait_until(|| dir.join("queue").exists()));
        let second_dir = dir.clone();
        let second = thread::spawn(move || match begin_transaction() {
            Ok(_second) => fs::write(second_dir.join("second-started"), "ready").unwrap(),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                fs::write(second_dir.join("cancelled"), "ready").unwrap();
            }
            Err(error) => panic!("unexpected batch control error: {error}"),
        });
        assert!(wait_until(|| dir.join("continue").exists()));
        drop(first);
        second.join().unwrap();
    }

    #[test]
    fn subprocess_pause_resume_and_cancel_only_at_transaction_boundary() {
        for cancel in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
            }
            let control = dir.path().join("control");
            replace_request(&control, "running", "token-1");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("convert::batch_control::tests::subprocess_worker")
                .arg("--nocapture")
                .env(CONTROL_ENV, &control)
                .env("MFB_BATCH_CONTROL_TEST_CHILD", dir.path())
                .spawn()
                .unwrap();
            if !wait_until(|| dir.path().join("first-started").exists()) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("subprocess did not enter first transaction");
            }
            replace_request(&control, "paused", "token-2");
            fs::write(dir.path().join("queue"), "go").unwrap();
            thread::sleep(Duration::from_millis(150));
            if dir.path().join("control.ack").exists() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("pause was acknowledged before the active transaction finished");
            }
            fs::write(dir.path().join("continue"), "go").unwrap();
            if !wait_until(
                || match fs::read_to_string(dir.path().join("control.ack")) {
                    Ok(text) => text == "paused\ntoken-2",
                    Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                    Err(error) => panic!("pause acknowledgment unreadable: {error}"),
                },
            ) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("subprocess did not acknowledge quiescent pause");
            }
            assert!(!dir.path().join("second-started").exists());
            replace_request(
                &control,
                if cancel { "cancelled" } else { "running" },
                "token-3",
            );
            if !wait_until(|| {
                dir.path()
                    .join(if cancel {
                        "cancelled"
                    } else {
                        "second-started"
                    })
                    .exists()
            }) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("subprocess did not observe resume/cancellation");
            }
            assert!(child.wait().unwrap().success());
        }
    }

    #[test]
    fn pause_ack_does_not_remove_a_preexisting_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("control");
        let temp = dir.path().join(format!(
            "control.ack.{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.load(Ordering::Relaxed)
        ));
        fs::write(&temp, "existing file").unwrap();
        assert_eq!(
            write_pause_ack(&control, "token").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&temp).unwrap(), "existing file");
    }

    #[test]
    fn malformed_missing_or_symlink_control_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("control");
        assert_eq!(
            read_request(&control).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::write(&control, "unexpected\ntoken").unwrap();
        assert_eq!(
            read_request(&control).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&control, &link).unwrap();
            assert_eq!(
                read_request(&link).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}
