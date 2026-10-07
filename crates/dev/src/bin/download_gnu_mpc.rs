//! Download GNU MPC with mirror fallbacks for CI.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

// Direct mirrors from https://www.gnu.org/prep/ftp.html avoid a shared GNU outage.
const MIRRORS: &[&str] = &[
    "https://mirrors.ocf.berkeley.edu/gnu/mpc/mpc-1.4.1.tar.xz",
    "https://mirror.csclub.uwaterloo.ca/gnu/mpc/mpc-1.4.1.tar.xz",
    "https://ftp.gnu.org/gnu/mpc/mpc-1.4.1.tar.xz",
    "https://ftpmirror.gnu.org/mpc/mpc-1.4.1.tar.xz",
];
const MPC_SHA256: &str = "91204cd32f164bd3b7c992d4a6a8ce6519511aadab30f78b6982d0bf8d73e931";

const fn help_text() -> &'static str {
    "Download GNU MPC 1.4.1 with mirror fallbacks.\n\nUsage: download_gnu_mpc \
     [OUTPUT]\n\nArguments:\n  OUTPUT    Target tarball path [default: mpc.tar.xz]"
}

fn print_help() {
    println!("{}", help_text());
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("--help"))
        || first.as_deref() == Some(std::ffi::OsStr::new("-h"))
    {
        print_help();
        return Ok(());
    }
    let output = first.map_or_else(|| PathBuf::from("mpc.tar.xz"), PathBuf::from);
    download_archive(&output, MIRRORS, MPC_SHA256)
}

fn download_archive(output: &Path, mirrors: &[&str], expected_sha256: &str) -> Result<()> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for url in mirrors {
        let staged = tempfile::NamedTempFile::new_in(parent).context("create MPC download file")?;
        eprintln!("Attempting to download MPC from {url}...");
        let status = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--connect-timeout",
                "10",
                "--max-time",
                "60",
                "--user-agent",
                "Mozilla/5.0",
                "--output",
            ])
            .arg(staged.path())
            .arg(url)
            .status()
            .with_context(|| format!("launch curl for {url}"))?;
        if !status.success() {
            eprintln!("MPC download failed from {url}: {status}");
            continue;
        }
        if let Err(error) = verify_sha256(staged.path(), expected_sha256) {
            eprintln!("MPC archive rejected from {url}: {error:#}");
            continue;
        }
        staged
            .persist(output)
            .with_context(|| format!("publish MPC archive to {}", output.display()))?;
        eprintln!("MPC tarball fetched and SHA-256 verified from {url}");
        return Ok(());
    }
    bail!("failed to download verified MPC 1.4.1 from all mirrors")
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("shasum");
        command.args(["--algorithm", "256"]);
        command
    } else {
        Command::new("sha256sum")
    };
    let output = command
        .arg(path)
        .output()
        .context("calculate MPC SHA-256")?;
    if !output.status.success() {
        bail!(
            "MPC SHA-256 command failed: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        != Some(expected)
    {
        bail!("MPC source archive SHA-256 mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_documents_default_output() {
        let text = help_text();
        assert!(text.contains("download_gnu_mpc [OUTPUT]"));
        assert!(text.contains("mpc.tar.xz"));
    }

    #[test]
    fn mirrors_fail_over_without_publishing_unverified_files() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let invalid = source.join("invalid");
        let valid = source.join("valid");
        std::fs::write(&invalid, b"invalid archive").unwrap();
        std::fs::write(&valid, b"abc").unwrap();
        let missing_url = format!("file://{}", source.join("missing").display());
        let invalid_url = format!("file://{}", invalid.display());
        let valid_url = format!("file://{}", valid.display());
        let output = destination.join("mpc.tar.xz");
        std::fs::write(&output, b"previous archive").unwrap();

        assert!(download_archive(&output, &[&missing_url, &invalid_url], MPC_SHA256).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"previous archive");
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 1);

        download_archive(
            &output,
            &[&missing_url, &invalid_url, &valid_url],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"abc");
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 1);
    }
}
