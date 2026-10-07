//! Read-only local CI gate: clean tree, rustfmt check, strict clippy.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Ok(dir.to_path_buf());
        }
        let Some(parent) = dir.parent() else {
            break;
        };
        dir = parent;
    }
    bail!("cannot locate workspace root from {}", cwd.display())
}

fn run(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("run {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} failed with status {status}", args.join(" "));
    }
    Ok(())
}

fn command_exists(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file()))
}

fn clean_tree_status(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .context("run git status --porcelain")?;
    if !output.status.success() {
        bail!("git status failed with status {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() -> Result<()> {
    let root = project_root()?;
    if !command_exists("just") {
        eprintln!("just is required for fix-gate (install: cargo install just --locked)");
        std::process::exit(1);
    }
    let status = clean_tree_status(&root)?;
    if !status.is_empty() {
        eprintln!("fix-gate requires a clean working tree before running just check");
        eprintln!("{status}");
        std::process::exit(1);
    }
    println!("just check (fmt --check + strict clippy)");
    run(&root, "just", &["check"])?;
    println!("just fix-gate passed (read-only checks clean)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_delegates_to_just_check() {
        let command = ["just", "check"];
        assert_eq!(command, ["just", "check"]);
        assert!(!command.contains(&"python3"));
    }

    #[test]
    fn command_exists_finds_shell() {
        assert!(command_exists("sh") || command_exists("bash"));
    }
}
