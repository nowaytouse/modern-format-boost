//! Detach long-running dev binaries to background (mirrors `mfb_entry_guard.detach_to_background`).

use crate::infra::hardening::optional_env;
use anyhow::{Context, Result, bail};
use chrono::Local;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const MFB_BACKGROUND_PID_FILE_ENV: &str = "MFB_BACKGROUND_PID_FILE";
pub const MFB_INVOKER_INTERNAL_REEXEC: &str = "internal_reexec";

/// Remove pid file on normal process exit when spawned via [`detach_current_process`].
pub struct BackgroundPidGuard {
    path: PathBuf,
}

impl BackgroundPidGuard {
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let raw = optional_env(MFB_BACKGROUND_PID_FILE_ENV)?;
        Some(Self {
            path: PathBuf::from(raw),
        })
    }
}

impl Drop for BackgroundPidGuard {
    fn drop(&mut self) {
        if self.path.is_file()
            && let Err(err) = fs::remove_file(&self.path)
        {
            eprintln!(
                "[BACKGROUND] pid file cleanup failed ({}): {err}",
                self.path.display()
            );
        }
    }
}

fn process_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Re-exec current binary without `flag_to_strip`, detached to `log_dir`.
pub fn detach_current_process(
    repo_root: &Path,
    log_dir: &Path,
    pid_file: &Path,
    flag_to_strip: &str,
) -> Result<()> {
    let log_dir = super::log_paths::coerce_log_dir(log_dir);
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("create log dir {}", log_dir.display()))?;

    if pid_file.is_file() {
        let old_pid_str = fs::read_to_string(pid_file)
            .unwrap_or_default()
            .trim()
            .to_string();
        let stale = if old_pid_str.is_empty() {
            true
        } else {
            match old_pid_str.parse::<i32>() {
                Ok(old_pid) if old_pid > 0 && process_alive(old_pid) => {
                    bail!(
                        "已有后台进程 PID={old_pid}；停止: kill {old_pid} && rm -f {}",
                        pid_file.display()
                    );
                }
                Ok(_) => true,
                Err(err) => {
                    eprintln!(
                        "[BACKGROUND] stale pid parse failed ({}): {err}",
                        pid_file.display()
                    );
                    true
                }
            }
        };
        if stale && fs::remove_file(pid_file).is_ok() {
            eprintln!(
                "  [BACKGROUND] Cleared stale pid file: {}",
                pid_file.display()
            );
        }
    }

    let stamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let exe = std::env::current_exe().context("current_exe")?;
    let stem = exe
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("run_training");
    let log_path = log_dir.join(format!("{stem}_{stamp}.log"));

    let argv: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| arg != flag_to_strip)
        .collect();

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open background log {}", log_path.display()))?;
    let log_err = log_file
        .try_clone()
        .context("clone background log handle for stderr")?;

    let mut cmd = Command::new(&exe);
    cmd.args(argv)
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .env("PYTHONUNBUFFERED", "1")
        .env("MFB_LOG_DIR", &log_dir)
        .env("MFB_INVOKER", MFB_INVOKER_INTERNAL_REEXEC)
        .env("MFB_TRAINING_INVOKER", MFB_INVOKER_INTERNAL_REEXEC)
        .env(MFB_BACKGROUND_PID_FILE_ENV, pid_file);

    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn detached {}", exe.display()))?;
    let pid = child.id();

    if let Err(exc) = fs::write(pid_file, format!("{pid}\n")) {
        eprintln!(
            "  [BACKGROUND] pid file write failed ({}): {exc}; terminating detached child PID={pid}",
            pid_file.display()
        );
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "pid file write failed after spawn; child PID={pid} terminated; log={}: {exc}",
            log_path.display()
        );
    }

    eprintln!("  [BACKGROUND] PID={pid} log={}", log_path.display());
    std::process::exit(0);
}
