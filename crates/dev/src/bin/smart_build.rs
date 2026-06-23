//! Modern Format Boost - Smart Build System in Rust.
//! Compiles img and vid release binaries incrementally based on source file modifications.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::logger::setup_logger;
use dev::infra::ui_tokens::pick_symbol;
use foundation::tracing::{debug, error, info, warn};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

// Media / conversion tools aligned with foundation tools.rs.
const BREW_MEDIA_FORMULAE: &[&str] = &[
    "ffmpeg",
    "jpeg-xl",
    "exiftool",
    "imagemagick",
    "webp",
    "libheif",
    "libvmaf",
    "chromaprint",
    "pgvector",
];

const RUST_TOOLCHAIN_FILE: &str = "rust-toolchain.toml";
const APP_BUNDLE_CODESIGN_IDENTITY: &str = "MFB-Dev-Signing";
const APP_BUNDLE_RESOURCE_BINARIES: &[&str] = &[
    "img",
    "vid",
    "verify",
    "cache_cleaner",
    "database_manager",
    "collect_optimized",
    "merge_xmp",
    "icloud_import",
    "drag_and_drop_processor",
];
const VUE_QUALITY_SCRIPTS: &[&str] = &["lint", "format:check", "deps:check", "build"];
const VUE_UPDATE_SCRIPTS: &[&str] = &["deps:update", "deps:check"];

// ANSI Colors
const RED: &str = "\x1b[38;5;196m";
const GREEN: &str = "\x1b[38;5;46m";
const YELLOW: &str = "\x1b[38;5;226m";
const CYAN: &str = "\x1b[38;5;51m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const NC: &str = "\x1b[0m";

#[derive(Debug, Clone, Copy)]
struct Style {
    red: &'static str,
    green: &'static str,
    yellow: &'static str,
    cyan: &'static str,
    bold: &'static str,
    dim: &'static str,
    reset: &'static str,
}

impl Style {
    fn current() -> Self {
        if std::io::stdout().is_terminal() {
            Self {
                red: RED,
                green: GREEN,
                yellow: YELLOW,
                cyan: CYAN,
                bold: BOLD,
                dim: DIM,
                reset: NC,
            }
        } else {
            Self {
                red: "",
                green: "",
                yellow: "",
                cyan: "",
                bold: "",
                dim: "",
                reset: "",
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "smart_build",
    about = "Smart Build System for Modern Format Boost"
)]
struct Args {
    #[arg(
        long = "force",
        short = 'f',
        help = "Force rebuild all selected projects"
    )]
    force: bool,

    #[arg(
        long = "clean",
        short = 'c',
        help = "Clean build artifacts before compiling"
    )]
    clean: bool,

    #[arg(long = "verbose", short = 'v', help = "Show detailed output")]
    verbose: bool,

    #[arg(
        long = "no-clean-old",
        default_value_t = true,
        action = clap::ArgAction::SetFalse,
        help = "Don't clean old binary files"
    )]
    clean_old: bool,

    #[arg(long = "all", short = 'a', help = "Build all projects")]
    all: bool,

    #[arg(long = "img", help = "Build image tools")]
    img: bool,

    #[arg(long = "vid", help = "Build video tools")]
    vid: bool,

    #[arg(long = "hevc", help = "Support for HEVC codecs")]
    hevc: bool,

    #[arg(long = "av1", help = "Support for AV1 codecs")]
    av1: bool,

    #[arg(long = "kondo", help = "Perform deep project cleanup using kondo")]
    kondo: bool,

    #[arg(
        long = "no-verify-timestamps",
        default_value_t = true,
        action = clap::ArgAction::SetFalse,
        help = "Disable timestamp verification after build"
    )]
    verify_timestamps: bool,

    #[arg(
        long = "quiet",
        short = 'q',
        help = "No output when all selected binaries are already up-to-date"
    )]
    quiet: bool,

    #[arg(
        long = "update",
        help = "Run dependency updates (cargo update, topgrade, brew, etc.)"
    )]
    update: bool,

    #[arg(
        long = "gui",
        help = "Build the Tauri Vue GUI and replace the App bundle"
    )]
    gui: bool,
}

fn command_exists(cmd: &str) -> bool {
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            let candidate = path.join(cmd);
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

fn vue_quality_script_names() -> &'static [&'static str] {
    VUE_QUALITY_SCRIPTS
}

fn vue_update_script_names() -> &'static [&'static str] {
    VUE_UPDATE_SCRIPTS
}

fn vue_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("crates")
        .join("dev")
        .join("src")
        .join("vue")
}

fn run_vue_npm_script(
    project_root: &Path,
    script: &str,
    style: &Style,
    required: bool,
) -> Result<bool> {
    let vue_dir = vue_dir(project_root);
    if !vue_dir.join("package.json").is_file() {
        println!(
            "{}   · Vue package.json missing; skipping npm {script}.{}",
            style.dim, style.reset
        );
        return Ok(true);
    }
    if !command_exists("npm") {
        if required {
            anyhow::bail!("npm not found; cannot run Vue npm script {script}");
        }
        return Ok(false);
    }

    let mut command = Command::new("npm");
    command.arg("run").arg(script).current_dir(&vue_dir);
    let ok = run_update_step(&format!("npm run {script}"), &mut command, style, required);
    if required && !ok {
        anyhow::bail!("Vue npm script failed: {script}");
    }
    Ok(ok)
}

fn run_vue_quality_checks(project_root: &Path, style: &Style) -> Result<()> {
    for script in vue_quality_script_names() {
        run_vue_npm_script(project_root, script, style, true)?;
    }
    Ok(())
}

fn run_vue_dependency_update_validation(project_root: &Path, style: &Style) -> Result<()> {
    for script in vue_update_script_names() {
        run_vue_npm_script(project_root, script, style, true)?;
    }
    Ok(())
}

fn get_project_root() -> Result<PathBuf> {
    let exe_path = std::env::current_exe()?;
    let mut dir = exe_path.parent();
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() && d.join("crates").is_dir() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    let cwd = std::env::current_dir()?;
    Ok(cwd)
}

fn get_binary_path(project_root: &Path, binary_name: &str) -> PathBuf {
    project_root.join("target/release").join(binary_name)
}

fn get_mtime(path: &Path) -> f64 {
    match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs_f64(),
            Err(_err) => 0.0,
        },
        Err(_err) => 0.0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RustToolchain {
    cargo: PathBuf,
    bin_dir: PathBuf,
    name: Option<String>,
}

#[derive(Debug, Clone)]
struct RustToolchainInput {
    rustup_home: PathBuf,
    rustup_toolchain: Option<String>,
    rustup_which_cargo: Option<PathBuf>,
    path_cargo: Option<PathBuf>,
    prefer: &'static str,
}

fn toolchain_name_from_cargo_path(cargo_path: &Path) -> Option<String> {
    let parts = cargo_path.components().collect::<Vec<_>>();
    let idx = parts
        .iter()
        .position(|part| part.as_os_str() == "toolchains")?;
    if idx + 3 >= parts.len() {
        return None;
    }
    if parts[idx + 2].as_os_str() != "bin" || parts[idx + 3].as_os_str() != "cargo" {
        return None;
    }
    parts[idx + 1].as_os_str().to_str().map(ToOwned::to_owned)
}

fn default_host_triple() -> String {
    let arch = std::env::consts::ARCH;
    let os = if std::env::consts::OS == "macos" {
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

fn toolchain_from_cargo(cargo: PathBuf) -> RustToolchain {
    let bin_dir = cargo.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    RustToolchain {
        name: toolchain_name_from_cargo_path(&cargo),
        cargo,
        bin_dir,
    }
}

fn resolve_rust_toolchain_from(input: &RustToolchainInput) -> RustToolchain {
    if let Some(explicit) = input
        .rustup_toolchain
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let candidate = input
            .rustup_home
            .join("toolchains")
            .join(explicit)
            .join("bin")
            .join("cargo");
        if candidate.is_file() {
            return toolchain_from_cargo(candidate);
        }
    }

    if let Some(cargo) = input.rustup_which_cargo.as_ref().filter(|p| p.is_file()) {
        return toolchain_from_cargo(cargo.clone());
    }

    let toolchains_root = input.rustup_home.join("toolchains");
    if toolchains_root.is_dir() {
        for pattern in toolchain_globs(input.prefer) {
            let prefix = pattern.trim_end_matches('*');
            let mut dirs = match fs::read_dir(&toolchains_root) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name == pattern || name.starts_with(prefix))
                    })
                    .collect::<Vec<_>>(),
                Err(_) => Vec::new(),
            };
            dirs.sort();
            dirs.reverse();
            for dir in dirs {
                let candidate = dir.join("bin").join("cargo");
                if candidate.is_file() {
                    return toolchain_from_cargo(candidate);
                }
            }
        }
    }

    if let Some(cargo) = input.path_cargo.as_ref() {
        return toolchain_from_cargo(cargo.clone());
    }

    RustToolchain {
        cargo: PathBuf::from("cargo"),
        bin_dir: PathBuf::from("."),
        name: None,
    }
}

fn command_stdout(args: &[&str]) -> Option<String> {
    let output = match Command::new(args[0]).args(&args[1..]).output() {
        Ok(o) => o,
        Err(err) => {
            eprintln!("[SMART-BUILD] command {:?} failed: {err}", args);
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
            eprintln!("[SMART-BUILD] stdout decode failed: {err}");
            None
        }
    }
}

fn current_toolchain_input() -> RustToolchainInput {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    RustToolchainInput {
        rustup_home: std::env::var_os("RUSTUP_HOME")
            .map_or_else(|| home.join(".rustup"), PathBuf::from),
        rustup_toolchain: dev::infra::hardening::optional_env("RUSTUP_TOOLCHAIN"),
        rustup_which_cargo: command_stdout(&["rustup", "which", "cargo"])
            .map(PathBuf::from)
            .filter(|p| p.is_file()),
        path_cargo: find_on_path("cargo"),
        prefer: "nightly",
    }
}

fn find_on_path(cmd: &str) -> Option<PathBuf> {
    let path_var: OsString = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(cmd))
        .find(|path| path.is_file())
}

fn toolchain_env(toolchain: &RustToolchain) -> Vec<(&'static str, OsString)> {
    let mut env = Vec::new();
    if toolchain.bin_dir != Path::new(".") {
        let mut path = OsString::from(toolchain.bin_dir.as_os_str());
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(std::env::var_os("PATH").unwrap_or_default());
        env.push(("PATH", path));
    }
    if let Some(name) = &toolchain.name {
        env.push(("RUSTUP_TOOLCHAIN", OsString::from(name)));
    }
    env
}

fn get_newest_source_mtime(project_root: &Path, project_dir: &str) -> f64 {
    let mut newest = 0.0;
    let src_extensions: HashSet<&str> = ["rs", "sql", "c", "h", "cpp", "cc", "proto", "py", "sh"]
        .iter()
        .copied()
        .collect();

    let mut check_file = |p: &Path| {
        let m = get_mtime(p);
        if m > newest {
            newest = m;
        }
    };

    // 1. Scan the project's own directory
    let proj_path = project_root.join(project_dir);
    if proj_path.is_dir() {
        for entry in walkdir::WalkDir::new(&proj_path) {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if path.is_file()
                        && let Some(ext) = path.extension().and_then(|e| e.to_str())
                        && src_extensions.contains(ext)
                    {
                        check_file(path);
                    }
                }
                Err(_err) => {}
            }
        }
        check_file(&proj_path.join("Cargo.toml"));
    }

    // 2. Scan foundation (global dependency)
    let shared_path = project_root.join("crates/foundation");
    if shared_path.is_dir() && project_dir != "crates/foundation" {
        for entry in walkdir::WalkDir::new(&shared_path) {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if path.is_file()
                        && let Some(ext) = path.extension().and_then(|e| e.to_str())
                        && src_extensions.contains(ext)
                    {
                        check_file(path);
                    }
                }
                Err(_err) => {}
            }
        }
        check_file(&shared_path.join("Cargo.toml"));
    }

    // 3. Scan workspace configuration
    check_file(&project_root.join("Cargo.toml"));
    check_file(&project_root.join("Cargo.lock"));
    check_file(&project_root.join("rust-toolchain.toml"));
    check_file(&project_root.join("crates/dev/src/bin/smart_build.rs"));

    newest
}

fn clean_old_binaries(project_root: &Path, targets: &[&str], style: Style) -> Result<i32> {
    println!("{}Cleaning old binaries...{}", style.yellow, style.reset);
    let mut cleaned = 0;

    for entry in walkdir::WalkDir::new(project_root) {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.is_file()
                    && let Some(name) = path.file_name().and_then(|f| f.to_str())
                    && targets.contains(&name)
                    && !path.components().any(|c| c.as_os_str() == "target")
                {
                    println!(
                        "   {}Removing: {}{}",
                        style.red,
                        path.display(),
                        style.reset
                    );
                    if fs::remove_file(path).is_ok() {
                        cleaned += 1;
                    }
                }
            }
            Err(_err) => {}
        }
    }

    if cleaned == 0 {
        println!("   [OK] {}No old binaries found{}", style.dim, style.reset);
    } else {
        println!(
            "   [OK] Cleaned {} old binary file(s){}",
            cleaned, style.reset
        );
    }
    println!();
    Ok(cleaned)
}

fn clean_with_kondo(project_root: &Path, style: Style) -> Result<()> {
    if !command_exists("kondo") {
        println!(
            "{}kondo not found; skipping deep cleanup.{}",
            style.dim, style.reset
        );
        return Ok(());
    }
    println!(
        "{}Project Deep Cleanup (kondo)...{}",
        style.yellow, style.reset
    );
    let library_path = match std::env::var("HOME") {
        Ok(home) if !home.trim().is_empty() => format!("{home}/Library"),
        Ok(_) | Err(_) => String::from("/Library"),
    };
    Command::new("kondo")
        .arg("-n")
        .arg("-I")
        .arg("/Volumes")
        .arg("-I")
        .arg(&library_path)
        .arg(".")
        .current_dir(project_root)
        .status()?;
    println!();
    Ok(())
}

fn decide_build_action(
    project_root: &Path,
    project_dir: &str,
    binary_name: &str,
    force: bool,
) -> (&'static str, &'static str) {
    let binary_path = get_binary_path(project_root, binary_name);
    if force {
        return ("rebuild", "force");
    }
    if !binary_path.is_file() {
        return ("rebuild", "binary-missing");
    }

    let source_mtime = get_newest_source_mtime(project_root, project_dir);
    let binary_mtime = get_mtime(&binary_path);

    if source_mtime > binary_mtime {
        return ("rebuild", "source-newer");
    }

    ("skip", "")
}

fn build_project(
    project_root: &Path,
    project_dir: &str,
    binary_name: &str,
    retry_count: i32,
    args: &Args,
    style: Style,
) -> Result<bool> {
    let compile_start_time = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(_err) => 0.0,
    };

    let manifest = project_root.join(project_dir).join("Cargo.toml");
    let toolchain = resolve_rust_toolchain_from(&current_toolchain_input());
    info!("Executing cargo build for {project_dir}");
    debug!(
        "Cargo command: {} build --release --manifest-path {}",
        toolchain.cargo.display(),
        manifest.display()
    );
    if let Some(name) = &toolchain.name {
        debug!("Using rustup toolchain: {name}");
    }
    let mut command = if matches!(
        std::process::Command::new("which")
            .arg("rtk")
            .output()
            .map(|o| o.status.success()),
        Ok(true)
    ) {
        let mut c = Command::new("rtk");
        c.arg(&toolchain.cargo);
        c
    } else {
        Command::new(&toolchain.cargo)
    };
    for (key, value) in toolchain_env(&toolchain) {
        command.env(key, value);
    }
    let status = command
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest)
        .current_dir(project_root)
        .status()?;

    if !status.success() {
        error!("Cargo compilation failed for {project_dir} with status {status}");
        println!(
            "{}FAILURE: compilation failed for {}{}",
            style.red, project_dir, style.reset
        );
        return Ok(false);
    }

    if args.verify_timestamps {
        let binary_path = get_binary_path(project_root, binary_name);
        std::thread::sleep(std::time::Duration::from_secs(1));

        if !binary_path.is_file() {
            println!(
                "{}ERROR: TIMESTAMP VERIFICATION FAILED: Binary not found{}",
                style.red, style.reset
            );
            return Ok(false);
        }

        let binary_mtime = get_mtime(&binary_path);
        if binary_mtime < (compile_start_time - 2.0) {
            println!(
                "{}FAILURE: TIMESTAMP VERIFICATION FAILED{}",
                style.red, style.reset
            );
            if retry_count < 2 {
                warn!(
                    "Timestamp verification failed for {}. Retrying ({}/2).",
                    binary_path.display(),
                    retry_count + 1
                );
                println!(
                    "{}Retry {}/2: Rebuilding with clean...{}",
                    style.yellow,
                    retry_count + 1,
                    style.reset
                );
                warn!("[AUDIT] DESTRUCTIVE ACTION: Deleting target/release/deps");
                let _ = fs::remove_dir_all(project_root.join("target/release/deps"));
                warn!("[AUDIT] DESTRUCTIVE ACTION: Deleting target/release/.fingerprint");
                let _ = fs::remove_dir_all(project_root.join("target/release/.fingerprint"));
                return build_project(
                    project_root,
                    project_dir,
                    binary_name,
                    retry_count + 1,
                    args,
                    style,
                );
            }
            println!(
                "{}FAILURE: Timestamp verification failed after retries{}",
                style.red, style.reset
            );
            return Ok(false);
        }
    }

    Ok(true)
}

fn run_update_step(label: &str, cmd: &mut Command, style: &Style, required: bool) -> bool {
    println!("{}   · {}…{}", style.dim, label, style.reset);
    let result = cmd.status();
    match result {
        Err(e) => {
            if required {
                eprintln!("{}   ! {} failed: {}{}", style.red, label, e, style.reset);
            }
            false
        }
        Ok(status) => {
            if status.success() {
                true
            } else {
                eprintln!(
                    "{}   ! {} exited with status {}{}",
                    style.yellow, label, status, style.reset
                );
                false
            }
        }
    }
}

fn pinned_rust_channel(project_root: &Path) -> Option<String> {
    let toml_path = project_root.join(RUST_TOOLCHAIN_FILE);
    let text = match std::fs::read_to_string(&toml_path) {
        Ok(text) => text,
        Err(err) => {
            debug!("Failed to read {}: {err}", toml_path.display());
            return None;
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("channel") {
            let rest = rest.trim_start_matches([' ', '=']).trim();
            let channel = rest.trim_matches('"').to_string();
            if !channel.is_empty() {
                return Some(channel);
            }
        }
    }
    None
}

fn rust_toolchain_components(project_root: &Path) -> Vec<String> {
    let toml_path = project_root.join(RUST_TOOLCHAIN_FILE);
    let text = match std::fs::read_to_string(toml_path) {
        Ok(t) => t,
        Err(_) => {
            return vec![
                "rustfmt".to_string(),
                "clippy".to_string(),
                "llvm-tools".to_string(),
            ];
        }
    };
    let mut in_components = false;
    let mut components = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("components") && line.contains('=') {
            in_components = true;
        }
        if in_components {
            for part in line.split('"') {
                let part = part.trim();
                if !part.is_empty()
                    && !part.starts_with('[')
                    && !part.starts_with(']')
                    && !part.contains('=')
                    && part != ","
                    && part != "]"
                {
                    components.push(part.to_string());
                }
            }
            if line.contains(']') {
                break;
            }
        }
    }
    if components.is_empty() {
        vec![
            "rustfmt".to_string(),
            "clippy".to_string(),
            "llvm-tools".to_string(),
        ]
    } else {
        components
    }
}

fn bootstrap_macos_path() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let extra = [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    ];
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<&str> = current.split(':').collect();
    for p in extra.iter().rev() {
        if std::path::Path::new(p).is_dir() && !parts.contains(p) {
            parts.insert(0, p);
        }
    }
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("PATH", parts.join(":"));
    }
}

fn perform_updates(project_root: &Path, style: &Style, force: bool) -> Result<()> {
    let cache_file = project_root.join("crates/.modern_format_boost/.last_tool_refresh");
    if !force {
        match std::fs::metadata(&cache_file)
            .and_then(|meta| meta.modified())
            .and_then(|mtime| {
                std::time::SystemTime::now()
                    .duration_since(mtime)
                    .map_err(std::io::Error::other)
            }) {
            Ok(dur) if dur.as_secs() < 12 * 3600 => {
                println!(
                    "{}   · Updates checked within 12h. Skipping network pre-checks.{}",
                    style.dim, style.reset
                );
                return Ok(());
            }
            Ok(_) => {}
            Err(err) => debug!(
                "Update cache check skipped for {}: {err}",
                cache_file.display()
            ),
        }
    }

    println!(
        "\n{}{} Running Dependency Updates (cargo update, brew, pip, etc.)…{}\n",
        style.bold, style.cyan, style.reset
    );
    bootstrap_macos_path();

    if command_exists("brew") {
        run_update_step(
            "brew update",
            Command::new("brew").arg("update").current_dir(project_root),
            style,
            false,
        );
        let outdated_output = match Command::new("brew").arg("outdated").arg("-q").output() {
            Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
            Err(_) => String::new(),
        };
        let outdated_list: std::collections::HashSet<&str> =
            outdated_output.lines().map(|s| s.trim()).collect();

        for formula in BREW_MEDIA_FORMULAE {
            if outdated_list.contains(*formula) {
                run_update_step(
                    &format!("brew upgrade {formula}"),
                    Command::new("brew")
                        .arg("upgrade")
                        .arg(formula)
                        .current_dir(project_root),
                    style,
                    false,
                );
            }
        }
    }

    let req_path = project_root.join("crates/dev/scripts/requirements.txt");
    if req_path.is_file() {
        let python = if project_root
            .join("crates/.modern_format_boost/.venv/bin/python")
            .is_file()
        {
            project_root
                .join("crates/.modern_format_boost/.venv/bin/python")
                .to_string_lossy()
                .to_string()
        } else {
            "python3".to_string()
        };
        run_update_step(
            "pip requirements",
            Command::new(&python)
                .args(["-m", "pip", "install", "-U", "-q", "-r"])
                .arg(&req_path)
                .current_dir(project_root),
            style,
            false,
        );
    }

    if let Some(channel) = pinned_rust_channel(project_root) {
        if command_exists("rustup") {
            let ok = run_update_step(
                &format!("rustup toolchain install {channel}"),
                Command::new("rustup")
                    .args(["toolchain", "install", &channel])
                    .current_dir(project_root),
                style,
                true,
            );
            if ok {
                for component in rust_toolchain_components(project_root) {
                    run_update_step(
                        &format!("rustup component add {component}"),
                        Command::new("rustup")
                            .args(["component", "add", &component, "--toolchain", &channel])
                            .current_dir(project_root),
                        style,
                        false,
                    );
                }
            }
        }
    }

    if command_exists("cargo") {
        let use_rtk = command_exists("rtk");
        run_update_step(
            "cargo update",
            &mut if use_rtk {
                let mut c = Command::new("rtk");
                c.arg("cargo").arg("update").current_dir(project_root);
                c
            } else {
                let mut c = Command::new("cargo");
                c.arg("update").current_dir(project_root);
                c
            },
            style,
            false,
        );
        run_update_step(
            "cargo install kondo",
            &mut if use_rtk {
                let mut c = Command::new("rtk");
                c.arg("cargo")
                    .args(["install", "kondo", "--locked", "-q"])
                    .current_dir(project_root);
                c
            } else {
                let mut c = Command::new("cargo");
                c.args(["install", "kondo", "--locked", "-q"])
                    .current_dir(project_root);
                c
            },
            style,
            false,
        );
    }

    run_vue_dependency_update_validation(project_root, style)?;

    println!(
        "\n{}{} Dependency updates finished.{}\n",
        style.bold, style.green, style.reset
    );
    Ok(())
}

fn sync_app_bundle(project_root: &Path, style: &Style) -> Result<()> {
    let app_res_dir = project_root
        .join("Modern Format Boost.app")
        .join("Contents")
        .join("Resources");
    if !app_res_dir.is_dir() {
        return Ok(());
    }

    println!(
        "\n{}Syncing binaries to App Bundle...{}",
        style.dim, style.reset
    );
    let target_release = project_root.join("target").join("release");

    for bin in APP_BUNDLE_RESOURCE_BINARIES {
        let src = target_release.join(bin);
        if src.is_file() {
            let dest = app_res_dir.join(bin);
            let _ = fs::copy(&src, &dest);
        }
    }
    println!("{}App Bundle updated.{}", style.green, style.reset);

    sign_app_bundle(project_root, style)?;

    Ok(())
}

fn sign_app_bundle(project_root: &Path, style: &Style) -> Result<()> {
    let app_bundle = project_root.join("Modern Format Boost.app");
    if !app_bundle.is_dir() {
        return Ok(());
    }

    if !cfg!(target_os = "macos") {
        return Ok(());
    }

    if !command_exists("codesign") {
        anyhow::bail!("codesign not found; cannot seal Modern Format Boost.app");
    }

    let entitlements = project_root
        .join("crates")
        .join("dev")
        .join("src")
        .join("vue")
        .join("src-tauri")
        .join("entitlements.plist");
    let app_resources = app_bundle.join("Contents").join("Resources");
    for bin in APP_BUNDLE_RESOURCE_BINARIES {
        let bundled = app_resources.join(bin);
        if bundled.is_file() {
            let mut command = Command::new("codesign");
            command
                .arg("--force")
                .arg("--sign")
                .arg(app_bundle_codesign_identity());
            if entitlements.is_file() {
                command.arg("--entitlements").arg(&entitlements);
            }
            let status = command.arg(&bundled).status()?;
            if !status.success() {
                anyhow::bail!("codesign failed for {}", bundled.display());
            }
        }
    }

    let mut command = Command::new("codesign");
    command
        .arg("--force")
        .arg("--deep")
        .arg("--sign")
        .arg(app_bundle_codesign_identity());
    if entitlements.is_file() {
        command.arg("--entitlements").arg(&entitlements);
    }
    let status = command.arg(&app_bundle).status()?;

    if !status.success() {
        anyhow::bail!("codesign failed for {}", app_bundle.display());
    }

    println!(
        "{}App Bundle signed with {}.{}",
        style.green,
        app_bundle_codesign_identity(),
        style.reset
    );
    Ok(())
}

fn app_bundle_codesign_identity() -> &'static str {
    APP_BUNDLE_CODESIGN_IDENTITY
}

fn build_and_sync_gui(project_root: &Path, style: &Style) -> Result<()> {
    println!(
        "\n{}{} Building Tauri GUI...{}",
        style.bold, style.cyan, style.reset
    );
    run_vue_quality_checks(project_root, style)?;
    let vue_dir = vue_dir(project_root);

    let status = Command::new("npm")
        .arg("run")
        .arg("tauri")
        .arg("build")
        .arg("--")
        .arg("--bundles")
        .arg("app")
        .current_dir(&vue_dir)
        .status()?;

    if !status.success() {
        anyhow::bail!("Tauri build failed");
    }

    println!("{}Syncing App bundle...{}", style.dim, style.reset);
    let src_bundle = project_root
        .join("target")
        .join("release")
        .join("bundle")
        .join("macos")
        .join("Modern Format Boost.app");
    let dest_bundle = project_root.join("Modern Format Boost.app");

    if src_bundle.exists() {
        if dest_bundle.exists() {
            let _ = fs::remove_dir_all(&dest_bundle);
        }

        let cp_status = Command::new("cp")
            .arg("-R")
            .arg(&src_bundle)
            .arg(&dest_bundle)
            .status()?;

        if cp_status.success() {
            println!(
                "{}App bundle replaced successfully.{}",
                style.green, style.reset
            );
        } else {
            anyhow::bail!("Failed to copy App bundle");
        }
    } else {
        anyhow::bail!("Built app bundle not found at {:?}", src_bundle);
    }

    // Make sure we sync the Rust binaries into the newly created App bundle
    sync_app_bundle(project_root, style)?;

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let _ = setup_logger("mfb.smart_build");
    let project_root = get_project_root()?;
    let style = Style::current();

    if args.update {
        perform_updates(&project_root, &style, args.force)?;
        let cache_file = project_root.join("crates/.modern_format_boost/.last_tool_refresh");
        if let Some(parent) = cache_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create update cache directory {}", parent.display()))?;
        }
        std::fs::write(&cache_file, "done")
            .with_context(|| format!("write update cache marker {}", cache_file.display()))?;
    }

    let mut projects_to_build = Vec::new();
    if args.all {
        projects_to_build.push("crates/img");
        projects_to_build.push("crates/vid");
    } else {
        if args.img {
            projects_to_build.push("crates/img");
        }
        if args.vid {
            projects_to_build.push("crates/vid");
        }
        if args.hevc || args.av1 {
            if !projects_to_build.contains(&"crates/img") {
                projects_to_build.push("crates/img");
            }
            if !projects_to_build.contains(&"crates/vid") {
                projects_to_build.push("crates/vid");
            }
        }
    }

    if projects_to_build.is_empty() {
        projects_to_build.push("crates/img");
        projects_to_build.push("crates/vid");
        projects_to_build.push("crates/dev");
    }

    // Quiet mode check
    if args.quiet && !args.force {
        let mut needs_work = false;
        for proj in &projects_to_build {
            let bin = match *proj {
                "crates/img" => "img",
                "crates/vid" => "vid",
                _ => "verify",
            };
            let (action, _) = decide_build_action(&project_root, proj, bin, false);
            if action != "skip" {
                needs_work = true;
                break;
            }
        }
        if !needs_work {
            return Ok(());
        }
    }

    // Print header
    println!();
    println!(
        "{}{}{} Smart Build System v0.11.3 (Rust Edition){}",
        style.cyan,
        style.bold,
        pick_symbol("📦", "[BUILD]"),
        style.reset
    );
    println!(
        "{}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{}",
        style.dim, style.reset
    );
    println!(
        "{}Building:{} {}{}{}\n",
        style.cyan,
        style.reset,
        style.bold,
        projects_to_build.join(" "),
        style.reset
    );

    if args.clean_old {
        let targets = vec!["img", "vid"];
        clean_old_binaries(&project_root, &targets, style)?;
    }

    if args.clean {
        println!("{}Cleaning build artifacts...{}", style.yellow, style.reset);
        for proj in &projects_to_build {
            let _ = fs::remove_dir_all(project_root.join(proj).join("target/release/deps"));
            let _ = fs::remove_dir_all(project_root.join(proj).join("target/release/.fingerprint"));
        }
        let _ = fs::remove_dir_all(project_root.join("crates/foundation/target/release/deps"));
        println!();
        clean_with_kondo(&project_root, style)?;
    }

    if args.kondo && !args.clean {
        clean_with_kondo(&project_root, style)?;
    }

    if args.gui {
        build_and_sync_gui(&project_root, &style)?;
    }

    let mut rebuilt = 0;
    let mut skipped = 0;
    let mut failed = 0;

    for proj in &projects_to_build {
        let bin = match *proj {
            "crates/img" => "img",
            "crates/vid" => "vid",
            _ => "verify",
        };
        let (action, reason) = decide_build_action(&project_root, proj, bin, args.force);

        if action == "skip" {
            println!(
                "[OK] {}{}{} {}(up-to-date){}",
                style.bold, proj, style.reset, style.dim, style.reset
            );
            skipped += 1;
        } else {
            println!(
                "[BUILD] {}{}{} {}({}){}",
                style.bold, proj, style.reset, style.dim, reason, style.reset
            );
            if build_project(&project_root, proj, bin, 0, &args, style)? {
                println!("[OK] {}{}{} - compiled", style.bold, proj, style.reset);
                rebuilt += 1;
            } else {
                failed += 1;
            }
        }
    }

    println!(
        "\n{}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{}",
        style.dim, style.reset
    );

    if failed > 0 {
        println!(
            "{}Build failed: {} project(s){}",
            style.red, failed, style.reset
        );
        std::process::exit(1);
    }

    if rebuilt == 0 {
        println!(
            "{}OK: All binaries up-to-date (skipped {}){}",
            style.green, skipped, style.reset
        );
    } else {
        println!(
            "{}OK: Built {}, skipped {}{}",
            style.green, rebuilt, skipped, style.reset
        );
    }

    if args.verbose || rebuilt > 0 {
        println!("\n{}Binary info:{}", style.bold, style.reset);
        for proj in &projects_to_build {
            let bin = match *proj {
                "crates/img" => "img",
                "crates/vid" => "vid",
                _ => "verify",
            };
            let p = get_binary_path(&project_root, bin);
            if p.is_file() {
                match fs::metadata(&p) {
                    Ok(meta) => {
                        let sz_mb = (meta.len() as f64) / (1024.0 * 1024.0);
                        match meta.modified() {
                            Ok(modified) => {
                                let datetime: chrono::DateTime<chrono::Local> = modified.into();
                                let mtime_str = datetime.format("%Y-%m-%d %H:%M").to_string();
                                println!(
                                    "  {}{}{}: {:.1}M, {}",
                                    style.bold, bin, style.reset, sz_mb, mtime_str
                                );
                            }
                            Err(_err) => {}
                        }
                    }
                    Err(_err) => {}
                }
            }
        }
    }

    sync_app_bundle(&project_root, &style)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decide_build_action_force() {
        let tempdir = tempfile::tempdir().unwrap();
        // Since we pass force = true, decide_build_action should always return ("rebuild", "force")
        let (action, reason) = decide_build_action(tempdir.path(), "crates/img", "img", true);
        assert_eq!(action, "rebuild");
        assert_eq!(reason, "force");
    }

    #[test]
    fn test_decide_build_action_missing() {
        let tempdir = tempfile::tempdir().unwrap();
        // The binary doesn't exist, so decide_build_action should return ("rebuild", "binary-missing")
        let (action, reason) = decide_build_action(tempdir.path(), "crates/img", "img", false);
        assert_eq!(action, "rebuild");
        assert_eq!(reason, "binary-missing");
    }

    #[test]
    fn test_toolchain_name_from_cargo_path_matches_python_helper() {
        let cargo = Path::new("/tmp/rustup/toolchains/nightly-test/bin/cargo");
        assert_eq!(
            toolchain_name_from_cargo_path(cargo),
            Some("nightly-test".to_string())
        );
        assert_eq!(
            toolchain_name_from_cargo_path(Path::new("/usr/bin/cargo")),
            None
        );
    }

    #[test]
    fn test_app_bundle_codesign_identity_is_stable() {
        let identity = app_bundle_codesign_identity();
        assert_eq!(identity, "MFB-Dev-Signing");
        assert_ne!(identity, "-");
    }

    #[test]
    fn test_app_bundle_resource_binaries_include_terminal_processor() {
        assert!(APP_BUNDLE_RESOURCE_BINARIES.contains(&"drag_and_drop_processor"));
    }

    #[test]
    fn test_vue_quality_scripts_cover_lint_format_dependencies_and_build() {
        assert_eq!(
            vue_quality_script_names(),
            &["lint", "format:check", "deps:check", "build"]
        );
    }

    #[test]
    fn test_vue_update_scripts_validate_dependency_updates() {
        assert_eq!(vue_update_script_names(), &["deps:update", "deps:check"]);
    }

    #[test]
    fn test_resolve_rust_toolchain_prefers_explicit_env_toolchain() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let cargo = tempdir.path().join("toolchains/nightly-explicit/bin/cargo");
        fs::create_dir_all(cargo.parent().unwrap())?;
        fs::write(&cargo, "")?;

        let resolved = resolve_rust_toolchain_from(&RustToolchainInput {
            rustup_home: tempdir.path().to_path_buf(),
            rustup_toolchain: Some("nightly-explicit".to_string()),
            rustup_which_cargo: None,
            path_cargo: Some(PathBuf::from("/usr/bin/cargo")),
            prefer: "nightly",
        });

        assert_eq!(resolved.cargo, cargo);
        assert_eq!(resolved.name, Some("nightly-explicit".to_string()));
        Ok(())
    }

    #[test]
    fn test_resolve_rust_toolchain_uses_rustup_which_before_path() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let cargo = tempdir.path().join("toolchains/nightly-rustup/bin/cargo");
        fs::create_dir_all(cargo.parent().unwrap())?;
        fs::write(&cargo, "")?;

        let resolved = resolve_rust_toolchain_from(&RustToolchainInput {
            rustup_home: tempdir.path().join("missing"),
            rustup_toolchain: None,
            rustup_which_cargo: Some(cargo.clone()),
            path_cargo: Some(PathBuf::from("/usr/bin/cargo")),
            prefer: "nightly",
        });

        assert_eq!(resolved.cargo, cargo);
        assert_eq!(resolved.name, Some("nightly-rustup".to_string()));
        Ok(())
    }

    #[test]
    fn test_toolchain_env_prepends_bin_and_sets_rustup_toolchain() {
        let tc = RustToolchain {
            cargo: PathBuf::from("/tmp/rustup/toolchains/nightly-test/bin/cargo"),
            bin_dir: PathBuf::from("/tmp/rustup/toolchains/nightly-test/bin"),
            name: Some("nightly-test".to_string()),
        };
        let env = toolchain_env(&tc);
        assert!(env.iter().any(|(key, value)| {
            *key == "PATH"
                && value
                    .to_string_lossy()
                    .starts_with("/tmp/rustup/toolchains/nightly-test/bin:")
        }));
        assert!(
            env.iter()
                .any(|(key, value)| { *key == "RUSTUP_TOOLCHAIN" && value == "nightly-test" })
        );
    }
}
