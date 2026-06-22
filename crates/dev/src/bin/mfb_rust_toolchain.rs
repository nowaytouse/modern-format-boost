//! Rust toolchain resolver and inspector.
//!
//! Port of `crates/dev/scripts/mfb_rust_toolchain.py`.
//!
//! Locates the active rustup toolchain, resolves cargo/clippy/rustfmt paths, and
//! prints toolchain info. Prepends the toolchain bin-dir when rustup shims are
//! broken (Homebrew macOS issue documented in `repair_rustup_shims.rs`).
//!
//! Usage:
//!   cargo run --locked -p dev --bin mfb_rust_toolchain
//!   cargo run --locked -p dev --bin mfb_rust_toolchain -- --prefer stable
//!   cargo run --locked -p dev --bin mfb_rust_toolchain -- --which cargo

use anyhow::{Context, Result, bail};
use clap::Parser;
use dev::infra::hardening::{delegated_exit_code, optional_env};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser, Debug)]
#[command(
    name = "mfb_rust_toolchain",
    about = "Resolve and inspect the active rustup toolchain (port of mfb_rust_toolchain.py)"
)]
struct Args {
    /// Prefer this channel when scanning toolchain dirs (nightly|stable)
    #[arg(long, default_value = "nightly")]
    prefer: String,

    /// Print the resolved path for a specific binary (cargo|clippy|rustfmt)
    #[arg(long, value_name = "BINARY")]
    which: Option<String>,

    /// Apply toolchain env to a subprocess (remaining args after --)
    #[arg(last = true)]
    run: Vec<String>,
}

#[derive(Debug)]
struct RustToolchain {
    cargo: PathBuf,
    bin_dir: PathBuf,
    name: Option<String>,
    clippy: Option<PathBuf>,
    rustfmt: Option<PathBuf>,
}

impl RustToolchain {
    fn env(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.bin_dir != *"." {
            let path_var = std::env::var("PATH").unwrap_or_default();
            out.push((
                "PATH".to_string(),
                format!("{}:{}", self.bin_dir.display(), path_var),
            ));
        }
        if let Some(name) = &self.name {
            out.push(("RUSTUP_TOOLCHAIN".to_string(), name.clone()));
        }
        out
    }
}

fn default_host_triple() -> String {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let os = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown-linux-gnu"
    };
    format!("{arch}-{os}")
}

fn toolchain_globs(prefer: &str) -> Vec<String> {
    let host = default_host_triple();
    if prefer == "nightly" {
        vec![
            format!("nightly-{host}"),
            "nightly-*".to_string(),
            format!("stable-{host}"),
            "stable-*".to_string(),
        ]
    } else {
        vec![
            format!("stable-{host}"),
            "stable-*".to_string(),
            format!("nightly-{host}"),
            "nightly-*".to_string(),
        ]
    }
}

fn toolchain_name_from_cargo(cargo_path: &Path) -> Option<String> {
    let parts: Vec<_> = cargo_path.components().collect();
    let idx = parts.iter().position(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("toolchains")
    })?;
    // toolchains/<name>/bin/cargo
    if idx + 3 < parts.len() {
        let name = parts[idx + 1].as_os_str().to_string_lossy().to_string();
        let bin_part = parts[idx + 2].as_os_str().to_string_lossy();
        let bin_name = parts[idx + 3].as_os_str().to_string_lossy();
        if bin_part == "bin" && bin_name == "cargo" {
            return Some(name);
        }
    }
    None
}

fn toolchain_from_cargo(cargo_path: PathBuf) -> RustToolchain {
    let bin_dir = cargo_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = toolchain_name_from_cargo(&cargo_path);
    let clippy = {
        let p = bin_dir.join("cargo-clippy");
        p.is_file().then_some(p)
    };
    let rustfmt = {
        let p = bin_dir.join("cargo-fmt");
        p.is_file().then_some(p)
    };
    RustToolchain {
        cargo: cargo_path,
        bin_dir,
        name,
        clippy,
        rustfmt,
    }
}

fn resolve_rust_toolchain(prefer: &str) -> Result<RustToolchain> {
    let rustup_home = std::env::var("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs_next_home()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".rustup")
        });
    let toolchains_root = rustup_home.join("toolchains");

    // 1. Explicit RUSTUP_TOOLCHAIN env
    if let Some(explicit) = optional_env("RUSTUP_TOOLCHAIN") {
        let candidate = toolchains_root.join(&explicit).join("bin").join("cargo");
        if candidate.is_file() {
            return Ok(toolchain_from_cargo(candidate));
        }
    }

    match Command::new("rustup").args(["which", "cargo"]).output() {
        Ok(output) if output.status.success() => {
            let cargo_txt = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !cargo_txt.is_empty() {
                let cargo_path = PathBuf::from(&cargo_txt);
                if cargo_path.is_file() {
                    return Ok(toolchain_from_cargo(cargo_path));
                }
            }
        }
        Ok(output) => eprintln!(
            "[TOOLCHAIN] rustup which cargo failed with status {:?}",
            output.status.code()
        ),
        Err(err) => eprintln!("[TOOLCHAIN] rustup which cargo failed: {err}"),
    }

    // 3. Scan toolchain dirs by glob priority
    if toolchains_root.is_dir() {
        for pattern in toolchain_globs(prefer) {
            let prefix = pattern.trim_end_matches('*');
            match std::fs::read_dir(&toolchains_root) {
                Ok(entries) => {
                    let mut candidates: Vec<PathBuf> = entries
                        .filter_map(|entry| match entry {
                            Ok(e) => Some(e),
                            Err(err) => {
                                eprintln!("[TOOLCHAIN] toolchain dir entry failed: {err}");
                                None
                            }
                        })
                        .filter(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            if pattern.ends_with('*') {
                                name.starts_with(prefix)
                            } else {
                                name == pattern
                            }
                        })
                        .map(|e| e.path().join("bin").join("cargo"))
                        .filter(|p| p.is_file())
                        .collect();
                    candidates.sort_by(|a, b| b.cmp(a));
                    if let Some(cargo) = candidates.into_iter().next() {
                        return Ok(toolchain_from_cargo(cargo));
                    }
                }
                Err(err) => eprintln!(
                    "[TOOLCHAIN] toolchain scan failed ({}): {err}",
                    toolchains_root.display()
                ),
            }
        }
    }

    // 4. PATH fallback
    if let Some(cargo_path) = which_in_path("cargo") {
        return Ok(toolchain_from_cargo(cargo_path));
    }

    // 5. Bare fallback
    Ok(RustToolchain {
        cargo: PathBuf::from("cargo"),
        bin_dir: PathBuf::from("."),
        name: None,
        clippy: None,
        rustfmt: None,
    })
}

fn dirs_next_home() -> Option<PathBuf> {
    dev::infra::hardening::optional_env("HOME").map(PathBuf::from)
}

fn which_in_path(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path_var| {
        std::env::split_paths(&path_var).find_map(|dir| {
            let candidate = dir.join(bin);
            candidate.is_file().then_some(candidate)
        })
    })
}

fn main() -> Result<()> {
    let args = Args::parse();
    let tc = resolve_rust_toolchain(&args.prefer)?;

    if let Some(binary) = &args.which {
        let path = match binary.as_str() {
            "cargo" => Some(tc.cargo.clone()),
            "clippy" | "cargo-clippy" => tc
                .clippy
                .clone()
                .or_else(|| Some(tc.bin_dir.join("cargo-clippy"))),
            "rustfmt" | "cargo-fmt" => tc
                .rustfmt
                .clone()
                .or_else(|| Some(tc.bin_dir.join("cargo-fmt"))),
            other => bail!("unknown binary {other:?}; expected cargo|clippy|rustfmt"),
        };
        println!("{}", path.unwrap_or(PathBuf::from(binary)).display());
        return Ok(());
    }

    if !args.run.is_empty() {
        let env = tc.env();
        let (program, cmd_args) = args.run.split_first().unwrap();
        let mut cmd = Command::new(program);
        cmd.args(cmd_args);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        let status = cmd
            .status()
            .with_context(|| format!("failed to run {program:?}"))?;
        let code = delegated_exit_code(status, program, "mfb_rust_toolchain --run");
        std::process::exit(code);
    }

    // Default: print toolchain summary
    println!("=== Rust Toolchain ===");
    println!("  cargo    : {}", tc.cargo.display());
    println!("  bin_dir  : {}", tc.bin_dir.display());
    println!("  name     : {}", tc.name.as_deref().unwrap_or("<unknown>"));
    println!(
        "  clippy   : {}",
        tc.clippy
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<not found>".to_string())
    );
    println!(
        "  rustfmt  : {}",
        tc.rustfmt
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<not found>".to_string())
    );
    if let Some(name) = &tc.name {
        println!("  RUSTUP_TOOLCHAIN={name}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_host_triple_non_empty() {
        let triple = default_host_triple();
        assert!(!triple.is_empty());
        assert!(triple.contains('-'));
    }

    #[test]
    fn test_toolchain_globs_nightly_first() {
        let globs = toolchain_globs("nightly");
        assert!(globs[0].starts_with("nightly-"));
    }

    #[test]
    fn test_toolchain_globs_stable_first() {
        let globs = toolchain_globs("stable");
        assert!(globs[0].starts_with("stable-"));
    }

    #[test]
    fn test_toolchain_name_from_cargo_known_path() {
        let path =
            PathBuf::from("/Users/user/.rustup/toolchains/nightly-aarch64-apple-darwin/bin/cargo");
        let name = toolchain_name_from_cargo(&path);
        assert_eq!(name.as_deref(), Some("nightly-aarch64-apple-darwin"));
    }

    #[test]
    fn test_toolchain_name_from_cargo_path_fallback() {
        // Plain PATH cargo has no toolchain name
        let path = PathBuf::from("/usr/local/bin/cargo");
        let name = toolchain_name_from_cargo(&path);
        assert!(name.is_none());
    }

    #[test]
    fn test_which_in_path_sh() {
        // sh is universally present
        assert!(which_in_path("sh").is_some() || which_in_path("bash").is_some());
    }
}
