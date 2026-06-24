//! Resolve rustup toolchain `bin/` for broken `~/.cargo/bin` shims.
//!
//! Port of `crates/dev/scripts/ci/mfb_cargo_env.py` (Python retained as compat
//! reference).

use crate::infra::hardening::optional_env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct CargoEnvInput {
    pub rustup_which_cargo: Option<PathBuf>,
    pub rustc_host: Option<String>,
    pub mfb_rust_host: Option<String>,
    pub rustup_home: PathBuf,
    pub rustup_toolchain: Option<String>,
    pub current_path: OsString,
}

#[derive(Debug, Clone)]
pub struct CargoEnvResolved {
    pub bin_dir: PathBuf,
    pub rustup_toolchain: String,
    pub path_string: OsString,
}

impl CargoEnvResolved {
    pub fn cargo_program(&self) -> PathBuf {
        let candidate = self.bin_dir.join("cargo");
        if candidate.is_file() {
            candidate
        } else {
            PathBuf::from("cargo")
        }
    }
}

fn command_stdout(args: &[&str]) -> Option<String> {
    let output = match Command::new(args[0])
        .args(&args[1..])
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(err) => {
            eprintln!("[MFB-CARGO-ENV] command {:?} failed: {err}", args);
            return None;
        }
    };
    if !output.status.success() {
        return None;
    }
    match String::from_utf8(output.stdout) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Err(err) => {
            eprintln!("[MFB-CARGO-ENV] stdout decode failed: {err}");
            None
        }
    }
}

pub fn rustup_which_cargo() -> Option<PathBuf> {
    let cargo = command_stdout(&["rustup", "which", "cargo"])?;
    let path = PathBuf::from(cargo);
    path.is_file().then_some(path)
}

pub fn rustc_host() -> Option<String> {
    let info = command_stdout(&["rustc", "-vV"])?;
    info.lines()
        .find_map(|line| line.strip_prefix("host:").map(str::trim))
        .filter(|host| !host.is_empty())
        .map(ToOwned::to_owned)
}

pub fn platform_host() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    match (arch, os) {
        ("aarch64", "macos") => "aarch64-apple-darwin".to_string(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_string(),
        ("aarch64", _) => "aarch64-unknown-linux-gnu".to_string(),
        ("x86_64", _) => "x86_64-unknown-linux-gnu".to_string(),
        (other, _) => format!("{other}-unknown-linux-gnu"),
    }
}

pub fn current_cargo_env_input() -> CargoEnvInput {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    CargoEnvInput {
        rustup_which_cargo: rustup_which_cargo(),
        rustc_host: rustc_host(),
        mfb_rust_host: optional_env("MFB_RUST_HOST"),
        rustup_home: std::env::var_os("RUSTUP_HOME")
            .map_or_else(|| home.join(".rustup"), PathBuf::from),
        rustup_toolchain: optional_env("RUSTUP_TOOLCHAIN"),
        current_path: std::env::var_os("PATH").unwrap_or_default(),
    }
}

pub fn resolve_cargo_env(input: &CargoEnvInput) -> CargoEnvResolved {
    let bin_dir = input
        .rustup_which_cargo
        .as_ref()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| {
            let host = input
                .mfb_rust_host
                .clone()
                .or_else(|| input.rustc_host.clone())
                .unwrap_or_else(platform_host);
            let toolchain = input
                .rustup_toolchain
                .clone()
                .unwrap_or_else(|| format!("nightly-{host}"));
            input
                .rustup_home
                .join("toolchains")
                .join(toolchain)
                .join("bin")
        });

    let rustup_toolchain = input.rustup_toolchain.clone().unwrap_or_else(|| {
        bin_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .map_or_else(|| "nightly".to_string(), ToOwned::to_owned)
    });

    let mut path_string = OsString::from(bin_dir.as_os_str());
    path_string.push(if cfg!(windows) { ";" } else { ":" });
    path_string.push(&input.current_path);

    CargoEnvResolved {
        bin_dir,
        rustup_toolchain,
        path_string,
    }
}

pub fn setup_cargo_env() -> CargoEnvResolved {
    resolve_cargo_env(&current_cargo_env_input())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_fallback_path_from_host_and_toolchain() {
        let env = CargoEnvInput {
            rustup_which_cargo: None,
            rustc_host: None,
            mfb_rust_host: Some("aarch64-apple-darwin".to_string()),
            rustup_home: PathBuf::from("/tmp/rustup-home"),
            rustup_toolchain: None,
            current_path: OsString::from("/usr/bin"),
        };

        let resolved = resolve_cargo_env(&env);

        assert_eq!(
            resolved.bin_dir,
            PathBuf::from("/tmp/rustup-home/toolchains/nightly-aarch64-apple-darwin/bin")
        );
        assert_eq!(resolved.rustup_toolchain, "nightly-aarch64-apple-darwin");
        let path = resolved.path_string.to_string_lossy();
        assert!(path.starts_with("/tmp/rustup-home/toolchains/nightly-aarch64-apple-darwin/bin:"));
    }

    #[test]
    fn resolves_from_rustup_which_cargo_first() {
        let env = CargoEnvInput {
            rustup_which_cargo: Some(PathBuf::from(
                "/tmp/rustup/toolchains/nightly-test/bin/cargo",
            )),
            rustc_host: Some("x86_64-apple-darwin".to_string()),
            mfb_rust_host: Some("aarch64-apple-darwin".to_string()),
            rustup_home: PathBuf::from("/tmp/ignored"),
            rustup_toolchain: None,
            current_path: OsString::from("/usr/bin"),
        };

        let resolved = resolve_cargo_env(&env);

        assert_eq!(
            resolved.bin_dir,
            PathBuf::from("/tmp/rustup/toolchains/nightly-test/bin")
        );
        assert_eq!(resolved.rustup_toolchain, "nightly-test");
    }

    #[test]
    fn cargo_program_uses_resolved_toolchain_binary_when_present() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let bin_dir = tempdir.path().join("toolchains/nightly-test/bin");
        std::fs::create_dir_all(&bin_dir)?;
        let cargo = bin_dir.join("cargo");
        std::fs::write(&cargo, "#!/bin/sh\n")?;

        let resolved = CargoEnvResolved {
            bin_dir,
            rustup_toolchain: "nightly-test".to_string(),
            path_string: OsString::from("/usr/bin"),
        };

        assert_eq!(resolved.cargo_program(), cargo);
        Ok(())
    }
}
