//! iCloud Import tool.
//!
//! Imports processed media into Apple Photos / iCloud via `osxphotos`.
//!
//! Two import modes:
//!   Mode 1 (Optimized): ✨ emoji prefix + organised album structure
//! (✨/{folder})   Mode 2 (Simple):    plain import organised by folder name,
//! no emoji rename
//!
//! Behaviour parity:
//!   - Process-lock (flock) prevents concurrent imports
//!   - Searches PATH + common Homebrew / local paths for osxphotos
//!   - Strips `_optimized_collected`, `_collected_optimized`, `_optimized`,
//!     `_collected` suffixes from album folder names
//!   - Interactive mode selection (skippable via --mode flag)
//!   - Streams osxphotos output line-by-line to stdout

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use dev::infra::ui_tokens::pick_symbol;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ImportMode {
    /// Mode 1: ✨ prefix + organized album structure (default)
    Optimized,
    /// Mode 2: plain import with basic folder-name album organization
    Simple,
}

#[derive(Parser, Debug)]
#[command(
    name = "icloud_import",
    about = "Import processed media into Apple Photos / iCloud via osxphotos"
)]
struct Args {
    /// Directory containing media files to import
    target: PathBuf,

    /// Import mode (skips interactive menu)
    #[arg(long, value_enum)]
    mode: Option<ImportMode>,

    /// Skip the interactive 'yes' confirmation prompt
    #[arg(long)]
    yes: bool,
}

// ── process lock (mirrors fcntl.flock in py) ─────────────────────────────────

fn lock_path() -> PathBuf {
    let root = std::env::var("MFB_HOME_ROOT")
        .map_or_else(|_| dirs_home().join(".modern_format_boost"), PathBuf::from);
    root.join("locks").join("photos_import.lock")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from)
}

/// Acquire an exclusive non-blocking flock on the import lock file.
/// Returns the open lock file on success or None if already locked.
fn acquire_import_lock() -> Option<fs::File> {
    let path = lock_path();
    if let Some(parent) = path.parent()
        && !dev::infra::hardening::ensure_parent_dir(parent)
    {
        return None;
    }
    let file = match OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(err) => {
            eprintln!("[ICLOUD] lock open failed ({}): {err}", path.display());
            return None;
        }
    };
    // SAFETY: valid fd, LOCK_EX|LOCK_NB — non-blocking exclusive flock.
    let ret = unsafe {
        libc::flock(
            std::os::unix::io::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if ret == 0 { Some(file) } else { None }
}

fn release_import_lock(file: fs::File) {
    // SAFETY: valid fd, LOCK_UN.
    unsafe { libc::flock(std::os::unix::io::AsRawFd::as_raw_fd(&file), libc::LOCK_UN) };
    drop(file);
}

// ── osxphotos discovery
// ───────────────────────────────────────────────────────

const OSXPHOTOS_SEARCH_PATHS: &[&str] =
    &["/opt/homebrew/bin/osxphotos", "/usr/local/bin/osxphotos"];

fn command_succeeded(mut cmd: Command, label: &str) -> bool {
    match cmd.output() {
        Ok(out) => out.status.success(),
        Err(err) => {
            eprintln!("[ICLOUD] {label} failed: {err}");
            false
        }
    }
}

fn find_osxphotos() -> Option<String> {
    let mut path_cmd = Command::new("osxphotos");
    path_cmd.arg("--version");
    if command_succeeded(path_cmd, "osxphotos --version") {
        return Some("osxphotos".to_owned());
    }
    let home_local = dirs_home().join(".local/bin/osxphotos");
    let extra = std::iter::once(home_local.to_string_lossy().into_owned());
    let mut candidates = extra.chain(
        OSXPHOTOS_SEARCH_PATHS
            .iter()
            .map(std::string::ToString::to_string),
    );
    candidates.find(|path| {
        if !Path::new(path).is_file() {
            return false;
        }
        let mut cmd = Command::new(path);
        cmd.arg("--version");
        command_succeeded(cmd, &format!("{path} --version"))
    })
}

// ── folder name helpers
// ───────────────────────────────────────────────────────

fn get_album_name(target: &Path) -> Result<String> {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .context("target path has no valid directory name")?;

    // In case the target already has emoji prepended by Mode 1 or earlier runs
    let name = name.strip_prefix("✨").unwrap_or(name);
    let name = name.strip_prefix("[*]").unwrap_or(name);

    let name = name.strip_suffix("_optimized_collected").unwrap_or(name);
    let name = name.strip_suffix("_collected_optimized").unwrap_or(name);
    let name = name.strip_suffix("_optimized").unwrap_or(name);
    let name = name.strip_suffix("_collected").unwrap_or(name);

    Ok(name.trim().to_string())
}

/// The osxphotos `--album` template for Mode 1 uses the target folder name.
fn optimized_album_template(target: &Path) -> Result<String> {
    Ok(format!("✨/✨{}", get_album_name(target)?))
}

/// The osxphotos `--album` template for Mode 2.
fn simple_album_template(target: &Path) -> Result<String> {
    get_album_name(target)
}

// ── folder rename (Mode 1 only)
// ───────────────────────────────────────────────

fn rename_with_emoji(target: &Path) -> Result<PathBuf> {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .context("target has no valid directory name")?;
    if name.starts_with('✨') {
        println!(
            "   {} Folder already has {} prefix: {name}",
            pick_symbol("ℹ️", "[INFO]"),
            pick_symbol("✨", "[*]")
        );
        return Ok(target.to_path_buf());
    }
    let new_name = format!("{}{name}", pick_symbol("✨", "[*]"));
    let new_path = target
        .parent()
        .context("target has no parent")?
        .join(&new_name);
    match fs::rename(target, &new_path) {
        Ok(()) => {
            println!(
                "   {} Folder renamed: {name} -> {new_name}",
                pick_symbol("✨", "[*]")
            );
            Ok(new_path)
        }
        Err(e) => {
            eprintln!(
                "   {} Failed to rename folder: {e}",
                pick_symbol("⚠️", "[WARN]")
            );
            Ok(target.to_path_buf()) // non-fatal: continue with original path
        }
    }
}

// ── osxphotos subprocess runner
// ───────────────────────────────────────────────

fn run_osxphotos(cmd_args: &[String]) -> Result<bool> {
    let mut child = Command::new(&cmd_args[0])
        .args(&cmd_args[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn osxphotos: {}", cmd_args.join(" ")))?;

    // Stream output line by line (mirrors py Popen iteration)
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let line = line?;
            println!("   {}", line.trim());
        }
    }
    let status = child.wait().context("wait for osxphotos")?;
    Ok(status.success())
}

// ── confirm helper
// ────────────────────────────────────────────────────────────

fn confirm(prompt: &str, yes: bool) -> bool {
    if yes {
        return true;
    }
    print!("{prompt}");
    dev::infra::hardening::flush_stdout();
    let mut line = String::new();
    dev::infra::hardening::read_stdin_line(&mut line);
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

// ── Mode 1: optimized import
// ──────────────────────────────────────────────────

fn run_optimized_import(target: &Path, osxphotos: &str, yes: bool) -> Result<bool> {
    println!(
        "\n{} Preparing for optimized import...",
        pick_symbol("⏳", "[WAIT]")
    );

    let album_template = optimized_album_template(target)?;

    let target = rename_with_emoji(target)?;
    if !target.is_dir() {
        bail!("{} is not a directory", target.display());
    }

    println!(
        "\n{} Starting Optimized Import...",
        pick_symbol("🚀", "[LAUNCH]")
    );
    println!("   Target:     {}", target.display());
    println!(
        "   Mode:       Organized ({}/{{folder_name}})",
        pick_symbol("✨", "[*]")
    );
    println!("   Auto-Album: Enabled");
    println!("   Album Name: Auto-strip suffix from folder names");

    println!("\n{}  READY TO IMPORT?", pick_symbol("⚠️", "[WARN]"));
    if !confirm("   Type 'yes' to proceed: ", yes) {
        println!(
            "\n{} Import cancelled by user.",
            pick_symbol("❌", "[ERROR]")
        );
        return Ok(false);
    }

    println!(
        "\n{} Initializing osxphotos...",
        pick_symbol("⚙️", "[GEAR]")
    );
    println!("   Connecting to Apple Photos library...");

    let cmd = vec![
        osxphotos.to_owned(),
        "import".to_owned(),
        target.to_string_lossy().into_owned(),
        "--walk".to_owned(),
        "--album".to_owned(),
        album_template,
        "--split-folder".to_owned(),
        "/".to_owned(),
    ];

    let ok = run_osxphotos(&cmd)?;
    if ok {
        println!(
            "\n{} Optimized import completed successfully!",
            pick_symbol("✅", "[OK]")
        );
    } else {
        eprintln!("\n{} Import failed.", pick_symbol("❌", "[ERROR]"));
    }
    Ok(ok)
}

// ── Mode 2: simple import
// ─────────────────────────────────────────────────────

fn run_simple_import(target: &Path, osxphotos: &str, yes: bool) -> Result<bool> {
    println!(
        "\n{} Preparing for simple import...",
        pick_symbol("⏳", "[WAIT]")
    );

    if !target.is_dir() {
        bail!("{} is not a directory", target.display());
    }

    println!(
        "\n{} Starting Simple Import...",
        pick_symbol("🚀", "[LAUNCH]")
    );
    println!("   Target:     {}", target.display());
    println!("   Mode:       Simple (organized by folder name)");
    println!("   Album Name: Auto-strip suffix from folder names");

    println!("\n{}  READY TO IMPORT?", pick_symbol("⚠️", "[WARN]"));
    if !confirm("   Type 'yes' to proceed: ", yes) {
        println!(
            "\n{} Import cancelled by user.",
            pick_symbol("❌", "[ERROR]")
        );
        return Ok(false);
    }

    println!(
        "\n{} Initializing osxphotos...",
        pick_symbol("⚙️", "[GEAR]")
    );

    let album_template = simple_album_template(target)?;

    let cmd = vec![
        osxphotos.to_owned(),
        "import".to_owned(),
        target.to_string_lossy().into_owned(),
        "--walk".to_owned(),
        "--album".to_owned(),
        album_template,
    ];

    let ok = run_osxphotos(&cmd)?;
    if ok {
        println!(
            "\n{} Simple import completed successfully!",
            pick_symbol("✅", "[OK]")
        );
    } else {
        eprintln!("\n{} Import failed.", pick_symbol("❌", "[ERROR]"));
    }
    Ok(ok)
}

// ── interactive mode selection
// ────────────────────────────────────────────────

fn select_import_mode() -> ImportMode {
    loop {
        println!(
            "\n{} iCloud Import Mode Selection",
            pick_symbol("📱", "[PHONE]")
        );
        println!("{}", "─".repeat(50));
        println!(
            "  1 - Optimized Import (Default)\n     • Auto-rename folder with {} emoji\n     • \
             Organize into {}/{{folder_name}} albums\n     • Best for processed/final media",
            pick_symbol("✨", "[*]"),
            pick_symbol("✨", "[*]")
        );
        println!();
        println!(
            "  2 - Simple Import\n     • Basic album organization by folder name\n     • No {} \
             renaming",
            pick_symbol("✨", "[*]")
        );
        println!("{}", "─".repeat(50));

        print!("Select mode (1 or 2) [default: 1]: ");
        dev::infra::hardening::flush_stdout();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return ImportMode::Optimized;
        }
        match line.trim() {
            "" | "1" => return ImportMode::Optimized,
            "2" => return ImportMode::Simple,
            _ => {
                eprintln!(
                    "{} Invalid choice. Please enter 1 or 2.",
                    pick_symbol("❌", "[ERROR]")
                );
            }
        }
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();

    // 1. Check osxphotos
    let osxphotos = find_osxphotos().ok_or_else(|| {
        anyhow::anyhow!(
            "'osxphotos' not found in PATH or common locations.\nTried: ~/.local/bin, \
             /opt/homebrew/bin, /usr/local/bin\nInstall with: pip install osxphotos"
        )
    })?;

    // 2. Acquire process lock
    let lock = acquire_import_lock().ok_or_else(|| {
        anyhow::anyhow!(
            "Another import operation is already in progress.\nIf this is an error, delete: {}",
            lock_path().display()
        )
    })?;

    // 3. Resolve mode
    let mode = args.mode.unwrap_or_else(select_import_mode);

    // 4. Run
    let target = args
        .target
        .canonicalize()
        .with_context(|| format!("resolve target: {}", args.target.display()))?;

    let ok = match mode {
        ImportMode::Optimized => run_optimized_import(&target, &osxphotos, args.yes)?,
        ImportMode::Simple => run_simple_import(&target, &osxphotos, args.yes)?,
    };

    release_import_lock(lock);

    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimized_album_template_contains_emoji_root() {
        let path = Path::new("/some/folder/my_album_optimized");
        let t = optimized_album_template(path).unwrap();
        assert!(t.starts_with("✨/✨"));
        assert_eq!(t, "✨/✨my_album");
    }

    #[test]
    fn test_simple_album_template_no_emoji_root() {
        let path = Path::new("/some/folder/my_album_optimized_collected");
        let t = simple_album_template(path).unwrap();
        assert!(!t.starts_with("✨/✨"));
        assert_eq!(t, "my_album");
    }

    #[test]
    fn test_rename_with_emoji_already_prefixed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let folder = dir.path().join("✨already");
        std::fs::create_dir_all(&folder)?;
        let result = rename_with_emoji(&folder)?;
        assert_eq!(result, folder, "should not rename if already prefixed");
        Ok(())
    }

    #[test]
    fn test_rename_with_emoji_adds_prefix() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let folder = dir.path().join("Vacation_optimized");
        std::fs::create_dir_all(&folder)?;
        let result = rename_with_emoji(&folder)?;
        let name = result.file_name().unwrap().to_string_lossy().into_owned();
        // pick_symbol returns ✨ on TTY or [*] in CI/tests — accept either
        let expected_prefix = pick_symbol("✨", "[*]");
        assert!(
            name.starts_with(expected_prefix),
            "renamed folder '{name}' must start with '{expected_prefix}'"
        );
        assert!(result.is_dir(), "renamed path must still be a dir");
        Ok(())
    }

    #[test]
    fn test_lock_path_uses_mfb_home_root() {
        // SAFETY: single-threaded test setup.
        unsafe { std::env::set_var("MFB_HOME_ROOT", "/tmp/mfb_test") };
        let p = lock_path();
        assert!(p.to_string_lossy().contains("mfb_test"));
        assert_eq!(p.file_name().unwrap(), "photos_import.lock");
    }
}
