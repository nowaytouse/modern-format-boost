//! Session audit appender.
//!
//! Mirrors the deleted Python helper: use `MFB_SESSION_AUDIT` when present,
//! append `ISO_SECONDS EVENT`, and do nothing when no audit path is configured.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::hardening::ensure_parent_dir;
use dev::infra::log_paths::audit_log_path_from_env;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "session_audit",
    about = "Append an event line to the MFB session audit log"
)]
struct Args {
    /// The audit event text to append
    #[arg(value_name = "EVENT")]
    event: String,

    /// Override the audit log path (default: `MFB_SESSION_AUDIT`; otherwise
    /// no-op)
    #[arg(long, value_name = "PATH")]
    path: Option<PathBuf>,
}

fn audit_log_path(override_path: Option<&PathBuf>) -> Option<PathBuf> {
    override_path.cloned().or_else(audit_log_path_from_env)
}

fn append_session_audit(path: &Path, line: &str) -> Result<()> {
    if !ensure_parent_dir(path) {
        anyhow::bail!("create parent dir for session audit {}", path.display());
    }
    let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open session audit {}", path.display()))?;
    writeln!(file, "{stamp} {line}")
        .with_context(|| format!("write session audit {}", path.display()))?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let Some(path) = audit_log_path(args.path.as_ref()) else {
        return Ok(());
    };
    append_session_audit(&path, &args.event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn append_creates_and_appends_plain_lines() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("audit.log");
        append_session_audit(&log, "test-event-1")?;
        append_session_audit(&log, "test-event-2")?;

        let content = fs::read_to_string(&log)?;
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(" test-event-1"));
        assert!(lines[1].contains(" test-event-2"));
        assert!(!lines[0].starts_with('{'));
        Ok(())
    }

    #[test]
    fn missing_env_is_noop() {
        unsafe { std::env::remove_var("MFB_SESSION_AUDIT") };
        assert!(audit_log_path(None).is_none());
    }

    #[test]
    fn env_path_is_used() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("env-audit.log");
        unsafe { std::env::set_var("MFB_SESSION_AUDIT", &log) };

        assert_eq!(audit_log_path(None), Some(log));

        unsafe { std::env::remove_var("MFB_SESSION_AUDIT") };
        Ok(())
    }
}
