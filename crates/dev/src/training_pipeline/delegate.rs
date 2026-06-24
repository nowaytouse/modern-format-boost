//! Subprocess delegation: foundation bins, dev bins, and ML Python scripts.

use crate::infra::hardening::{delegated_exit_code, optional_env};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

pub const DEFAULT_CONNSTR: &str = super::DEFAULT_CONNSTR;

pub fn resolve_connstr(explicit: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| optional_env("MFB_PG_CONNSTR"))
        .unwrap_or_else(|| DEFAULT_CONNSTR.to_string())
}

pub fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("current_dir")?;
    if cwd.join("Cargo.toml").exists() {
        return Ok(cwd);
    }
    for ancestor in cwd.ancestors() {
        if ancestor.join("Cargo.toml").exists() {
            return Ok(ancestor.to_path_buf());
        }
    }
    bail!("could not locate repository root (missing Cargo.toml)")
}

pub fn preferred_training_python(root: &Path) -> PathBuf {
    if let Some(explicit) = optional_env("MFB_QUALITY_MODEL_PYTHON") {
        return PathBuf::from(explicit);
    }
    let venv = root.join("crates/.modern_format_boost/.venv/bin/python");
    if venv.is_file() {
        return venv;
    }
    PathBuf::from("python3")
}

pub fn install_python_training_requirements(root: &Path) -> Result<()> {
    let req_file = root.join("requirements-training.txt");
    if !req_file.is_file() {
        return Ok(());
    }
    let python = preferred_training_python(root);
    eprintln!(
        "  [PYTHON] Installing dependencies from {}...",
        req_file.display()
    );
    let status = Command::new(&python)
        .args(["-m", "pip", "install", "-r"])
        .arg(&req_file)
        .current_dir(root)
        .status()
        .context("pip install failed")?;
    if !status.success() {
        bail!("pip install exited with {}", status);
    }
    Ok(())
}

pub fn ensure_python_training_requirements(root: &Path, install_missing: bool) -> Result<()> {
    let python = preferred_training_python(root);
    let check_cmd = Command::new(&python)
        .args(["-c", "import lightgbm, sklearn, psycopg2"])
        .output()
        .context("python module check")?;
    if !check_cmd.status.success() {
        if install_missing {
            install_python_training_requirements(root)?;
        } else {
            bail!(
                "Missing python training requirements. Pass --install-missing-python-deps or run pip install -r requirements-training.txt"
            );
        }
    }
    Ok(())
}

fn release_bin(root: &Path, name: &str) -> PathBuf {
    root.join("target/release").join(name)
}

fn foundation_bin(root: &Path, name: &str, stale: bool) -> Command {
    let bin = release_bin(root, name);
    if !stale && bin.is_file() {
        Command::new(bin)
    } else {
        let mut cmd = Command::new("cargo");
        cmd.args(["run", "--locked", "--release", "-p", "foundation", "--bin", name, "--"]);
        cmd
    }
}

pub fn training_pipeline_command(root: &Path) -> Command {
    let bin = release_bin(root, "training_pipeline");
    if bin.is_file() {
        Command::new(bin)
    } else {
        let mut cmd = Command::new("cargo");
        cmd.args([
            "run",
            "--locked",
            "--release",
            "-p",
            "dev",
            "--bin",
            "training_pipeline",
            "--",
        ]);
        cmd.current_dir(root);
        cmd
    }
}

pub fn run_training_pipeline_subcommand(
    root: &Path,
    connstr: &str,
    subcommand: &str,
    extra_args: &[&str],
) -> Result<i32> {
    let mut cmd = training_pipeline_command(root);
    cmd.arg("--connstr").arg(connstr).arg(subcommand);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let status = cmd
        .status()
        .with_context(|| format!("training_pipeline {subcommand}"))?;
    Ok(delegated_exit_code(status, "training_pipeline", subcommand))
}

pub fn run_foundation_bin(root: &Path, bin: &str, args: &[&str], connstr: &str) -> Result<i32> {
    let mut cmd = foundation_bin(root, bin, false);
    cmd.args(args);
    cmd.env("MFB_PG_CONNSTR", connstr);
    cmd.current_dir(root);
    let status = cmd
        .status()
        .with_context(|| format!("foundation bin {bin}"))?;
    Ok(delegated_exit_code(status, "foundation", bin))
}

pub fn run_dev_bin(root: &Path, bin: &str, args: &[&str]) -> Result<i32> {
    let debug = debug_bin(root, bin);
    let mut cmd = if debug.is_file() {
        Command::new(debug)
    } else {
        let mut c = Command::new("cargo");
        c.args(["run", "--locked", "-p", "dev", "--bin", bin, "--"]);
        c
    };
    cmd.args(args).current_dir(root);
    let status = cmd.status().with_context(|| format!("dev bin {bin}"))?;
    Ok(delegated_exit_code(status, "dev", bin))
}

pub fn run_python_script(
    root: &Path,
    script_rel: &str,
    args: &[&str],
    connstr: Option<&str>,
) -> Result<i32> {
    let python = preferred_training_python(root);
    let script = root.join(script_rel);
    if !script.is_file() {
        bail!("python script not found: {}", script.display());
    }
    let mut cmd = Command::new(python);
    cmd.arg(script).args(args);
    if let Some(cs) = connstr {
        cmd.env("MFB_PG_CONNSTR", cs);
    }
    cmd.current_dir(root);
    let status = cmd
        .status()
        .with_context(|| format!("python {script_rel}"))?;
    Ok(delegated_exit_code(status, "python", script_rel))
}

pub fn run_run_training_batch(root: &Path, connstr: &str) -> Result<i32> {
    eprintln!("Legacy `train` → run_training (Rust) --use-api --fill-runtime-assets");
    let debug = debug_bin(root, "run_training");
    let mut cmd = if debug.is_file() {
        Command::new(debug)
    } else {
        let mut c = Command::new("cargo");
        c.args([
            "run",
            "--locked",
            "-p",
            "dev",
            "--bin",
            "run_training",
            "--",
        ]);
        c
    };
    cmd.args([
        "--use-api",
        "--repair-schema",
        "--fill-runtime-assets",
        "--verify-after",
        "--install-missing-python-deps",
    ])
    .env("MFB_PG_CONNSTR", connstr)
    .env("MFB_INVOKER", "training_pipeline")
    .current_dir(root);
    let status = cmd.status().context("run_training batch")?;
    Ok(delegated_exit_code(status, "run_training", "batch"))
}

#[allow(dead_code)]
pub fn exit_status_code(status: ExitStatus, tool: &str, context: &str) -> i32 {
    delegated_exit_code(status, tool, context)
}
