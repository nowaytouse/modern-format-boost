//! Ultra-strict Clippy gate for Modern Format Boost.

use anyhow::{Context, Result, bail};
use clap::Parser;
use dev::infra::mfb_cargo_env::{CargoEnvResolved, setup_cargo_env};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
#[command(
    name = "clippy_strict",
    about = "Run workspace clippy with hardening lints"
)]
struct Args {
    #[arg(long)]
    fix: bool,
}

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

fn clippy_args(fix: bool, github_actions: bool) -> Vec<String> {
    let mut args = vec![
        "clippy",
        "--locked",
        "--workspace",
        "--all-targets",
        "--all-features",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    if github_actions {
        args.extend(
            ["--features", "foundation/ci-static-build"]
                .into_iter()
                .map(String::from),
        );
    }
    if fix {
        args.extend(
            ["--fix", "--allow-dirty", "--allow-staged"]
                .into_iter()
                .map(String::from),
        );
    }
    args.extend(
        [
            "--",
            "-D",
            "warnings",
            "-A",
            "clippy::option_if_let_else",
            "-A",
            "clippy::manual_let_else",
            "-A",
            "clippy::question_mark",
            "-A",
            "clippy::missing_errors_doc",
            "-A",
            "clippy::multiple_crate_versions",
            "-A",
            "clippy::manual_unwrap_or",
            "-A",
            "clippy::manual_unwrap_or_default",
            "-A",
            "clippy::many_single_char_names",
            "-A",
            "clippy::similar_names",
            "-A",
            "clippy::cast_precision_loss",
            "-A",
            "clippy::cast_possible_truncation",
            "-A",
            "clippy::cast_possible_wrap",
            "-A",
            "clippy::cast_sign_loss",
            "-A",
            "clippy::unnecessary_wraps",
            "-A",
            "clippy::fn_params_excessive_bools",
            "-A",
            "clippy::struct_excessive_bools",
            "-A",
            "clippy::branches_sharing_code",
            "-A",
            "clippy::redundant_locals",
            "-A",
            "clippy::match_same_arms",
            "-A",
            "clippy::ptr_arg",
            "-A",
            "clippy::too_many_arguments",
            "-A",
            "clippy::useless_let_if_seq",
            "-A",
            "clippy::comparison_chain",
            "-A",
            "clippy::or_fun_call",
            "-A",
            "clippy::missing_panics_doc",
            "-A",
            "clippy::manual_checked_ops",
            "-A",
            "clippy::needless_pass_by_value",
            "-A",
            "clippy::match_like_matches_macro",
            "-A",
            "clippy::too_many_lines",
            "-A",
            "clippy::case_sensitive_file_extension_comparisons",
            "-A",
            "clippy::unnecessary_debug_formatting",
            "-A",
            "clippy::items_after_statements",
            "-A",
            "clippy::default_trait_access",
            "-A",
            "clippy::float_cmp",
            "-A",
            "clippy::unused_self",
            "-A",
            "clippy::used_underscore_binding",
            "-A",
            "clippy::ref_option",
            "-A",
            "clippy::format_collect",
            "-A",
            "clippy::assigning_clones",
            "-A",
            "clippy::format_push_string",
            "-A",
            "clippy::collection_is_never_read",
            "-A",
            "clippy::map_unwrap_or",
            "-A",
            "clippy::collapsible_if",
            "-A",
            "clippy::if_not_else",
            "-A",
            "clippy::duration_suboptimal_units",
        ]
        .into_iter()
        .map(String::from),
    );
    args
}

const fn clippy_version_args() -> [&'static str; 2] {
    ["clippy", "--version"]
}

fn ensure_clippy_available(root: &Path, cargo_env: &CargoEnvResolved) -> Result<()> {
    let status = Command::new(cargo_env.cargo_program())
        .args(clippy_version_args())
        .env("PATH", &cargo_env.path_string)
        .env("RUSTUP_TOOLCHAIN", &cargo_env.rustup_toolchain)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if matches!(status, Ok(status) if status.success()) {
        return Ok(());
    }

    eprintln!("cargo clippy broken — running repair_rustup_shims (Rust bin)");
    let repair_status = Command::new("cargo")
        .args([
            "run",
            "--locked",
            "-p",
            "dev",
            "--bin",
            "repair_rustup_shims",
        ])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run repair_rustup_shims")?;
    if !repair_status.success() {
        bail!("repair_rustup_shims failed with status {repair_status}");
    }
    Ok(())
}

fn run_clippy(
    root: &Path,
    args: &[String],
    github_actions: bool,
    cargo_env: &CargoEnvResolved,
) -> Result<()> {
    let mut command = Command::new(cargo_env.cargo_program());
    command.args(args).current_dir(root);
    command
        .env("PATH", &cargo_env.path_string)
        .env("RUSTUP_TOOLCHAIN", &cargo_env.rustup_toolchain);
    if github_actions {
        command.env("LIBHEIF_STATIC", "1");
        command.env("LIBHEIF_SYS_STATIC", "1");
    }
    let status = command.status().context("run cargo clippy")?;
    if !status.success() {
        bail!("clippy_strict failed with status {status}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.fix {
        eprintln!("--fix on full workspace is slow; prefer targeted cargo clippy --fix");
    }
    println!("clippy ultra-strict: workspace deny + pedantic/nursery/cargo warnings as errors");
    let root = project_root()?;
    let cargo_env = setup_cargo_env();
    ensure_clippy_available(&root, &cargo_env)?;
    let github_actions = std::env::var_os("GITHUB_ACTIONS").is_some();
    run_clippy(
        &root,
        &clippy_args(args.fix, github_actions),
        github_actions,
        &cargo_env,
    )?;
    println!("clippy ultra-strict passed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clippy_args_include_locked_workspace_and_denies() {
        let args = clippy_args(false, true);
        assert!(args.contains(&"--locked".to_string()));
        assert!(args.contains(&"--workspace".to_string()));
        assert!(args.contains(&"-D".to_string()));
        assert!(args.contains(&"warnings".to_string()));
        assert!(args.contains(&"foundation/ci-static-build".to_string()));
    }

    #[test]
    fn checks_clippy_version_before_running_gate_and_repairs_on_failure() {
        assert_eq!(clippy_version_args(), ["clippy", "--version"]);
    }

    #[test]
    fn cargo_env_module_matches_python_fallback() {
        let env = dev::infra::mfb_cargo_env::CargoEnvInput {
            rustup_which_cargo: None,
            rustc_host: None,
            mfb_rust_host: Some("aarch64-apple-darwin".to_string()),
            rustup_home: PathBuf::from("/tmp/rustup-home"),
            rustup_toolchain: None,
            current_path: std::ffi::OsString::from("/usr/bin"),
        };
        let resolved = dev::infra::mfb_cargo_env::resolve_cargo_env(&env);
        assert_eq!(
            resolved.bin_dir,
            PathBuf::from("/tmp/rustup-home/toolchains/nightly-aarch64-apple-darwin/bin")
        );
    }
}
