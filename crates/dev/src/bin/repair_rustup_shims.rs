//! Repair `~/.cargo/bin` rustup proxies on Homebrew macOS installs.
//!
//! Port of `crates/dev/scripts/ci/repair_rustup_shims.py` (Python retained as
//! compat reference).

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

#[derive(Parser, Debug)]
#[command(
    name = "repair_rustup_shims",
    about = "Repair Homebrew rustup shims under CARGO_HOME/bin"
)]
struct Args {
    /// Print actions without modifying files
    #[arg(long)]
    dry_run: bool,
}

#[allow(dead_code)]
fn cargo_bin_dir() -> PathBuf {
    std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".cargo"))
        .join("bin")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn resolve_rustup_real() -> Option<PathBuf> {
    match Command::new("brew")
        .args(["--prefix", "rustup"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(prefix) if prefix.status.success() => {
            let text = String::from_utf8_lossy(&prefix.stdout).trim().to_string();
            if !text.is_empty() {
                let candidate = PathBuf::from(text).join("libexec/bin/rustup");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        Ok(prefix) => eprintln!(
            "[SHIM-REPAIR] brew --prefix rustup failed with status {:?}",
            prefix.status.code()
        ),
        Err(err) => eprintln!("[SHIM-REPAIR] brew --prefix rustup failed: {err}"),
    }

    for cellar_root in ["/opt/homebrew/Cellar/rustup", "/usr/local/Cellar/rustup"] {
        if let Some(found) = find_cellar_rustup(Path::new(cellar_root)) {
            return Some(found);
        }
    }
    None
}

fn find_cellar_rustup(root: &Path) -> Option<PathBuf> {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(err) => {
            eprintln!(
                "[SHIM-REPAIR] cellar read failed ({}): {err}",
                root.display()
            );
            return None;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                eprintln!(
                    "[SHIM-REPAIR] cellar entry failed under {}: {err}",
                    root.display()
                );
                continue;
            }
        };
        let candidate = entry.path().join("libexec/bin/rustup");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn backup_wrappers(cargo_bin: &Path, dry_run: bool) -> Result<()> {
    let stamp = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs(),
        Err(err) => {
            eprintln!("[SHIM-REPAIR] epoch duration failed: {err}");
            0
        }
    };
    let backup = cargo_bin.join(format!(".shim-repair-backup-{stamp}"));

    for name in ["cargo", "rustc", "rustdoc"] {
        let target = cargo_bin.join(name);
        if target.is_file() && !target.symlink_metadata()?.file_type().is_symlink() {
            eprintln!("  backup custom wrapper: {name} → {}/", backup.display());
            if !dry_run {
                fs::create_dir_all(&backup)?;
                fs::copy(&target, backup.join(name))?;
                fs::remove_file(&target)?;
            }
        }
    }
    Ok(())
}

fn link_proxy(cargo_bin: &Path, name: &str, dry_run: bool) -> Result<()> {
    let target = cargo_bin.join(name);
    if dry_run {
        eprintln!("  would: ln -sf rustup {}", target.display());
        return Ok(());
    }
    if target.exists() || target.symlink_metadata().is_ok() {
        let _ = fs::remove_file(&target);
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("rustup", &target)?;
    }
    #[cfg(not(unix))]
    {
        bail!("symlink repair requires a Unix platform");
    }
    Ok(())
}

fn verify_shims(cargo_bin: &Path, cargo_home: &Path) -> Result<()> {
    let mut path = cargo_bin.as_os_str().to_owned();
    path.push(if cfg!(windows) { ";" } else { ":" });
    if let Some(existing) = std::env::var_os("PATH") {
        path.push(existing);
    }

    let run = |args: &[&str]| -> Result<()> {
        let status = Command::new("cargo")
            .args(args)
            .env("PATH", &path)
            .env("CARGO_HOME", cargo_home)
            .env(
                "RUSTUP_HOME",
                std::env::var("RUSTUP_HOME")
                    .unwrap_or_else(|_| dirs_home().join(".rustup").to_string_lossy().into_owned()),
            )
            .status()
            .with_context(|| format!("run cargo {}", args.join(" ")))?;
        if !status.success() {
            bail!("verification failed: cargo {}", args.join(" "));
        }
        Ok(())
    };

    run(&["--version"])?;
    run(&["clippy", "--version"])?;
    run(&["fmt", "--version"])?;

    let nightly_ok = Command::new("cargo")
        .args(["+nightly", "--version"])
        .env("PATH", &path)
        .env("CARGO_HOME", cargo_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if nightly_ok {
        let _ = Command::new("cargo")
            .args(["+nightly", "clippy", "--version"])
            .env("PATH", &path)
            .env("CARGO_HOME", cargo_home)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cargo_home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".cargo"));
    let cargo_bin = cargo_home.join("bin");
    fs::create_dir_all(&cargo_bin)?;

    let rustup_real = resolve_rustup_real()
        .context("could not find Homebrew rustup libexec binary (install: brew install rustup)")?;

    eprintln!("▶ rustup real binary: {}", rustup_real.display());
    eprintln!("▶ cargo bin dir:      {}", cargo_bin.display());

    backup_wrappers(&cargo_bin, args.dry_run)?;

    let rustup_symlink = cargo_bin.join("rustup");
    if args.dry_run {
        eprintln!(
            "  would: ln -sf {} {}",
            rustup_real.display(),
            rustup_symlink.display()
        );
    } else {
        if rustup_symlink.exists() || rustup_symlink.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&rustup_symlink);
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&rustup_real, &rustup_symlink)?;
    }

    for tool in [
        "cargo",
        "rustc",
        "rustdoc",
        "rust-gdb",
        "rust-lldb",
        "cargo-clippy",
        "cargo-fmt",
        "cargo-miri",
    ] {
        let target = cargo_bin.join(tool);
        if target.exists() || ["cargo", "rustc"].contains(&tool) {
            link_proxy(&cargo_bin, tool, args.dry_run)?;
        }
    }

    eprintln!("▶ verifying (CARGO_HOME={})", cargo_home.display());
    if args.dry_run {
        eprintln!("  dry-run: skip verification");
        return Ok(());
    }

    verify_shims(&cargo_bin, &cargo_home)?;
    eprintln!("✅ rustup shims repaired — use plain `cargo clippy` / `cargo fmt`");
    Ok(())
}
