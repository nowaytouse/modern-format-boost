//! Modern Format Boost - Smart Build System in Rust.
//! Compiles img and vid release binaries incrementally based on source file
//! modifications.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::logger::setup_logger;
use dev::infra::ui_tokens::pick_symbol;
use foundation::tracing::{debug, error, info};
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
const VUE_UPDATE_SCRIPTS: &[&str] = &["deps:update", "deps:check"];
const RUST_SOURCE_EXTENSIONS: &[&str] = &["rs", "sql", "c", "h", "cpp", "cc", "proto", "py", "sh"];
const GUI_SOURCE_EXTENSIONS: &[&str] = &[
    "css", "html", "icns", "ico", "js", "json", "lock", "plist", "png", "sh", "svg", "swift",
    "toml", "ts", "tsx", "vue",
];
const IGNORED_SOURCE_DIRECTORIES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "coverage",
    "__pycache__",
];

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

/// Smart Build — builds img / vid plus the packaged terminal launcher and
/// verification tool, and optionally the native macOS GUI.
///
/// Default (no flags): build img + vid + verify + drag_and_drop_processor if
/// sources are newer than binaries.
#[derive(Parser, Debug)]
#[command(about = "Smart Build System — incremental Rust + native macOS builder")]
struct Args {
    /// Force rebuild even when binaries are up-to-date
    #[arg(long, short = 'f')]
    force: bool,

    /// Clean stale deps and run kondo before building
    #[arg(long, short = 'c')]
    clean: bool,

    /// Show binary size and mtime after build
    #[arg(long, short = 'v')]
    verbose: bool,

    /// Build every Rust binary packaged inside the app, then refresh the native GUI only when its inputs changed
    #[arg(long, short = 'a')]
    all: bool,

    /// Build image tools only (img binary)
    #[arg(long)]
    img: bool,

    /// Build video tools only (vid binary)
    #[arg(long)]
    vid: bool,

    /// No output when all binaries are already up-to-date
    #[arg(long, short = 'q')]
    quiet: bool,

    /// Update dependencies first (brew, cargo, pip, rustup)
    #[arg(long, short = 'u')]
    update: bool,

    /// Build the native Vue GUI and sync the .app bundle
    #[arg(long)]
    gui: bool,

    /// Build Rust binaries only — skip the native Vue GUI step
    #[arg(long, short = 'r')]
    rust_only: bool,

    /// Patch-cycle shortcut: incremental Rust-only build with verbose verification.
    /// Use after small source edits without forcing unrelated targets to rebuild.
    #[arg(long, short = 'p')]
    patch: bool,

    /// Build a single named binary only (e.g. img, vid, drag_and_drop_processor).
    /// Resolves to the owning crate automatically.
    #[arg(long, value_name = "BINARY", conflicts_with_all = ["all", "img", "vid"])]
    bin: Option<String>,

    /// Skip compilation — just sync existing target/release binaries to the .app bundle.
    /// Fast path after a manual cargo build.
    #[arg(long, short = 's')]
    sync: bool,
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

fn vue_update_script_names() -> &'static [&'static str] {
    VUE_UPDATE_SCRIPTS
}

fn vue_dir(project_root: &Path) -> PathBuf {
    project_root.join("crates").join("gui")
}

fn native_gui_dir(project_root: &Path) -> PathBuf {
    project_root.join("crates").join("gui").join("src-macos")
}

fn native_app_bundle_path(project_root: &Path) -> PathBuf {
    project_root
        .join("target")
        .join("release")
        .join("bundle")
        .join("macos")
        .join("Modern Format Boost.app")
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

fn is_ignored_source_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| IGNORED_SOURCE_DIRECTORIES.contains(&name))
}

fn newest_source_mtime_in_dir(dir: &Path, extensions: &[&str]) -> f64 {
    if !dir.is_dir() {
        return 0.0;
    }

    let mut newest = get_mtime(&dir.join("Cargo.toml"));
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir() || !is_ignored_source_directory(entry.path())
        })
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
        {
            newest = newest.max(get_mtime(path));
        }
    }
    newest
}

fn direct_workspace_dependencies(project_dir: &str) -> &'static [&'static str] {
    match project_dir {
        "crates/img" => &["crates/foundation"],
        "crates/vid" => &["crates/foundation"],
        "crates/dev" => &["crates/img", "crates/vid", "crates/foundation"],
        _ => &[],
    }
}

fn newest_dev_binary_source_mtime(project_root: &Path, binary_name: &str) -> f64 {
    let dev_dir = project_root.join("crates/dev");
    if !dev_dir.is_dir() {
        return 0.0;
    }

    let source_dir = dev_dir.join("src");
    let bin_dir = source_dir.join("bin");
    let bin_file = bin_dir.join(format!("{binary_name}.rs"));
    let bin_module_dir = bin_dir.join(binary_name);
    let mut newest =
        get_mtime(&dev_dir.join("Cargo.toml")).max(get_mtime(&dev_dir.join("build.rs")));

    for entry in walkdir::WalkDir::new(&source_dir)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir() || !is_ignored_source_directory(entry.path())
        })
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file()
            || !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| RUST_SOURCE_EXTENSIONS.contains(&extension))
        {
            continue;
        }
        if path.starts_with(&bin_dir) && path != bin_file && !path.starts_with(&bin_module_dir) {
            continue;
        }
        newest = newest.max(get_mtime(path));
    }

    newest
}

fn get_newest_binary_source_mtime(
    project_root: &Path,
    project_dir: &str,
    binary_name: &str,
) -> f64 {
    let mut newest = if project_dir == "crates/dev" {
        newest_dev_binary_source_mtime(project_root, binary_name)
    } else {
        newest_source_mtime_in_dir(&project_root.join(project_dir), RUST_SOURCE_EXTENSIONS)
    };
    for dependency in direct_workspace_dependencies(project_dir) {
        newest = newest.max(newest_source_mtime_in_dir(
            &project_root.join(dependency),
            RUST_SOURCE_EXTENSIONS,
        ));
    }
    for config in ["Cargo.toml", "Cargo.lock", RUST_TOOLCHAIN_FILE] {
        newest = newest.max(get_mtime(&project_root.join(config)));
    }
    newest
}

fn gui_needs_rebuild(project_root: &Path) -> bool {
    let vue_root = vue_dir(project_root);
    let newest_input = newest_source_mtime_in_dir(&vue_root, GUI_SOURCE_EXTENSIONS);
    let bundle_binary = native_app_bundle_path(project_root)
        .join("Contents")
        .join("MacOS")
        .join("Modern Format Boost");
    !bundle_binary.is_file() || newest_input > get_mtime(&bundle_binary)
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

    let source_mtime = get_newest_binary_source_mtime(project_root, project_dir, binary_name);
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
    build_all_bins: bool,
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
    command
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(&manifest);
    if !build_all_bins {
        command.arg("--bin").arg(binary_name);
    }
    let status = command.current_dir(project_root).status()?;

    if !status.success() {
        error!("Cargo compilation failed for {project_dir} with status {status}");
        println!(
            "{}FAILURE: compilation failed for {}{}",
            style.red, project_dir, style.reset
        );
        return Ok(false);
    }

    // For non-forced builds, an unchanged output after a source-triggered build
    // is suspicious. A forced build is also allowed to reuse Cargo's cache.
    {
        let expected_binaries = if build_all_bins {
            match project_dir {
                "crates/img" => vec!["img"],
                "crates/vid" => vec!["vid"],
                _ => vec![
                    "verify",
                    "cache_cleaner",
                    "database_manager",
                    "collect_optimized",
                    "merge_xmp",
                    "icloud_import",
                    "drag_and_drop_processor",
                ],
            }
        } else {
            vec![binary_name]
        };

        let mut any_updated = false;
        let mut newest_mtime = 0.0;
        let mut missing_binary = false;

        for bin in &expected_binaries {
            let p = get_binary_path(project_root, bin);
            if !p.is_file() {
                missing_binary = true;
                break;
            }
            let mtime = get_mtime(&p);
            if mtime > newest_mtime {
                newest_mtime = mtime;
            }
            if mtime >= (compile_start_time - 2.0) {
                any_updated = true;
            }
        }

        if missing_binary {
            println!(
                "{}ERROR: TIMESTAMP VERIFICATION FAILED: Binary not found{}",
                style.red, style.reset
            );
            return Ok(false);
        }

        if !any_updated && !args.force {
            println!(
                "{}FAILURE: Cargo returned success but did not refresh an expected binary{}",
                style.red, style.reset
            );
            return Ok(false);
        }

        let datetime: chrono::DateTime<chrono::Local> = std::time::SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs_f64(newest_mtime))
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .into();
        let mtime_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        println!(
            "   {}\u{2713} outputs ready{}  {}{}{}",
            style.green, style.reset, style.dim, mtime_str, style.reset
        );
    }

    Ok(true)
}

/// Compile several changed single-binary workspace members in one Cargo
/// invocation. This keeps the common img + vid + verify path incremental while
/// avoiding repeated Cargo process startup and dependency graph resolution.
fn build_workspace_projects(
    project_root: &Path,
    projects: &[(&str, String, bool)],
    args: &Args,
    style: Style,
) -> Result<bool> {
    if projects.len() < 2
        || projects
            .iter()
            .any(|(_, _, build_all_bins)| *build_all_bins)
    {
        anyhow::bail!("workspace batching requires two or more single-binary targets");
    }

    let compile_start_time = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(_err) => 0.0,
    };
    let toolchain = resolve_rust_toolchain_from(&current_toolchain_input());
    let manifest = project_root.join("Cargo.toml");
    let mut packages = Vec::new();
    for (project_dir, _, _) in projects {
        let package = project_dir.strip_prefix("crates/").unwrap_or(project_dir);
        if !packages.contains(&package) {
            packages.push(package);
        }
    }

    info!(
        "Executing one batched cargo build for {} targets",
        projects.len()
    );
    debug!(
        "Cargo batch command: {} build --release --manifest-path {}",
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
    command
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(&manifest);
    for package in packages {
        command.arg("-p").arg(package);
    }
    for (_, binary_name, _) in projects {
        command.arg("--bin").arg(binary_name);
    }
    let status = command.current_dir(project_root).status()?;
    if !status.success() {
        println!(
            "{}FAILURE: batched compilation failed for {} targets{}",
            style.red,
            projects.len(),
            style.reset
        );
        return Ok(false);
    }

    let mut any_updated = false;
    let mut newest_mtime = 0.0_f64;
    for (_, binary_name, _) in projects {
        let path = get_binary_path(project_root, binary_name);
        if !path.is_file() {
            println!(
                "{}ERROR: TIMESTAMP VERIFICATION FAILED: Binary not found{}",
                style.red, style.reset
            );
            return Ok(false);
        }
        let mtime = get_mtime(&path);
        newest_mtime = newest_mtime.max(mtime);
        any_updated |= mtime >= (compile_start_time - 2.0);
    }
    if !any_updated && !args.force {
        println!(
            "{}FAILURE: Cargo returned success but did not refresh an expected binary{}",
            style.red, style.reset
        );
        return Ok(false);
    }
    let datetime: chrono::DateTime<chrono::Local> = std::time::SystemTime::UNIX_EPOCH
        .checked_add(std::time::Duration::from_secs_f64(newest_mtime))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        .into();
    println!(
        "   {}\u{2713} batched outputs ready{}  {}{}{}",
        style.green,
        style.reset,
        style.dim,
        datetime.format("%Y-%m-%d %H:%M:%S"),
        style.reset
    );
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
    let home_root = foundation::process_lock::get_mfb_root().context("resolve MFB state root")?;
    let cache_file = home_root.join(".last_tool_refresh");
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
        let venv_py = home_root.join(".venv/bin/python");
        let python = if venv_py.is_file() {
            venv_py.to_string_lossy().to_string()
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

    if let Some(channel) = pinned_rust_channel(project_root)
        && command_exists("rustup")
    {
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

fn bundle_file_needs_sync(src: &Path, dest: &Path) -> bool {
    let Ok(src_meta) = fs::metadata(src) else {
        return false;
    };
    let Ok(dest_meta) = fs::metadata(dest) else {
        return true;
    };

    match (src_meta.modified(), dest_meta.modified()) {
        (Ok(src_modified), Ok(dest_modified)) => src_modified > dest_modified,
        _ => true,
    }
}

fn sync_foundation_dylib_artifact(project_root: &Path, style: &Style, force: bool) -> Result<bool> {
    let dylib_name = if cfg!(target_os = "macos") {
        "libfoundation.dylib"
    } else if cfg!(target_os = "windows") {
        "foundation.dll"
    } else {
        "libfoundation.so"
    };

    let target_dylib = project_root.join("target").join("release").join(dylib_name);
    let home_root = foundation::process_lock::get_mfb_root().context("resolve MFB state root")?;
    let artifact_dir = home_root.join("artifacts");
    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("create artifact dir {}", artifact_dir.display()))?;
    let artifact_dylib = artifact_dir.join(dylib_name);

    if force || !target_dylib.is_file() {
        println!(
            "{}Building foundation dylib (cdylib)...{}",
            style.cyan, style.reset
        );
        let status = Command::new("cargo")
            .args([
                "rustc",
                "--release",
                "--locked",
                "-p",
                "foundation",
                "--lib",
                "--crate-type",
                "cdylib",
            ])
            .current_dir(project_root)
            .status()
            .context("cargo rustc foundation cdylib failed")?;
        if !status.success() {
            anyhow::bail!(
                "cargo rustc cdylib failed with exit status {:?}",
                status.code()
            );
        }
    }
    if !target_dylib.is_file() {
        anyhow::bail!(
            "cargo build succeeded but foundation dylib is missing: {}",
            target_dylib.display()
        );
    }

    if force || bundle_file_needs_sync(&target_dylib, &artifact_dylib) {
        fs::copy(&target_dylib, &artifact_dylib).with_context(|| {
            format!(
                "copy {} to {}",
                target_dylib.display(),
                artifact_dylib.display()
            )
        })?;
        println!(
            "{}Synced foundation dylib to artifact: {}{}",
            style.green,
            artifact_dylib.display(),
            style.reset
        );
    }

    let app_bundle = project_root.join("Modern Format Boost.app");
    if !app_bundle.is_dir() {
        return Ok(false);
    }

    let app_res = app_bundle
        .join("Contents")
        .join("Resources")
        .join(dylib_name);
    let app_fw_dir = app_bundle.join("Contents").join("Frameworks");
    fs::create_dir_all(&app_fw_dir)
        .with_context(|| format!("create framework dir {}", app_fw_dir.display()))?;
    let app_fw = app_fw_dir.join(dylib_name);
    let mut app_changed = false;
    for destination in [&app_res, &app_fw] {
        if force || bundle_file_needs_sync(&target_dylib, destination) {
            fs::copy(&target_dylib, destination).with_context(|| {
                format!(
                    "copy {} to {}",
                    target_dylib.display(),
                    destination.display()
                )
            })?;
            app_changed = true;
        }
    }

    if app_changed && cfg!(target_os = "macos") {
        if !command_exists("codesign") {
            anyhow::bail!("codesign not found; cannot sign foundation dylib");
        }
        for destination in [&app_res, &app_fw] {
            let status = Command::new("codesign")
                .arg("--force")
                .arg("--sign")
                .arg(app_bundle_codesign_identity()?)
                .arg(destination)
                .status()
                .with_context(|| format!("codesign {}", destination.display()))?;
            if !status.success() {
                anyhow::bail!("codesign failed for {}", destination.display());
            }
        }
    }

    Ok(app_changed)
}

fn sync_app_bundle(project_root: &Path, style: &Style, force: bool) -> Result<()> {
    let foundation_changed = sync_foundation_dylib_artifact(project_root, style, force)?;

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

    let mut changed_bins = Vec::new();
    for bin in APP_BUNDLE_RESOURCE_BINARIES {
        let src = target_release.join(bin);
        if src.is_file() {
            let dest = app_res_dir.join(bin);
            if bundle_file_needs_sync(&src, &dest) {
                fs::copy(&src, &dest)
                    .with_context(|| format!("sync {} to App bundle", src.display()))?;
                changed_bins.push(*bin);
            }
        }
    }
    if changed_bins.is_empty() && !foundation_changed {
        println!("{}App Bundle already current.{}", style.dim, style.reset);
        return Ok(());
    }

    if foundation_changed {
        println!(
            "{}App Bundle foundation dylib updated.{}",
            style.green, style.reset
        );
    }
    if !changed_bins.is_empty() {
        println!(
            "{}App Bundle updated ({} binary file(s)).{}",
            style.green,
            changed_bins.len(),
            style.reset
        );
    }

    sign_app_bundle(project_root, style, &changed_bins)?;

    Ok(())
}

fn sign_app_bundle(project_root: &Path, style: &Style, changed_bins: &[&str]) -> Result<()> {
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

    let entitlements = native_gui_dir(project_root).join("entitlements.plist");
    let app_resources = app_bundle.join("Contents").join("Resources");
    let signing_identity = app_bundle_codesign_identity()?;
    for bin in changed_bins {
        let bundled = app_resources.join(bin);
        if bundled.is_file() {
            let mut command = Command::new("codesign");
            command.arg("--force").arg("--sign").arg(&signing_identity);
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
    command.arg("--force").arg("--sign").arg(&signing_identity);
    if entitlements.is_file() {
        command.arg("--entitlements").arg(&entitlements);
    }
    let status = command.arg(&app_bundle).status()?;

    if !status.success() {
        anyhow::bail!("codesign failed for {}", app_bundle.display());
    }

    println!(
        "{}App Bundle signed with {}.{}",
        style.green, signing_identity, style.reset
    );
    Ok(())
}

/// Resolve the codesign identity to use for the app bundle.
///
/// Priority:
///   1. `CODESIGN_IDENTITY` environment variable (CI / release override)
///   2. `MFB-Dev-Signing` if the certificate is present in the local keychain
fn app_bundle_codesign_identity() -> Result<String> {
    // 1. Explicit env override
    if let Ok(id) = std::env::var("CODESIGN_IDENTITY") {
        let id = id.trim().to_owned();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    // 2. MFB-Dev-Signing if the certificate exists in the keychain
    let available = Command::new("/usr/bin/security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("MFB-Dev-Signing"))
        .unwrap_or(false);
    if available {
        return Ok(APP_BUNDLE_CODESIGN_IDENTITY.to_owned());
    }
    anyhow::bail!(concat!(
        "No stable code-signing identity found. Install MFB-Dev-Signing or set ",
        "CODESIGN_IDENTITY explicitly; refusing ad-hoc signing because it invalidates ",
        "persistent Photos Automation grants."
    ))
}

/// Compile the Swift native host and assemble the macOS .app bundle at
/// `target/release/bundle/macos/Modern Format Boost.app`.
///
/// This replaces the former `crates/gui/src-macos/build.sh` shell script
/// and is invoked by `build_and_sync_gui()` after the Vue build.
fn compile_swift_native_host(project_root: &Path, style: &Style) -> Result<()> {
    let native_dir = native_gui_dir(project_root);
    let gui_dir = vue_dir(project_root);
    let bundle = native_app_bundle_path(project_root);
    let macos_dir = bundle.join("Contents").join("MacOS");
    let resources_dir = bundle.join("Contents").join("Resources");

    // Detect host architecture (arm64 or x86_64)
    let arch_out = Command::new("uname")
        .arg("-m")
        .output()
        .context("uname -m")?;
    let arch = String::from_utf8_lossy(&arch_out.stdout).trim().to_owned();
    match arch.as_str() {
        "arm64" | "x86_64" => {}
        other => anyhow::bail!("Unsupported macOS architecture: {other}"),
    }

    // Verify the Vue build output exists
    let index_html = gui_dir.join("dist").join("index.html");
    if !index_html.is_file() {
        anyhow::bail!("Vue build output missing: {}", index_html.display());
    }
    // Guard against WKWebView-incompatible module attributes
    let index_content = fs::read_to_string(&index_html).context("read dist/index.html")?;
    if index_content.contains("type=\"module\"") || index_content.contains("crossorigin") {
        anyhow::bail!(
            "Vue entry point contains type=\"module\" or crossorigin attributes, \
             which are incompatible with bundled WKWebView file loading"
        );
    }

    // (Re-)create the bundle skeleton
    if bundle.exists() {
        fs::remove_dir_all(&bundle)
            .with_context(|| format!("remove stale app bundle {}", bundle.display()))?;
    }
    fs::create_dir_all(&macos_dir).context("create bundle MacOS dir")?;
    fs::create_dir_all(&resources_dir).context("create bundle Resources dir")?;

    println!(
        "{}  Compiling Swift native host ({arch})...{}",
        style.cyan, style.reset
    );
    let swift_src = native_dir.join("main.swift");
    let host_binary = macos_dir.join("Modern Format Boost");
    let target_triple = format!("{arch}-apple-macos13.0");
    let status = Command::new("xcrun")
        .args([
            "swiftc",
            "-swift-version",
            "5",
            "-O",
            "-target",
            &target_triple,
            "-framework",
            "AppKit",
            "-framework",
            "CoreServices",
            "-framework",
            "WebKit",
        ])
        .arg(&swift_src)
        .arg("-o")
        .arg(&host_binary)
        .status()
        .context("xcrun swiftc")?;
    if !status.success() {
        anyhow::bail!("Swift native host compilation failed");
    }

    // Copy bundle resources
    let info_src = native_dir.join("Info.plist");
    let info_dst = bundle.join("Contents").join("Info.plist");
    fs::copy(&info_src, &info_dst)
        .with_context(|| format!("copy Info.plist to {}", info_dst.display()))?;

    let icon_src = native_dir.join("icon.icns");
    let icon_dst = resources_dir.join("icon.icns");
    fs::copy(&icon_src, &icon_dst)
        .with_context(|| format!("copy icon.icns to {}", icon_dst.display()))?;

    let dist_src = gui_dir.join("dist");
    let dist_dst = resources_dir.join("dist");
    let status = Command::new("ditto")
        .arg(&dist_src)
        .arg(&dist_dst)
        .status()
        .context("ditto dist -> Resources/dist")?;
    if !status.success() {
        anyhow::bail!("ditto failed copying Vue dist");
    }

    // Validate plists
    for plist in [&info_dst, &native_dir.join("entitlements.plist")] {
        if plist.is_file() {
            let s = Command::new("plutil")
                .arg("-lint")
                .arg(plist)
                .status()
                .with_context(|| format!("plutil -lint {}", plist.display()))?;
            if !s.success() {
                anyhow::bail!("plutil -lint failed for {}", plist.display());
            }
        }
    }

    // Run native host self-test before signing
    println!(
        "{}  Running native host self-test...{}",
        style.cyan, style.reset
    );
    let test_status = Command::new(&host_binary)
        .arg("--self-test")
        .status()
        .context("native host --self-test")?;
    if !test_status.success() {
        anyhow::bail!("Native host self-test failed");
    }

    println!(
        "{}  Swift native host compiled and assembled.{}",
        style.green, style.reset
    );
    Ok(())
}

fn build_and_sync_gui(project_root: &Path, style: &Style) -> Result<()> {
    println!(
        "\n{}{} Building native macOS GUI...{}",
        style.bold, style.cyan, style.reset
    );
    let vue_dir = vue_dir(project_root);

    // Step 1: Vue frontend build (dist/ only — Swift compilation handled below)
    let status = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(&vue_dir)
        .status()?;
    if !status.success() {
        anyhow::bail!("Vue frontend build failed");
    }

    // Step 2: Compile Swift native host and assemble .app bundle skeleton
    compile_swift_native_host(project_root, style)?;

    println!("{}Syncing App bundle...{}", style.dim, style.reset);
    let src_bundle = native_app_bundle_path(project_root);
    let dest_bundle = project_root.join("Modern Format Boost.app");

    if src_bundle.exists() {
        if dest_bundle.exists() {
            fs::remove_dir_all(&dest_bundle)
                .with_context(|| format!("remove stale app bundle {}", dest_bundle.display()))?;
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

    // Step 3: Sync Rust binaries and sign the final bundle
    sync_app_bundle(project_root, style, false)?;

    Ok(())
}

/// Map a binary name to its owning crate directory (relative to workspace root).
///
/// # Errors
/// Returns an error when the binary name is not known.
fn bin_name_to_crate_dir(name: &str) -> Result<&'static str> {
    match name {
        "img" => Ok("crates/img"),
        "vid" => Ok("crates/vid"),
        // All dev-crate binaries map to crates/dev.
        "verify"
        | "cache_cleaner"
        | "database_manager"
        | "collect_optimized"
        | "merge_xmp"
        | "icloud_import"
        | "drag_and_drop_processor"
        | "smart_build"
        | "check_all"
        | "install_deps"
        | "ingest_audit"
        | "index_gallery"
        | "session_audit" => Ok("crates/dev"),
        other => anyhow::bail!(
            "unknown binary '{other}'; valid: img, vid, or any dev binary \
             (verify, cache_cleaner, database_manager, collect_optimized, \
             merge_xmp, icloud_import, drag_and_drop_processor, ...)"
        ),
    }
}

fn process_cmd_joined_lower(process: &sysinfo::Process) -> String {
    process
        .cmd()
        .iter()
        .map(|arg| arg.to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn process_path_is_under_project(path: Option<&Path>, project_root: &Path) -> bool {
    path.is_some_and(|path| path.starts_with(project_root))
}

fn command_mentions_project(cmd_joined: &str, project_root: &Path) -> bool {
    let root = project_root.to_string_lossy().to_lowercase();
    cmd_joined.contains(root.as_str())
}

fn process_belongs_to_project(
    process: &sysinfo::Process,
    project_root: &Path,
    cmd_joined: &str,
) -> bool {
    process_path_is_under_project(process.exe(), project_root)
        || process_path_is_under_project(process.cwd(), project_root)
        || command_mentions_project(cmd_joined, project_root)
}

fn process_name_matches_project_tool(name: &str, cmd_joined: &str) -> bool {
    let target_names = [
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
    let name_lower = name.to_lowercase();
    let is_project_vite_node = name == "node" && cmd_joined.contains("vite");
    target_names.contains(&name) || name_lower.contains("vite") || is_project_vite_node
}

fn should_terminate_process_identity(
    name: &str,
    cmd_joined: &str,
    belongs_to_project: bool,
) -> bool {
    if name == "Modern Format Boost" || name.contains("Modern Format Boost") {
        return true;
    }
    belongs_to_project && process_name_matches_project_tool(name, cmd_joined)
}

fn should_terminate_running_instance(process: &sysinfo::Process, project_root: &Path) -> bool {
    let name = process.name().to_string_lossy();
    let cmd_joined = process_cmd_joined_lower(process);
    should_terminate_process_identity(
        &name,
        &cmd_joined,
        process_belongs_to_project(process, project_root, &cmd_joined),
    )
}

fn terminate_running_instances(style: &Style, project_root: &Path) -> Result<()> {
    println!(
        "{}{}{} Checking and terminating project-scoped running applications and terminal processes...{}",
        style.yellow,
        style.bold,
        pick_symbol("⚠️", "[PROCESS]"),
        style.reset
    );

    let mut s = sysinfo::System::new_all();
    s.refresh_all();
    let project_root = project_root
        .canonicalize()
        .unwrap_or_else(|_err| project_root.to_path_buf());

    let mut terminated_pids = Vec::new();
    let current_pid = std::process::id();

    for (pid, process) in s.processes() {
        let pid_str = pid.to_string();
        let pid_val = pid_str.parse::<u32>().unwrap_or_default();
        if pid_val == current_pid {
            continue;
        }

        if should_terminate_running_instance(process, &project_root) {
            let name = process.name().to_string_lossy();
            println!(
                "   {} Interrupted running process: {} (PID {})",
                pick_symbol("⊖", "[-]"),
                name,
                pid_str
            );
            process.kill_with(sysinfo::Signal::Term);
            terminated_pids.push(*pid);
        }
    }

    if !terminated_pids.is_empty() {
        // Wait a bit for processes to exit gracefully
        std::thread::sleep(std::time::Duration::from_millis(800));

        // Refresh and force kill if still alive
        s.refresh_all();
        for pid in terminated_pids {
            if let Some(process) = s.process(pid) {
                let name = process.name().to_string_lossy();
                println!(
                    "   {} Force terminating remaining process: {} (PID {})",
                    pick_symbol("💥", "[!]"),
                    name,
                    pid
                );
                process.kill_with(sysinfo::Signal::Kill);
            }
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    let _ = setup_logger("mfb.smart_build");
    let project_root = get_project_root()?;
    let style = Style::current();

    // Interrupt running GUI app and child binaries if forcing full build
    if args.all && args.force {
        terminate_running_instances(&style, &project_root)?;
    }

    // Keep patch cycles incremental; callers who need a clean rebuild opt into --force.
    if args.patch {
        args.rust_only = true;
        args.verbose = true;
        println!(
            "{}{}[patch mode]{} incremental + rust-only + verbose",
            style.cyan, style.bold, style.reset
        );
    }

    if args.update {
        perform_updates(&project_root, &style, args.force)?;
        let home_root =
            foundation::process_lock::get_mfb_root().context("resolve MFB state root")?;
        let cache_file = home_root.join(".last_tool_refresh");
        if let Some(parent) = cache_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create update cache directory {}", parent.display()))?;
        }
        std::fs::write(&cache_file, "done")
            .with_context(|| format!("write update cache marker {}", cache_file.display()))?;
    }

    // --sync: skip compilation entirely, just push current target/release binaries into .app.
    if args.sync {
        println!(
            "{}{}[sync mode]{} Syncing binaries to app bundle (no build).",
            style.cyan, style.bold, style.reset
        );
        sync_app_bundle(&project_root, &style, args.force)?;
        return Ok(());
    }

    let mut targets_to_build: Vec<(&str, String, bool)> = Vec::new();
    if args.all {
        // Check every app-bundled tool, but still compile only binaries whose own
        // sources are newer. This makes --all complete without turning it into a
        // forced workspace rebuild.
        for binary_name in APP_BUNDLE_RESOURCE_BINARIES {
            let crate_dir = bin_name_to_crate_dir(binary_name)?;
            targets_to_build.push((crate_dir, (*binary_name).to_string(), false));
        }
    } else if let Some(ref bin_name) = args.bin {
        // --bin <name>: resolve the binary to its owning crate.
        let crate_dir = bin_name_to_crate_dir(bin_name).with_context(|| {
            format!("unknown binary '{bin_name}'; valid values: img, vid, or any dev binary")
        })?;
        targets_to_build.push((crate_dir, bin_name.clone(), false));
    } else {
        if args.img {
            targets_to_build.push(("crates/img", "img".to_string(), false));
        }
        if args.vid {
            targets_to_build.push(("crates/vid", "vid".to_string(), false));
        }
    }

    if targets_to_build.is_empty() {
        targets_to_build.push(("crates/img", "img".to_string(), false));
        targets_to_build.push(("crates/vid", "vid".to_string(), false));
        targets_to_build.push(("crates/dev", "verify".to_string(), false));
        targets_to_build.push(("crates/dev", "drag_and_drop_processor".to_string(), false));
    }

    // Quiet mode check
    if args.quiet && !args.force {
        let mut needs_work = false;
        for (project_dir, binary_name, _) in &targets_to_build {
            let (action, _) = decide_build_action(&project_root, project_dir, binary_name, false);
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
        "{}{}{} Smart Build System v0.12.0 (Rust Edition){}",
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
        targets_to_build
            .iter()
            .map(|(_, binary_name, _)| binary_name.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        style.reset
    );

    if args.clean {
        println!("{}Cleaning build artifacts...{}", style.yellow, style.reset);
        let targets = APP_BUNDLE_RESOURCE_BINARIES.to_vec();
        clean_old_binaries(&project_root, &targets, style)?;
        for (project_dir, _, _) in &targets_to_build {
            let pkg = project_dir.strip_prefix("crates/").unwrap_or(project_dir);
            let _ = Command::new("cargo")
                .args(["clean", "-p", pkg, "--release"])
                .current_dir(&project_root)
                .status();
        }
        let _ = Command::new("cargo")
            .args(["clean", "-p", "foundation", "--release"])
            .current_dir(&project_root)
            .status();
        println!();
        clean_with_kondo(&project_root, style)?;
    }

    let mut rebuilt = 0;
    let mut skipped = 0;
    let mut failed = 0;

    let mut pending_projects = Vec::new();
    for (project_dir, binary_name, build_all_bins) in &targets_to_build {
        let (action, reason) =
            decide_build_action(&project_root, project_dir, binary_name, args.force);

        if action == "skip" {
            println!(
                "[OK] {}{}{} {}(up-to-date){}",
                style.bold, binary_name, style.reset, style.dim, style.reset
            );
            skipped += 1;
        } else {
            println!(
                "[BUILD] {}{}{} {}({}){}",
                style.bold, binary_name, style.reset, style.dim, reason, style.reset
            );
            pending_projects.push((*project_dir, binary_name.clone(), *build_all_bins));
        }
    }

    if pending_projects.len() > 1
        && pending_projects
            .iter()
            .all(|(_, _, build_all_bins)| !build_all_bins)
    {
        if build_workspace_projects(&project_root, &pending_projects, &args, style)? {
            for (_, binary_name, _) in &pending_projects {
                println!(
                    "[OK] {}{}{} - compiled",
                    style.bold, binary_name, style.reset
                );
            }
            rebuilt += pending_projects.len();
        } else {
            failed += pending_projects.len();
        }
    } else {
        for (project_dir, binary_name, build_all_bins) in &pending_projects {
            if build_project(
                &project_root,
                project_dir,
                binary_name,
                *build_all_bins,
                &args,
                style,
            )? {
                println!(
                    "[OK] {}{}{} - compiled",
                    style.bold, binary_name, style.reset
                );
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

    // Build the GUI only when its own Vue/native-host inputs changed. Running it after
    // native compilation ensures the generated app receives the current bundled
    // binaries during the following sync step.
    if (args.gui || args.all) && !args.rust_only {
        if args.force || gui_needs_rebuild(&project_root) {
            build_and_sync_gui(&project_root, &style)?;
        } else {
            println!("[OK] GUI bundle up-to-date (skipped)");
        }
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
        for bin in APP_BUNDLE_RESOURCE_BINARIES {
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

    sync_app_bundle(&project_root, &style, args.force)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decide_build_action_force() {
        let tempdir = tempfile::tempdir().unwrap();
        // Since we pass force = true, decide_build_action should always return
        // ("rebuild", "force")
        let (action, reason) = decide_build_action(tempdir.path(), "crates/img", "img", true);
        assert_eq!(action, "rebuild");
        assert_eq!(reason, "force");
    }

    #[test]
    fn test_decide_build_action_missing() {
        let tempdir = tempfile::tempdir().unwrap();
        // The binary doesn't exist, so decide_build_action should return ("rebuild",
        // "binary-missing")
        let (action, reason) = decide_build_action(tempdir.path(), "crates/img", "img", false);
        assert_eq!(action, "rebuild");
        assert_eq!(reason, "binary-missing");
    }

    #[test]
    fn test_img_source_inputs_include_foundation_dependency() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path();
        let binary = root.join("target/release/img");
        fs::create_dir_all(binary.parent().unwrap())?;
        fs::write(&binary, b"img")?;

        std::thread::sleep(std::time::Duration::from_millis(20));
        let dependency = root.join("crates/foundation/src/lib.rs");
        fs::create_dir_all(dependency.parent().unwrap())?;
        fs::write(dependency, "pub fn changed() {}")?;

        let (action, reason) = decide_build_action(root, "crates/img", "img", false);
        assert_eq!((action, reason), ("rebuild", "source-newer"));
        Ok(())
    }

    #[test]
    fn test_img_source_inputs_exclude_dev_only_vid_dependency() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path();
        let binary = root.join("target/release/img");
        fs::create_dir_all(binary.parent().unwrap())?;
        fs::write(&binary, b"img")?;

        std::thread::sleep(std::time::Duration::from_millis(20));
        let dev_dependency = root.join("crates/vid/src/lib.rs");
        fs::create_dir_all(dev_dependency.parent().unwrap())?;
        fs::write(dev_dependency, "pub fn changed() {}")?;

        let (action, reason) = decide_build_action(root, "crates/img", "img", false);
        assert_eq!((action, reason), ("skip", ""));
        Ok(())
    }

    #[test]
    fn test_source_scan_skips_node_modules() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path();
        let binary = root.join("target/release/verify");
        fs::create_dir_all(binary.parent().unwrap())?;
        fs::write(&binary, b"verify")?;

        std::thread::sleep(std::time::Duration::from_millis(20));
        let ignored = root.join("crates/gui/node_modules/pkg/index.js");
        fs::create_dir_all(ignored.parent().unwrap())?;
        fs::write(ignored, "export default 'ignored';")?;

        let (action, reason) = decide_build_action(root, "crates/dev", "verify", false);
        assert_eq!((action, reason), ("skip", ""));
        Ok(())
    }

    #[test]
    fn test_dev_binary_source_scan_ignores_unrelated_bins() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path();
        let binary = root.join("target/release/drag_and_drop_processor");
        fs::create_dir_all(binary.parent().unwrap())?;
        fs::write(&binary, b"launcher")?;

        std::thread::sleep(std::time::Duration::from_millis(20));
        let unrelated_bin = root.join("crates/dev/src/bin/verify.rs");
        fs::create_dir_all(unrelated_bin.parent().unwrap())?;
        fs::write(unrelated_bin, "fn main() {}")?;

        let (action, reason) =
            decide_build_action(root, "crates/dev", "drag_and_drop_processor", false);
        assert_eq!((action, reason), ("skip", ""));
        Ok(())
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
    fn test_default_app_bundle_codesign_identity_is_stable() {
        assert_eq!(APP_BUNDLE_CODESIGN_IDENTITY, "MFB-Dev-Signing");
        assert_ne!(APP_BUNDLE_CODESIGN_IDENTITY, "-");
    }

    #[test]
    fn test_app_bundle_resource_binaries_include_terminal_processor() {
        assert!(APP_BUNDLE_RESOURCE_BINARIES.contains(&"drag_and_drop_processor"));
    }

    #[test]
    fn test_bundle_file_needs_sync_detects_missing_and_current_files() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let src = tempdir.path().join("source");
        let dest = tempdir.path().join("dest");
        fs::write(&src, b"compiled binary")?;

        assert!(bundle_file_needs_sync(&src, &dest));
        fs::copy(&src, &dest)?;
        assert!(!bundle_file_needs_sync(&src, &dest));

        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&src, b"new compiled binary")?;
        assert!(bundle_file_needs_sync(&src, &dest));
        Ok(())
    }

    #[test]
    fn test_bundle_file_needs_sync_ignores_code_signature_size_delta() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let src = tempdir.path().join("source");
        let dest = tempdir.path().join("dest");
        fs::write(&src, b"compiled binary")?;
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&dest, b"compiled binary plus code signature")?;

        assert!(!bundle_file_needs_sync(&src, &dest));
        Ok(())
    }

    #[test]
    fn test_native_app_bundle_path_matches_workspace_root_target() {
        let project_root = Path::new("/tmp/mfb");
        assert_eq!(
            native_app_bundle_path(project_root),
            Path::new("/tmp/mfb")
                .join("target")
                .join("release")
                .join("bundle")
                .join("macos")
                .join("Modern Format Boost.app")
        );
    }

    #[test]
    fn test_vue_update_scripts_validate_dependency_updates() {
        assert_eq!(vue_update_script_names(), &["deps:update", "deps:check"]);
    }

    #[test]
    fn gui_rebuild_ignores_unrelated_dev_binary_sources() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let root = tempdir.path();
        let vue_source = root.join("crates/gui/src/App.vue");
        fs::create_dir_all(vue_source.parent().unwrap())?;
        fs::write(&vue_source, "<template><main /></template>")?;

        std::thread::sleep(std::time::Duration::from_millis(20));
        let bundle_binary = native_app_bundle_path(root)
            .join("Contents")
            .join("MacOS")
            .join("Modern Format Boost");
        fs::create_dir_all(bundle_binary.parent().unwrap())?;
        fs::write(&bundle_binary, "app")?;
        assert!(!gui_needs_rebuild(root));

        std::thread::sleep(std::time::Duration::from_millis(20));
        let dev_binary = root.join("crates/dev/src/bin/icloud_import.rs");
        fs::create_dir_all(dev_binary.parent().unwrap())?;
        fs::write(&dev_binary, "fn main() {}")?;
        assert!(!gui_needs_rebuild(root));

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&vue_source, "<template><main>updated</main></template>")?;
        assert!(gui_needs_rebuild(root));
        Ok(())
    }

    #[test]
    fn test_project_process_detection_requires_project_scope_for_generic_tools() {
        let project_root = Path::new("/work/modern_format_boost");

        assert!(should_terminate_process_identity(
            "node",
            "/work/modern_format_boost/crates/gui/node_modules/.bin/vite",
            command_mentions_project(
                "/work/modern_format_boost/crates/gui/node_modules/.bin/vite",
                project_root
            ),
        ));
        assert!(!should_terminate_process_identity(
            "node",
            "/other/project/node_modules/.bin/vite",
            command_mentions_project("/other/project/node_modules/.bin/vite", project_root),
        ));
        assert!(!should_terminate_process_identity("img", "", false));
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

    #[test]
    fn test_bin_flag_maps_img_to_crates_img() {
        assert_eq!(bin_name_to_crate_dir("img").unwrap(), "crates/img");
    }

    #[test]
    fn test_bin_flag_maps_vid_to_crates_vid() {
        assert_eq!(bin_name_to_crate_dir("vid").unwrap(), "crates/vid");
    }

    #[test]
    fn test_bin_flag_maps_dev_binaries_to_crates_dev() {
        for bin in &[
            "verify",
            "cache_cleaner",
            "database_manager",
            "drag_and_drop_processor",
        ] {
            assert_eq!(
                bin_name_to_crate_dir(bin).unwrap(),
                "crates/dev",
                "binary '{bin}' should map to crates/dev"
            );
        }
    }

    #[test]
    fn test_bin_flag_rejects_unknown_binary() {
        assert!(
            bin_name_to_crate_dir("not_a_real_binary").is_err(),
            "unknown binary should return Err"
        );
    }
}
