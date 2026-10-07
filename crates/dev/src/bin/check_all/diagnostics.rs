//! Preserve command evidence independently of the terminal's scrollback.
#![expect(
    clippy::redundant_pub_crate,
    reason = "Diagnostic helpers are private to the checker binary"
)]
use anyhow::{Context, Result};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

pub(super) fn status(label: &str, command: &mut Command) -> Result<ExitStatus> {
    let root = std::env::var_os("MFB_CHECK_LOG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("mfb-check-all-{}", std::process::id()))
        });
    std::fs::create_dir_all(&root)?;
    let slug: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = root.join(format!(
        "{:03}-{slug}.log",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let argv = serde_json::to_string(
        &std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>(),
    )?;
    println!("[CHECK] {label}\ncommand={argv}\nlog={}", path.display());
    let mut log = std::fs::File::create(&path)?;
    writeln!(log, "command={argv}\ncwd={:?}", command.get_current_dir())?;
    command
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?));
    let started = std::time::Instant::now();
    let outcome = command.status();
    let result = match &outcome {
        Ok(status) => format!("{status}"),
        Err(error) => format!("spawn failed: {error}"),
    };
    writeln!(
        log,
        "\nexit={result}\nelapsed_ms={}",
        started.elapsed().as_millis()
    )?;
    log.sync_all()?;
    let excerpt = tail(&path)?;
    println!(
        "{excerpt}\n[CHECK] {label}: {result}; log={}",
        path.display()
    );
    if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        let mut summary = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(summary)?;
        writeln!(
            summary,
            "### {label}\n\nCommand: `{argv}`\n\nExit: `{result}`\n\nLog: `{}`\n\n<details><summary>Last output</summary>\n\n```text\n{}\n```\n</details>\n",
            path.display(),
            excerpt.replace("```", "'''")
        )?;
    }
    outcome.with_context(|| format!("{label}: {result}; command={argv}; log={}", path.display()))
}

fn tail(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(16_384)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_tail_is_bounded_and_handles_multibyte_boundaries() -> Result<()> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all("诊断".repeat(4000).as_bytes())?;
        let result = tail(file.path())?;
        assert!(result.ends_with("诊断"));
        assert!(result.len() <= 16_390);
        Ok(())
    }
}
