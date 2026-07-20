//! Download GNU MPC with mirror fallbacks for CI.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

const MIRRORS: &[&str] = &[
    "https://ftp.gnu.org/gnu/mpc/mpc-1.4.1.tar.xz",
    "https://ftpmirror.gnu.org/mpc/mpc-1.4.1.tar.xz",
];

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
    for url in MIRRORS {
        for attempt in 1_u64..=3 {
            eprintln!("Attempting to download MPC from {url} (attempt {attempt})...");
            let status = Command::new("curl")
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "180",
                    "--user-agent",
                    "Mozilla/5.0",
                    "--output",
                ])
                .arg(&output)
                .arg(url)
                .status()
                .with_context(|| format!("launch curl for {url}"))?;
            if status.success() {
                eprintln!("MPC tarball fetched from {url}");
                return Ok(());
            }
            thread::sleep(Duration::from_secs(attempt * 5));
        }
    }
    bail!("failed to download MPC 1.4.1 from all mirrors")
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
    fn tries_gnu_primary_before_official_mirror_redirector() {
        assert_eq!(
            MIRRORS,
            [
                "https://ftp.gnu.org/gnu/mpc/mpc-1.4.1.tar.xz",
                "https://ftpmirror.gnu.org/mpc/mpc-1.4.1.tar.xz",
            ]
        );
    }
}
