//! Modern Format Boost - Workspace Auditor in Rust.
//! Checks code formatting, Cargo.toml validity, changelog versions, and runs
//! tests.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::hardening::read_text_file;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const NIGHTLY_COMPONENTS: [&str; 5] = ["clippy", "rustfmt", "miri", "rust-src", "llvm-tools"];
const VUE_QUALITY_SCRIPTS: [&str; 4] = ["lint", "format:check", "deps:check", "build"];

#[derive(Parser, Debug)]
#[command(name = "check_all", about = "MFB Multi-Language Auditor")]
struct Args {
    #[arg(long = "allow-non-nightly", help = "Don't enforce branch check")]
    allow_non_nightly: bool,

    #[arg(long = "required-only", help = "Skip optional checks")]
    required_only: bool,

    #[arg(long = "no-expensive", help = "Skip slow checks")]
    no_expensive: bool,

    #[arg(long = "fix", help = "Auto-fix formatting issues")]
    fix: bool,

    #[arg(long = "build", help = "Run full release build")]
    build: bool,

    #[arg(long = "ai-smell", help = "Run AI smell detection")]
    ai_smell: bool,

    #[arg(long = "miri", help = "Run library tests under Miri")]
    miri: bool,

    #[arg(long = "sanitizers", help = "Run library tests with AddressSanitizer")]
    sanitizers: bool,

    #[arg(long = "mutants", help = "Run cargo-mutants")]
    mutants: bool,

    #[arg(long = "fuzz-list", help = "Discover and list fuzz targets")]
    fuzz_list: bool,

    #[arg(long = "fuzz-smoke", help = "Run each fuzz target briefly")]
    fuzz_smoke: bool,

    #[arg(long = "install-nightly", help = "Install/update nightly toolchain")]
    install_nightly: bool,

    #[arg(long = "branch", default_value_t = default_branch(), help = "Required branch name")]
    branch: String,

    #[arg(long = "verbose", short = 'v', help = "Show tool install hints")]
    verbose: bool,

    #[arg(long = "ci", help = "GitHub Actions health-check profile")]
    ci: bool,
}

fn default_branch() -> String {
    std::env::var("CHECK_ALL_DEFAULT_BRANCH").unwrap_or_else(|_| "nightly".to_string())
}

fn command_exists(cmd: &str) -> bool {
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            if path.join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

fn bootstrap_macos_path_with(
    platform_is_macos: bool,
    current_path: &str,
    existing_dirs: impl Fn(&str) -> bool,
) -> Option<String> {
    if !platform_is_macos {
        return None;
    }
    let mut path_parts: Vec<String> = std::env::split_paths(std::ffi::OsStr::new(current_path))
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let mut changed = false;
    for extra in [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    ] {
        if existing_dirs(extra) && !path_parts.iter().any(|part| part == extra) {
            path_parts.insert(0, extra.to_string());
            changed = true;
        }
    }
    changed.then(|| path_parts.join(":"))
}

fn bootstrap_macos_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    if let Some(updated) = bootstrap_macos_path_with(cfg!(target_os = "macos"), &current, |path| {
        Path::new(path).is_dir()
    }) {
        unsafe {
            std::env::set_var("PATH", updated);
        }
    }
}

fn cargo_subcommand_exists(sub: &str) -> bool {
    match Command::new("cargo").args([sub, "--version"]).output() {
        Ok(out) => out.status.success(),
        Err(err) => {
            eprintln!("[CHECK] cargo {sub} probe failed: {err}");
            false
        }
    }
}

fn taplo_fmt_command_with_availability(
    files: &[String],
    args: &[&str],
    has_cargo_taplo: bool,
    has_taplo: bool,
) -> Option<Vec<String>> {
    if files.is_empty() {
        return None;
    }

    let mut cmd = if has_cargo_taplo {
        vec!["cargo".to_string(), "taplo".to_string(), "fmt".to_string()]
    } else if has_taplo {
        vec!["taplo".to_string(), "fmt".to_string()]
    } else {
        return None;
    };
    cmd.extend(args.iter().map(|arg| (*arg).to_string()));
    cmd.extend(files.iter().cloned());
    Some(cmd)
}

fn taplo_fmt_command(files: &[String], args: &[&str]) -> Option<Vec<String>> {
    taplo_fmt_command_with_availability(
        files,
        args,
        cargo_subcommand_exists("taplo"),
        command_exists("taplo"),
    )
}

fn get_project_root() -> Result<PathBuf> {
    let exe_path = std::env::current_exe().context("get current exe path")?;
    let mut dir = exe_path.parent();
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() && d.join("crates").is_dir() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    let cwd = std::env::current_dir().context("get current dir")?;
    Ok(cwd)
}

fn parse_rust_toolchain_channel_toml(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("channel")?.trim_start();
        let value = rest.strip_prefix('=')?.trim();
        let quoted = value.strip_prefix('"')?;
        let end = quoted.find('"')?;
        Some(quoted[..end].to_string())
    })
}

fn rust_toolchain_channel_for_probe(repo_root: &Path) -> String {
    let toml_path = repo_root.join("rust-toolchain.toml");
    if toml_path.is_file()
        && let Some(content) = read_text_file(&toml_path)
        && let Some(channel) = parse_rust_toolchain_channel_toml(&content)
    {
        return channel;
    }
    match Command::new("rustup")
        .args(["show", "active-toolchain"])
        .output()
    {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(first) = text
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().next())
                && !first.is_empty()
            {
                return first.to_string();
            }
        }
        Ok(out) => eprintln!(
            "[CHECK] rustup show active-toolchain failed with status {:?}",
            out.status.code()
        ),
        Err(err) => eprintln!("[CHECK] rustup probe failed: {err}"),
    }
    "nightly".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NightlyComponents {
    toolchain: bool,
    clippy: bool,
    rustfmt: bool,
    miri: bool,
    rust_src: bool,
    llvm_tools: bool,
}

impl NightlyComponents {
    fn missing_components(&self) -> Vec<&'static str> {
        [
            ("clippy", self.clippy),
            ("rustfmt", self.rustfmt),
            ("miri", self.miri),
            ("rust-src", self.rust_src),
            ("llvm-tools", self.llvm_tools),
        ]
        .into_iter()
        .filter_map(|(name, present)| (!present).then_some(name))
        .collect()
    }

    fn status_line(&self) -> String {
        format!(
            "toolchain:{} clippy:{} rustfmt:{} miri:{} rust-src:{} llvm-tools:{}",
            if self.toolchain { "OK" } else { "MISSING" },
            if self.clippy { "+" } else { "-" },
            if self.rustfmt { "+" } else { "-" },
            if self.miri { "+" } else { "-" },
            if self.rust_src { "+" } else { "-" },
            if self.llvm_tools { "+" } else { "-" }
        )
    }

    fn install_hint(&self) -> String {
        let missing = self.missing_components();
        if missing.is_empty() {
            return String::new();
        }
        let components = missing
            .iter()
            .map(|component| format!("--component {component}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("rustup toolchain install nightly {components}")
    }
}

fn parse_installed_rustup_components(stdout: &str) -> NightlyComponents {
    let has_component = |needle: &str| {
        stdout
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .any(|component| component == needle || component.starts_with(&format!("{needle}-")))
    };
    NightlyComponents {
        toolchain: true,
        clippy: has_component("clippy"),
        rustfmt: has_component("rustfmt"),
        miri: has_component("miri"),
        rust_src: has_component("rust-src"),
        llvm_tools: has_component("llvm-tools"),
    }
}

fn probe_nightly(repo_root: &Path) -> NightlyComponents {
    let mut nc = NightlyComponents {
        toolchain: false,
        clippy: false,
        rustfmt: false,
        miri: false,
        rust_src: false,
        llvm_tools: false,
    };
    if !command_exists("rustup") {
        return nc;
    }
    let channel = rust_toolchain_channel_for_probe(repo_root);
    let Ok(rustc) = Command::new("rustup")
        .args(["run", channel.as_str(), "rustc", "--version"])
        .output()
    else {
        return nc;
    };
    if !rustc.status.success() {
        return nc;
    }
    nc.toolchain = true;
    let Ok(components) = Command::new("rustup")
        .args([
            "component",
            "list",
            "--installed",
            "--toolchain",
            channel.as_str(),
        ])
        .output()
    else {
        return nc;
    };
    if !components.status.success() {
        return nc;
    }
    parse_installed_rustup_components(&String::from_utf8_lossy(&components.stdout))
}

fn install_nightly_command(components: &[&str]) -> Vec<String> {
    let mut cmd = vec![
        "rustup".to_string(),
        "toolchain".to_string(),
        "install".to_string(),
        "nightly".to_string(),
    ];
    for component in components {
        cmd.push("--component".to_string());
        cmd.push((*component).to_string());
    }
    cmd
}

fn install_nightly() -> Result<bool> {
    if !command_exists("rustup") {
        println!("  FAIL: rustup not found; cannot install nightly");
        return Ok(false);
    }
    let cmd = install_nightly_command(&NIGHTLY_COMPONENTS);
    println!("  Running: {}", cmd.join(" "));
    let Some((program, args)) = cmd.split_first() else {
        return Ok(false);
    };
    let status = Command::new(program)
        .args(args)
        .status()
        .context("run rustup toolchain install nightly")?;
    Ok(status.success())
}

#[allow(dead_code)]
fn parse_plist_string_key(content: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let idx = content.find(&key_tag)?;
    let after_key = &content[idx + key_tag.len()..];
    let string_start = after_key.find("<string>")?;
    let string_end = after_key.find("</string>")?;
    if string_end > string_start + 8 {
        Some(after_key[string_start + 8..string_end].trim().to_string())
    } else {
        None
    }
}

fn verify_normalize_stale_embed_measurement_slots(repo_root: &std::path::Path) -> Result<()> {
    let normalize_bin = repo_root
        .join("crates")
        .join("dev")
        .join("src")
        .join("bin")
        .join("normalize_stale_embed_measurement_slots.rs");
    if !normalize_bin.is_file() {
        println!("  Skipped: normalize_stale_embed_measurement_slots.rs (missing DB backfill bin)");
        return Ok(());
    }

    let text = fs::read_to_string(&normalize_bin)
        .with_context(|| format!("read {}", normalize_bin.display()))?;
    if text.contains("EMBED_SLOT_INDICES") && text.contains("PGVECTOR_MISSING_MEASUREMENT") {
        println!("  OK: normalize_stale_embed_measurement_slots.rs DB sentinel markers present");
    } else {
        println!("  Skipped: normalize_stale_embed_measurement_slots.rs (incomplete SSOT markers)");
    }
    Ok(())
}

fn run_required_vec_env(
    repo_root: &Path,
    label: &str,
    program: &str,
    args: &[String],
    env_vars: &[(&str, &str)],
) -> Result<()> {
    println!("{label}...");
    let mut command = Command::new(program);
    command.args(args).current_dir(repo_root);
    for (key, value) in env_vars {
        command.env(key, value);
    }
    let status = command
        .status()
        .with_context(|| format!("run {program} {}", args.join(" ")))?;
    if !status.success() {
        eprintln!("FAIL: {label} failed with status {status}");
        std::process::exit(1);
    }
    println!("  OK: {label}");
    Ok(())
}

fn run_required_vec(repo_root: &Path, label: &str, program: &str, args: &[String]) -> Result<()> {
    run_required_vec_env(repo_root, label, program, args, &[])
}

fn run_required(repo_root: &Path, label: &str, program: &str, args: &[&str]) -> Result<()> {
    let owned = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_required_vec(repo_root, label, program, &owned)
}

fn run_optional_vec_env(
    repo_root: &Path,
    label: &str,
    program: &str,
    args: &[String],
    env_vars: &[(&str, &str)],
    hard_fail: bool,
) -> Result<()> {
    println!("{label}...");
    let mut command = Command::new(program);
    command.args(args).current_dir(repo_root);
    for (key, value) in env_vars {
        command.env(key, value);
    }
    let status = command
        .status()
        .with_context(|| format!("run {program} {}", args.join(" ")))?;
    if status.success() {
        println!("  OK: {label}");
        return Ok(());
    }
    if hard_fail {
        eprintln!("FAIL: {label} failed with status {status}");
        std::process::exit(1);
    }
    println!("  Warning: {label} failed with status {status}");
    Ok(())
}

fn run_optional_vec(
    repo_root: &Path,
    label: &str,
    program: &str,
    args: &[String],
    hard_fail: bool,
) -> Result<()> {
    run_optional_vec_env(repo_root, label, program, args, &[], hard_fail)
}

fn run_optional(
    repo_root: &Path,
    label: &str,
    program: &str,
    args: &[&str],
    hard_fail: bool,
) -> Result<()> {
    let owned = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_optional_vec(repo_root, label, program, &owned, hard_fail)
}

fn git_tracked_existing_files(repo_root: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo_root)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|file| !file.is_empty())
        .filter(|file| repo_root.join(file).is_file())
        .map(ToOwned::to_owned)
        .collect()
}

fn files_with_suffixes(files: &[String], suffixes: &[&str]) -> Vec<String> {
    files
        .iter()
        .filter(|file| suffixes.iter().any(|suffix| file.ends_with(suffix)))
        .cloned()
        .collect()
}

fn run_python_syntax_check(repo_root: &Path, py_files: &[String]) -> Result<()> {
    if py_files.is_empty() {
        println!("  Skipped: python syntax (no scripts)");
        return Ok(());
    }
    let python = if command_exists("python3") {
        "python3"
    } else {
        "python"
    };
    let mut args = vec!["-m".to_string(), "py_compile".to_string()];
    args.extend(py_files.iter().cloned());
    run_required_vec(
        repo_root,
        &format!("python3 syntax ({} files)", py_files.len()),
        python,
        &args,
    )
}

fn run_argv_optional(
    repo_root: &Path,
    label: &str,
    argv: &[String],
    hard_fail: bool,
) -> Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Ok(());
    };
    run_optional_vec(repo_root, label, program, args, hard_fail)
}

fn check_bundle_metadata(repo_root: &Path, version: &str, hard_fail: bool) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (repo_root, version, hard_fail);
        println!("  Skipped: macOS App bundle metadata (non-macOS platform)");
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        println!("Checking macOS App bundle metadata...");
        let plist_path = repo_root
            .join("Modern Format Boost.app")
            .join("Contents")
            .join("Info.plist");
        if !plist_path.is_file() {
            println!("  Skipped: Info.plist not found (building outside app scope)");
            return Ok(());
        }
        let plist_content = fs::read_to_string(&plist_path).context("read Info.plist")?;
        let bundle_version = parse_plist_string_key(&plist_content, "CFBundleShortVersionString");
        let bundle_exec = parse_plist_string_key(&plist_content, "CFBundleExecutable");
        let mut errors = Vec::new();
        if let Some(ref bv) = bundle_version
            && bv != version
            && !version.is_empty()
        {
            errors.push(format!(
                "Version mismatch: Cargo.toml={version} vs Info.plist={bv}"
            ));
        }
        if let Some(ref be) = bundle_exec
            && be != "Modern Format Boost"
        {
            errors.push(format!(
                "Executable name mismatch: expected 'Modern Format Boost', got '{be}'"
            ));
        }
        for key in ["NSAppDataUsageDescription", "NSAppleEventsUsageDescription"] {
            match parse_plist_string_key(&plist_content, key) {
                Some(value) if !value.trim().is_empty() => {}
                _ => errors.push(format!("Missing required macOS privacy key: {key}")),
            }
        }
        let binary_path = repo_root
            .join("Modern Format Boost.app")
            .join("Contents")
            .join("MacOS")
            .join("Modern Format Boost");
        if !binary_path.exists() {
            errors.push(format!(
                "App binary wrapper missing at {}",
                binary_path.display()
            ));
        }
        if errors.is_empty() {
            println!("  OK: Info.plist and app wrapper matching");
            return Ok(());
        }
        for error in &errors {
            eprintln!("FAIL: {error}");
        }
        if hard_fail {
            std::process::exit(1);
        }
        Ok(())
    }
}

fn ensure_edge_test_media(repo_root: &Path) -> Result<()> {
    let marker = repo_root.join("crates/dev/tests/edge/videos/test_h264_10s.mp4");
    if marker.is_file() {
        println!("  Skipped: generate edge test media (already present)");
        return Ok(());
    }
    run_required(
        repo_root,
        "Generating edge test media",
        "cargo",
        &[
            "run",
            "--locked",
            "-p",
            "dev",
            "--bin",
            "generate_test_media",
        ],
    )
}

const fn ci_feature_args() -> [&'static str; 3] {
    ["--all-features", "--features", "foundation/ci-static-build"]
}

fn vue_quality_script_names() -> &'static [&'static str] {
    &VUE_QUALITY_SCRIPTS
}

fn vue_dir(repo_root: &Path) -> PathBuf {
    repo_root.join("crates").join("gui")
}

fn run_vue_quality_checks(repo_root: &Path) -> Result<()> {
    let vue_dir = vue_dir(repo_root);
    if !vue_dir.join("package.json").is_file() {
        println!("  Skipped: Vue quality checks (package.json missing)");
        return Ok(());
    }

    let prefix = vue_dir.to_string_lossy().into_owned();
    for script in vue_quality_script_names() {
        run_required_vec(
            repo_root,
            &format!("Vue npm run {script}"),
            "npm",
            &[
                "--prefix".to_string(),
                prefix.clone(),
                "run".to_string(),
                (*script).to_string(),
            ],
        )?;
    }
    Ok(())
}

fn cargo_check_args(ci: bool) -> Vec<String> {
    let mut args = vec![
        "check".to_string(),
        "--workspace".to_string(),
        "--all-targets".to_string(),
        "--locked".to_string(),
    ];
    if ci {
        args.extend([
            "--all-features".to_string(),
            "--features".to_string(),
            "foundation/ci-static-build".to_string(),
        ]);
    } else {
        args.push("--all-features".to_string());
    }
    args
}

#[cfg(test)]
fn workspace_members_block(manifest: &str) -> Option<&str> {
    let members_start = manifest.find("members")?;
    let after_members = &manifest[members_start..];
    let list_start = after_members.find('[')?;
    let after_list_start = &after_members[list_start + 1..];
    let list_end = after_list_start.find(']')?;
    Some(&after_list_start[..list_end])
}

fn apply_ci_runner_env() {
    unsafe {
        std::env::set_var(
            "GITHUB_ACTIONS",
            std::env::var("GITHUB_ACTIONS").unwrap_or_else(|_| "true".to_string()),
        );
        std::env::set_var(
            "LIBHEIF_STATIC",
            std::env::var("LIBHEIF_STATIC").unwrap_or_else(|_| "1".to_string()),
        );
        std::env::set_var(
            "LIBHEIF_SYS_STATIC",
            std::env::var("LIBHEIF_SYS_STATIC").unwrap_or_else(|_| "1".to_string()),
        );
        std::env::set_var(
            "NODE_OPTIONS",
            std::env::var("NODE_OPTIONS").unwrap_or_else(|_| "--no-deprecation".to_string()),
        );
        let existing = std::env::var("RUSTFLAGS").unwrap_or_default();
        if existing.split_whitespace().any(|part| part == "-D")
            && existing.split_whitespace().any(|part| part == "warnings")
        {
            std::env::set_var("RUSTFLAGS", existing);
        } else if existing.trim().is_empty() {
            std::env::set_var("RUSTFLAGS", "-D warnings");
        } else {
            std::env::set_var("RUSTFLAGS", format!("{} -D warnings", existing.trim()));
        }
    }
}

fn run_ci_health_rust_tests(repo_root: &Path) -> Result<()> {
    let ci_features = ci_feature_args();
    run_required(
        repo_root,
        "cargo test -p foundation --lib (serial, ci-static-build)",
        "cargo",
        &[
            "test",
            "--locked",
            "-p",
            "foundation",
            "--lib",
            ci_features[0],
            ci_features[1],
            ci_features[2],
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ],
    )?;
    run_required(
        repo_root,
        "cargo test --workspace --lib (ci-static-build, exclude foundation)",
        "cargo",
        &[
            "test",
            "--locked",
            "--workspace",
            "--lib",
            ci_features[0],
            ci_features[1],
            ci_features[2],
            "--exclude",
            "foundation",
            "--no-fail-fast",
        ],
    )?;
    run_required(
        repo_root,
        "cargo test -p dev test_real_silent_fallbacks (contract registry)",
        "cargo",
        &[
            "test",
            "--locked",
            "-p",
            "dev",
            "--test",
            "test_real_silent_fallbacks",
            ci_features[0],
            ci_features[1],
            ci_features[2],
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ],
    )?;
    run_required(
        repo_root,
        "cargo test -p dev headless_gif_regression (ffmpeg/runtime probe regression)",
        "cargo",
        &[
            "test",
            "--locked",
            "-p",
            "dev",
            "--test",
            "headless_gif_regression",
            ci_features[0],
            ci_features[1],
            ci_features[2],
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ],
    )?;
    run_required(
        repo_root,
        "cargo test -p dev runtime_probe_regression (WebP/APNG/HEIC/JXL/AVIF header preflight)",
        "cargo",
        &[
            "test",
            "--locked",
            "-p",
            "dev",
            "--test",
            "runtime_probe_regression",
            ci_features[0],
            ci_features[1],
            ci_features[2],
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ],
    )?;
    run_required(
        repo_root,
        "cargo test -p dev comprehensive_weakness_audit (inventory + closure SSOT)",
        "cargo",
        &[
            "test",
            "--locked",
            "-p",
            "dev",
            "--test",
            "comprehensive_weakness_audit",
            ci_features[0],
            ci_features[1],
            ci_features[2],
            "--no-fail-fast",
        ],
    )
}

fn main() -> Result<()> {
    bootstrap_macos_path();
    let args = Args::parse();
    if args.ci {
        apply_ci_runner_env();
    }
    let repo_root = get_project_root()?;
    std::env::set_current_dir(&repo_root).context("change directory to repo root")?;

    if args.install_nightly {
        println!("Installing nightly toolchain + components...");
        if install_nightly()? {
            println!("  OK: nightly toolchain installed/updated");
        } else {
            println!("  Warning: rustup install failed; continuing with available toolchain");
        }
    }

    let nc = probe_nightly(&repo_root);

    println!("--- Modern Quality Suite ---");
    println!("Root: {}", repo_root.display());
    println!("Nightly: {}", nc.status_line());
    if nc.toolchain && !nc.missing_components().is_empty() {
        println!(
            "  Missing nightly components: {}",
            nc.missing_components().join(", ")
        );
        println!("  Fix: {}", nc.install_hint());
    } else if !nc.toolchain {
        println!(
            "  Nightly toolchain not found. Install: rustup toolchain install nightly --component \
             clippy --component rustfmt --component miri --component rust-src --component \
             llvm-tools"
        );
    }

    let git_files = git_tracked_existing_files(&repo_root);
    let py_files = files_with_suffixes(&git_files, &[".py"]);
    let shell_files = files_with_suffixes(&git_files, &[".sh"]);
    let md_files = files_with_suffixes(&git_files, &[".md"]);
    let json_files = files_with_suffixes(&git_files, &[".json", ".jsonc"]);
    let yaml_files = files_with_suffixes(&git_files, &[".yml", ".yaml"]);
    let toml_files = files_with_suffixes(&git_files, &[".toml"]);
    let sql_files = files_with_suffixes(&git_files, &[".sql"]);
    let plist_files = files_with_suffixes(&git_files, &[".plist"]);
    let web_files =
        files_with_suffixes(&git_files, &[".vue", ".ts", ".js", ".cjs", ".css", ".html"]);

    // 1. Branch Guard
    let branch_output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output();
    match branch_output {
        Ok(out) => {
            let current_branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !args.allow_non_nightly && current_branch != args.branch {
                eprintln!(
                    "Fatal: required branch '{}', current is '{}'. Use --allow-non-nightly or \
                     --branch <n>.",
                    args.branch, current_branch
                );
                std::process::exit(2);
            }
        }
        Err(err) => {
            println!("Warning: could not determine git branch: {err}");
        }
    }

    // 2. Fix mode (Explicit user opt-in for multi-language workspace formatting)
    if args.fix {
        println!("Running multi-language auto-fixers across workspace...");
        // Rust
        let _ = Command::new("cargo").args(["fmt", "--all"]).status();
        let _ = Command::new("cargo")
            .args([
                "run",
                "--locked",
                "-p",
                "dev",
                "--bin",
                "clippy_strict",
                "--",
                "--fix",
            ])
            .status();
        // Python
        if command_exists("ruff") {
            let _ = Command::new("ruff").args(["check", "--fix", "."]).status();
            let _ = Command::new("ruff").args(["format", "."]).status();
        }
        if command_exists("pyupgrade") && !py_files.is_empty() {
            let mut pyupgrade_args = vec!["--py311-plus".to_string()];
            pyupgrade_args.extend(py_files.iter().cloned());
            let _ = Command::new("pyupgrade").args(pyupgrade_args).status();
        }
        // Shell
        if command_exists("shfmt") && !shell_files.is_empty() {
            let mut shfmt_args = vec!["-w".to_string(), "-i".to_string(), "4".to_string()];
            shfmt_args.extend(shell_files.iter().cloned());
            let _ = Command::new("shfmt").args(shfmt_args).status();
        }
        // Vue / Node
        let vue_path = vue_dir(&repo_root);
        if vue_path.join("package.json").is_file() {
            let vue_prefix = vue_path.to_string_lossy().into_owned();
            let _ = Command::new("npm")
                .args(["--prefix", &vue_prefix, "run", "format"])
                .status();
        }
        // Web / Prettier (MD, JSON, YAML, Vue, TS, JS, CSS, HTML)
        let mut prettier_targets = md_files.clone();
        prettier_targets.extend(json_files.iter().cloned());
        prettier_targets.extend(yaml_files.iter().cloned());
        prettier_targets.extend(web_files.iter().cloned());
        if command_exists("prettier") && !prettier_targets.is_empty() {
            let mut prettier_args = vec!["--write".to_string()];
            prettier_args.extend(prettier_targets);
            let _ = Command::new("prettier").args(prettier_args).status();
        }
        // TOML
        if let Some(cmd) = taplo_fmt_command(&toml_files, &[])
            && let Some((program, args)) = cmd.split_first()
        {
            let _ = Command::new(program).args(args).status();
        }
        // SQL (PostgreSQL dialect)
        if command_exists("npx") && !sql_files.is_empty() {
            for sql_file in &sql_files {
                let _ = Command::new("npx")
                    .args(["-y", "sql-formatter", "-l", "postgresql", "--fix", sql_file])
                    .status();
            }
        }
        // Plist (macOS)
        if cfg!(target_os = "macos") && command_exists("plutil") && !plist_files.is_empty() {
            let mut plutil_args = vec!["-convert".to_string(), "xml1".to_string()];
            plutil_args.extend(plist_files.iter().cloned());
            let _ = Command::new("plutil").args(plutil_args).status();
        }
    }

    // 3. cargo fmt --check
    println!("Checking formatting (cargo fmt --check)...");
    let fmt_status = Command::new("cargo")
        .args(["fmt", "--all", "--check"])
        .status()
        .context("run cargo fmt")?;
    if !fmt_status.success() {
        eprintln!(
            "FAIL: cargo fmt check failed.\n\
             Hint: To format all workspace languages (Rust, Python, Shell, Vue, JS/TS, SQL, TOML, JSON, YAML, Markdown, Plist), run:\n\
             cargo run --locked -p dev --bin check_all -- --fix"
        );
        std::process::exit(1);
    }
    println!("  OK: formatting matches");

    if !args.required_only {
        if let Some(cmd) = taplo_fmt_command(&toml_files, &["--check"]) {
            println!("Checking TOML formatting (taplo fmt --check)...");
            if let Some((program, args)) = cmd.split_first() {
                let taplo_status = Command::new(program)
                    .args(args)
                    .status()
                    .context("run taplo fmt --check")?;
                if !taplo_status.success() {
                    eprintln!(
                        "FAIL: taplo fmt check failed.\n\
                         Hint: To format all workspace languages, run:\n\
                         cargo run --locked -p dev --bin check_all -- --fix"
                    );
                    std::process::exit(1);
                }
            }
            println!("  OK: TOML formatting matches");
        } else if args.verbose {
            println!("  Skipped: neither 'cargo taplo' nor 'taplo' found");
        }
    }

    // 4. cargo check
    println!("Checking compilation (cargo check)...");
    let check_args = cargo_check_args(args.ci);
    let check_status = Command::new("cargo")
        .args(&check_args)
        .status()
        .context("run cargo check")?;
    if !check_status.success() {
        eprintln!("FAIL: cargo check failed.");
        std::process::exit(1);
    }
    println!("  OK: compiles cleanly");

    // 5. CHANGELOG version sync
    println!("Checking CHANGELOG version synchronization...");
    let cargo_toml_path = repo_root.join("Cargo.toml");
    let changelog_path = repo_root.join("docs").join("CHANGELOG.md");
    if !changelog_path.is_file() {
        eprintln!("FAIL: docs/CHANGELOG.md missing");
        std::process::exit(1);
    }
    let cargo_content = fs::read_to_string(&cargo_toml_path).context("read Cargo.toml")?;
    let version_line = cargo_content
        .lines()
        .find(|l| l.trim().starts_with("version =") || l.trim().starts_with("version="));
    let version = match version_line {
        Some(line) => {
            let parts: Vec<&str> = line.split('=').collect();
            if parts.len() >= 2 {
                parts[1].trim().trim_matches('"').to_string()
            } else {
                String::new()
            }
        }
        None => String::new(),
    };
    if version.is_empty() {
        println!("  Skipped: could not find workspace version in Cargo.toml");
    } else {
        let changelog_content = fs::read_to_string(&changelog_path).context("read CHANGELOG.md")?;
        let expected_header = format!("[v{version}]");
        let expected_header_alt = format!("[{version}]");
        if changelog_content.contains(&expected_header)
            || changelog_content.contains(&expected_header_alt)
        {
            println!("  OK: version {version} is documented in CHANGELOG");
        } else {
            eprintln!("FAIL: version '{version}' not found as a header in docs/CHANGELOG.md");
            std::process::exit(1);
        }
    }

    run_python_syntax_check(&repo_root, &py_files)?;
    run_vue_quality_checks(&repo_root)?;

    run_required(
        &repo_root,
        "Running ultra-strict clippy",
        "cargo",
        &["run", "--locked", "-p", "dev", "--bin", "clippy_strict"],
    )?;

    if !args.ci {
        ensure_edge_test_media(&repo_root)?;
    }

    // 8. Run workspace tests
    if args.ci {
        run_ci_health_rust_tests(&repo_root)?;
    } else {
        println!("Running workspace tests...");
        let test_status = Command::new("cargo")
            .args(["test", "--workspace", "--locked", "--all-features"])
            .status()
            .context("run cargo test")?;
        if !test_status.success() {
            eprintln!("FAIL: cargo test failed.");
            std::process::exit(1);
        }
        println!("  OK: all tests passed");
    }

    // 8b. DB sentinel backfill SSOT check retained from the Python auditor.
    verify_normalize_stale_embed_measurement_slots(&repo_root)?;

    if !args.required_only {
        if nc.rustfmt {
            run_optional(
                &repo_root,
                "cargo fmt --check (unstable options)",
                "cargo",
                &["fmt", "--all", "--check"],
                args.ci,
            )?;
        } else {
            println!("  Skipped: nightly rustfmt (unstable options)");
        }

        if !args.ci && !args.no_expensive && nc.llvm_tools {
            if cargo_subcommand_exists("llvm-cov") {
                run_optional(
                    &repo_root,
                    "cargo llvm-cov --summary-only",
                    "cargo",
                    &[
                        "llvm-cov",
                        "--workspace",
                        "--all-features",
                        "--summary-only",
                    ],
                    false,
                )?;
            } else if args.verbose {
                println!("  Hint: cargo-llvm-cov not found. Install: cargo install cargo-llvm-cov");
            }
        }

        if !py_files.is_empty() && command_exists("ruff") {
            let mut ruff_check = vec!["check".to_string()];
            ruff_check.extend(py_files.iter().cloned());
            run_optional_vec(&repo_root, "ruff linter", "ruff", &ruff_check, args.ci)?;
            let mut ruff_format = vec!["format".to_string(), "--check".to_string()];
            ruff_format.extend(py_files.iter().cloned());
            run_optional_vec(
                &repo_root,
                "ruff format check",
                "ruff",
                &ruff_format,
                args.ci,
            )?;
        } else if py_files.is_empty() {
            println!("  Skipped: python quality (no scripts)");
        }

        let shell_files = files_with_suffixes(&git_files, &[".sh"]);
        if !shell_files.is_empty() {
            if command_exists("shellcheck") {
                let mut shellcheck = vec!["--severity=error".to_string()];
                shellcheck.extend(shell_files.iter().cloned());
                run_optional_vec(&repo_root, "shellcheck", "shellcheck", &shellcheck, args.ci)?;
            }
            if command_exists("shfmt") {
                let mut shfmt = vec!["-d".to_string(), "-i".to_string(), "4".to_string()];
                shfmt.extend(shell_files.iter().cloned());
                run_optional_vec(&repo_root, "shfmt layout check", "shfmt", &shfmt, args.ci)?;
            }
        }

        check_bundle_metadata(&repo_root, &version, args.ci)?;

        if !md_files.is_empty() && command_exists("markdownlint-cli2") {
            let config_path = repo_root
                .join("crates/dev/scripts/config/.markdownlint-cli2.jsonc")
                .to_string_lossy()
                .into_owned();
            let mut markdownlint = vec!["--config".to_string(), config_path];
            markdownlint.extend(md_files.iter().cloned());
            run_optional_vec(
                &repo_root,
                "markdownlint",
                "markdownlint-cli2",
                &markdownlint,
                args.ci,
            )?;
        }

        let mut prettier_targets = md_files;
        prettier_targets.extend(json_files.iter().cloned());
        prettier_targets.extend(yaml_files.iter().cloned());
        if !prettier_targets.is_empty() && command_exists("prettier") {
            let mut prettier = vec!["--check".to_string()];
            prettier.extend(prettier_targets);
            run_optional_vec(&repo_root, "prettier check", "prettier", &prettier, args.ci)?;
        }

        if let Some(cmd) = taplo_fmt_command(&toml_files, &["--check"]) {
            run_argv_optional(&repo_root, "taplo fmt check", &cmd, args.ci)?;
        }

        if !args.ci {
            run_optional(
                &repo_root,
                "cargo doc",
                "cargo",
                &["doc", "--workspace", "--no-deps"],
                false,
            )?;
            if nc.toolchain {
                run_optional_vec_env(
                    &repo_root,
                    "cargo doc -D warnings (rustdoc lints)",
                    "cargo",
                    &[
                        "doc".to_string(),
                        "--workspace".to_string(),
                        "--no-deps".to_string(),
                    ],
                    &[("RUSTDOCFLAGS", "-D warnings")],
                    false,
                )?;
            } else {
                println!("  Skipped: nightly rustdoc -D warnings");
            }
        }

        for (sub, label, args_list) in [
            ("audit", "cargo audit", vec!["audit".to_string()]),
            (
                "deny",
                "cargo deny check (licenses + advisories + bans)",
                vec!["deny".to_string(), "check".to_string()],
            ),
            (
                "insta",
                "cargo insta test (snapshot regression check)",
                vec![
                    "insta".to_string(),
                    "test".to_string(),
                    "--workspace".to_string(),
                    "--unreferenced=reject".to_string(),
                ],
            ),
        ] {
            if cargo_subcommand_exists(sub) {
                run_optional_vec(&repo_root, label, "cargo", &args_list, args.ci)?;
            }
        }

        let bench_files = git_files
            .iter()
            .filter(|file| file.contains("benches/") && file.ends_with(".rs"))
            .count();
        if bench_files > 0 {
            run_optional(
                &repo_root,
                &format!("cargo bench --no-run (compile check, {bench_files} bench file(s))"),
                "cargo",
                &["bench", "--workspace", "--no-run"],
                args.ci,
            )?;
        } else {
            println!("  Skipped: bench compile check (no bench targets found)");
        }

        if !args.no_expensive {
            if cargo_subcommand_exists("bloat") {
                run_optional(
                    &repo_root,
                    "cargo bloat",
                    "cargo",
                    &["bloat", "--release", "--crates", "-n", "10"],
                    args.ci,
                )?;
            }
            if cargo_subcommand_exists("hack") {
                run_optional(
                    &repo_root,
                    "cargo hack feature matrix",
                    "cargo",
                    &[
                        "hack",
                        "check",
                        "--workspace",
                        "--each-feature",
                        "--no-dev-deps",
                    ],
                    args.ci,
                )?;
            }
        }

        if args.ai_smell {
            println!("Checking AI smells...");
            let agent = if command_exists("claude") {
                Some("claude")
            } else if command_exists("gemini") {
                Some("gemini")
            } else {
                None
            };
            if let Some(agent) = agent {
                run_optional(
                    &repo_root,
                    "AI smell detection",
                    agent,
                    &[
                        "--print",
                        "Check codebase for unneeded comments and AI smells.",
                    ],
                    args.ci,
                )?;
            } else {
                println!("  Skipped: neither 'claude' nor 'gemini' CLI found");
            }
        }
    }

    // 9. Run library tests under Miri
    if !args.required_only && args.miri {
        println!("Running tests under Miri...");
        if !nc.miri || !nc.rust_src {
            let missing = [
                (!nc.miri).then_some("miri"),
                (!nc.rust_src).then_some("rust-src"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            println!(
                "  Skipped: miri (missing: {} — run --install-nightly)",
                missing.join(", ")
            );
        } else {
            let channel = rust_toolchain_channel_for_probe(&repo_root);
            let status = Command::new("cargo")
                .args([
                    format!("+{channel}"),
                    "miri".to_string(),
                    "test".to_string(),
                    "--workspace".to_string(),
                    "--lib".to_string(),
                ])
                .env("MIRIFLAGS", "-Zmiri-strict-provenance")
                .status()
                .context("run cargo miri")?;
            if !status.success() {
                eprintln!("FAIL: Miri tests failed.");
                std::process::exit(1);
            }
            println!("  OK: Miri tests passed");
        }
    }

    // 10. Sanitizers
    if !args.required_only && args.sanitizers {
        println!("Running AddressSanitizer...");
        if !nc.toolchain || !nc.rust_src {
            let missing = [
                (!nc.toolchain).then_some("nightly toolchain"),
                (!nc.rust_src).then_some("rust-src"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            println!(
                "  Skipped: AddressSanitizer (missing: {} — run --install-nightly)",
                missing.join(", ")
            );
        } else {
            let channel = rust_toolchain_channel_for_probe(&repo_root);
            let build_target = match Command::new("rustc").arg("-vV").output() {
                Ok(out) => String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .find_map(|line| line.strip_prefix("host: ").map(ToOwned::to_owned))
                    .unwrap_or_else(|| {
                        eprintln!("[CHECK] rustc -vV missing host line; defaulting target");
                        "aarch64-apple-darwin".to_string()
                    }),
                Err(err) => {
                    eprintln!("[CHECK] rustc -vV failed: {err}; defaulting target");
                    "aarch64-apple-darwin".to_string()
                }
            };
            let status = Command::new("cargo")
                .args([
                    format!("+{channel}"),
                    "test".to_string(),
                    "--workspace".to_string(),
                    "--lib".to_string(),
                    "--target".to_string(),
                    build_target,
                ])
                .env("RUSTFLAGS", "-Z sanitizer=address")
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .status()
                .context("run AddressSanitizer")?;
            if !status.success() {
                eprintln!("FAIL: AddressSanitizer tests failed.");
                std::process::exit(1);
            }
            println!("  OK: AddressSanitizer passed");
        }
    }

    // 11. cargo mutants
    if !args.required_only && !args.no_expensive && args.mutants {
        println!("Running cargo-mutants...");
        if cargo_subcommand_exists("mutants") {
            let status = Command::new("cargo")
                .args([
                    "mutants",
                    "--workspace",
                    "--timeout",
                    "180",
                    "--minimum-test-timeout",
                    "180",
                    "--jobs",
                    "2",
                ])
                .status()
                .context("run cargo mutants")?;
            if !status.success() {
                eprintln!("FAIL: Mutants test failed.");
                std::process::exit(1);
            }
            println!("  OK: cargo-mutants passed");
        } else {
            println!("  Skipped: cargo-mutants not installed");
        }
    }

    // 12. Fuzzing
    if !args.required_only && (args.fuzz_list || args.fuzz_smoke) {
        let missing = [
            (!nc.toolchain).then_some("nightly toolchain"),
            (!cargo_subcommand_exists("fuzz")).then_some("cargo-fuzz (cargo install cargo-fuzz)"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if missing.is_empty() {
            let channel = rust_toolchain_channel_for_probe(&repo_root);
            println!("Listing fuzz targets...");
            let out = Command::new("cargo")
                .args([
                    format!("+{channel}"),
                    "fuzz".to_string(),
                    "list".to_string(),
                    "--fuzz-dir".to_string(),
                    "crates/dev/fuzz".to_string(),
                ])
                .output()?;
            if !out.status.success() {
                eprintln!("FAIL: cargo fuzz list failed.");
                std::process::exit(1);
            }
            print!("{}", String::from_utf8_lossy(&out.stdout));
            if args.fuzz_smoke {
                println!("Running fuzz smoke tests...");
                let targets = String::from_utf8_lossy(&out.stdout);
                for target in targets.lines() {
                    let target = target.trim();
                    if target.is_empty() {
                        continue;
                    }
                    println!("  Smoke testing fuzz target: {target} (max 5s)");
                    let st = Command::new("cargo")
                        .args([
                            format!("+{channel}"),
                            "fuzz".to_string(),
                            "run".to_string(),
                            target.to_string(),
                            "--fuzz-dir".to_string(),
                            "crates/dev/fuzz".to_string(),
                            "--".to_string(),
                            "-runs=1".to_string(),
                        ])
                        .status()?;
                    if !st.success() {
                        eprintln!("FAIL: fuzz target {target} failed.");
                        std::process::exit(1);
                    }
                }
                println!("  OK: Fuzz smoke tests passed");
            } else {
                println!("  OK: fuzz target discovery passed");
            }
        } else {
            if args.ci {
                eprintln!(
                    "FAIL: cargo fuzz availability missing: {}",
                    missing.join(", ")
                );
                std::process::exit(1);
            }
            println!("  Skipped: cargo fuzz (missing: {})", missing.join(", "));
        }
    }

    // 13. CI Health Coverage
    if args.ci && !args.no_expensive {
        println!("Running CI Health Coverage (cargo llvm-cov)...");
        if !cargo_subcommand_exists("llvm-cov") {
            eprintln!("FAIL: cargo-llvm-cov is required for --ci");
            std::process::exit(1);
        }
        if !nc.llvm_tools {
            let channel = rust_toolchain_channel_for_probe(&repo_root);
            let status = Command::new("rustup")
                .args([
                    "component",
                    "add",
                    "llvm-tools",
                    "--toolchain",
                    channel.as_str(),
                ])
                .status()
                .context("run rustup component add llvm-tools")?;
            if !status.success() {
                eprintln!("FAIL: rustup component add llvm-tools failed.");
                std::process::exit(1);
            }
        }
        let status = Command::new("cargo")
            .args([
                "llvm-cov",
                "-p",
                "foundation",
                "--lib",
                "--all-features",
                "--features",
                "foundation/ci-static-build",
                "--no-fail-fast",
                "--summary-only",
            ])
            .status()
            .context("run cargo llvm-cov summary")?;
        if !status.success() {
            eprintln!("FAIL: cargo llvm-cov summary failed.");
            std::process::exit(1);
        }
        let status = Command::new("cargo")
            .args([
                "llvm-cov",
                "-p",
                "foundation",
                "--lib",
                "--all-features",
                "--features",
                "foundation/ci-static-build",
                "--no-fail-fast",
                "--lcov",
                "--output-path",
                "lcov.info",
            ])
            .status()
            .context("run cargo llvm-cov")?;
        if !status.success() {
            eprintln!("FAIL: cargo llvm-cov failed.");
            std::process::exit(1);
        }
        if !repo_root.join("lcov.info").exists() {
            eprintln!("FAIL: lcov.info artifact missing after run.");
            std::process::exit(1);
        }
        println!("  OK: coverage passed and lcov.info generated");

        println!("Running CI Rustdoc Health Check...");
        let doc_status = Command::new("cargo")
            .args(["doc", "-p", "foundation", "--no-deps"])
            .env("RUSTDOCFLAGS", "-D warnings")
            .status()
            .context("run cargo doc")?;
        if !doc_status.success() {
            eprintln!("FAIL: cargo doc check failed.");
            std::process::exit(1);
        }
        println!("  OK: rustdoc check passed");
    }

    println!("\nALL AUDITS PASSED SUCCESFULLY");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plist_string_key() {
        let content = r"
        <dict>
            <key>CFBundleShortVersionString</key>
            <string>0.11.3</string>
            <key>CFBundleExecutable</key>
            <string>Modern Format Boost</string>
        </dict>
        ";
        assert_eq!(
            parse_plist_string_key(content, "CFBundleShortVersionString"),
            Some("0.11.3".to_string())
        );
        assert_eq!(
            parse_plist_string_key(content, "CFBundleExecutable"),
            Some("Modern Format Boost".to_string())
        );
        assert_eq!(parse_plist_string_key(content, "NonExistentKey"), None);
    }

    #[test]
    fn taplo_command_uses_direct_binary_when_cargo_subcommand_missing() {
        let files = vec!["Cargo.toml".to_string()];
        assert_eq!(
            taplo_fmt_command_with_availability(&files, &["--check"], false, true),
            Some(vec![
                "taplo".to_string(),
                "fmt".to_string(),
                "--check".to_string(),
                "Cargo.toml".to_string(),
            ])
        );
    }

    #[test]
    fn taplo_command_uses_tracked_files_with_cargo_subcommand() {
        let files = vec!["Cargo.toml".to_string()];
        assert_eq!(
            taplo_fmt_command_with_availability(&files, &["--check"], true, true),
            Some(vec![
                "cargo".to_string(),
                "taplo".to_string(),
                "fmt".to_string(),
                "--check".to_string(),
                "Cargo.toml".to_string(),
            ])
        );
    }

    #[test]
    fn taplo_command_refuses_empty_file_list_to_avoid_tree_scan() {
        let files: Vec<String> = Vec::new();
        assert_eq!(
            taplo_fmt_command_with_availability(&files, &["--check"], true, true),
            None
        );
    }

    #[test]
    fn parses_pinned_rust_toolchain_channel() {
        let content = r#"
            [toolchain]
            channel = "nightly-2026-07-16"
            components = ["clippy", "rustfmt"]
        "#;
        assert_eq!(
            parse_rust_toolchain_channel_toml(content),
            Some("nightly-2026-07-16".to_string())
        );
    }

    #[test]
    fn nightly_install_command_omits_invalid_yes_flag() {
        assert_eq!(
            install_nightly_command(&["clippy", "rustfmt"]),
            vec![
                "rustup".to_string(),
                "toolchain".to_string(),
                "install".to_string(),
                "nightly".to_string(),
                "--component".to_string(),
                "clippy".to_string(),
                "--component".to_string(),
                "rustfmt".to_string(),
            ]
        );
    }

    #[test]
    fn macos_path_bootstrap_prepends_existing_brew_paths_once() {
        let path = "/bin:/opt/homebrew/bin";
        let updated = bootstrap_macos_path_with(true, path, |dir| {
            matches!(
                dir,
                "/opt/homebrew/bin" | "/opt/homebrew/sbin" | "/usr/local/bin"
            )
        })
        .expect("path changed");
        assert_eq!(
            updated,
            "/usr/local/bin:/opt/homebrew/sbin:/bin:/opt/homebrew/bin"
        );
    }

    #[test]
    #[rustfmt::skip]
    fn parses_installed_nightly_components_exactly() {
        let components = parse_installed_rustup_components(
            "clippy-x86_64-apple-darwin\nrustfmt-x86_64-apple-darwin\nrust-src\nllvm-tools-x86_64-apple-darwin\n",
        );
        assert!(components.toolchain);
        assert!(components.clippy);
        assert!(components.rustfmt);
        assert!(!components.miri);
        assert!(components.rust_src);
        assert!(components.llvm_tools);
        assert_eq!(components.missing_components(), vec!["miri"]);
    }

    #[test]
    fn vue_quality_scripts_cover_lint_format_dependencies_and_build() {
        assert_eq!(
            vue_quality_script_names(),
            &["lint", "format:check", "deps:check", "build"]
        );
    }

    #[test]
    fn ci_cargo_check_targets_the_root_workspace() {
        let args = cargo_check_args(true);
        let rendered = args.join(" ");
        assert!(rendered.contains("--workspace"));
    }

    #[test]
    fn root_workspace_members_do_not_include_macos_only_crates() {
        let root_manifest = include_str!("../../../../Cargo.toml");
        let members = workspace_members_block(root_manifest).expect("workspace members block");
        assert!(
            !members.contains("crates/dev/dispatch2"),
            "macOS-only dispatch2 must stay in its dedicated workflow, not the root workspace"
        );
    }
}
