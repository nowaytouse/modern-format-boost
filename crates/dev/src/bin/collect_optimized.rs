//! Collect-optimized media mover.
//!
//! Moves only optimized outputs (`.jxl` images + HEVC `.mov`/`.mp4` videos)
//! into a mirrored destination directory tree, leaving legacy formats in place.
//!
//! Pipeline:
//!   1. Validate source / destination paths
//!   2. Snapshot directory timestamps (preserve metadata across moves)
//!   3. Scan for candidate files (skip symlinks; probe video codec via ffprobe)
//!   4. Mirror destination directory skeleton
//!   5. Move each candidate (skip if already at destination)
//!   6. Prune empty source directories
//!   7. Restore directory timestamps on both trees
//!   8. Print summary + emit COLLECT_* audit events

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::hardening::{ensure_parent_dir, flush_stdout, read_stdin_line};
use dev::infra::log_paths::audit_log_path_from_env;
use dev::infra::ui_tokens::pick_symbol;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::time::SystemTime;
use std::{fs, io};

// ── constants ─────────────────────────────────────────────────────────────────

const IMAGE_EXTENSIONS: &[&str] = &["jxl"];
const VIDEO_EXTENSIONS: &[&str] = &["mov", "mp4"];
const TARGET_VIDEO_CODECS: &[&str] = &["hevc"];
const PROBE_FAILURE_PREVIEW: usize = 10;
const FFPROBE_TIMEOUT: Duration = Duration::from_secs(15);

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "collect_optimized",
    about = "Move optimized media (JXL + HEVC) into a mirrored destination tree"
)]
struct Args {
    /// Source directory to scan
    source: PathBuf,

    /// Target directory to move files into
    destination: PathBuf,

    /// Preview directory mirroring and moves without making changes
    #[arg(long)]
    dry_run: bool,

    /// Skip the interactive 'yes' confirmation prompt
    #[arg(long)]
    yes: bool,
}

// ── audit ─────────────────────────────────────────────────────────────────────

fn collect_audit(event: &str, fields: &[(&str, &str)]) {
    let Some(audit_path) = audit_log_path_from_env() else {
        return;
    };
    let mut fields = fields.to_vec();
    fields.sort_by_key(|(key, _)| *key);
    let parts: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let line = if parts.is_empty() {
        format!("COLLECT_{event}")
    } else {
        format!("COLLECT_{event} {}", parts.join(" "))
    };
    let _ = append_plain_audit_line(&audit_path, &line);
}

fn append_plain_audit_line(path: &Path, line: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        let _ = ensure_parent_dir(parent);
    }
    let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open collect audit {}", path.display()))?;
    writeln!(file, "{stamp} {line}")
        .with_context(|| format!("write collect audit {}", path.display()))?;
    Ok(())
}

// ── video codec probe ─────────────────────────────────────────────────────────

/// Call ffprobe to get the primary video codec name.
/// Returns `(Some(codec), None)` on success or `(None, Some(error))` on failure.
fn probe_video_codec(path: &Path) -> (Option<String>, Option<String>) {
    let mut child = match Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return (
                None,
                Some("ffprobe is not installed or not in PATH".to_owned()),
            );
        }
        Err(e) => return (None, Some(e.to_string())),
    };

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) if start.elapsed() >= FFPROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return (None, Some("ffprobe timed out".to_owned()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return (None, Some(e.to_string())),
        }
    }

    match child.wait_with_output() {
        Err(e) => (None, Some(e.to_string())),
        Ok(out) if !out.status.success() => {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = if err.trim().is_empty() {
                format!("ffprobe exited with status {}", out.status)
            } else {
                err.trim().to_owned()
            };
            (None, Some(msg))
        }
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let codec = stdout.lines().next().map(|s| s.trim().to_lowercase());
            match codec {
                Some(c) if !c.is_empty() => (Some(c), None),
                _ => (None, Some("ffprobe returned no video codec".to_owned())),
            }
        }
    }
}

// ── directory timestamp snapshot ──────────────────────────────────────────────

type DirTimestamps = BTreeMap<PathBuf, (SystemTime, SystemTime)>;

fn snapshot_directories(src_root: &Path) -> Result<DirTimestamps> {
    let mut map = DirTimestamps::new();
    for entry in walkdir::WalkDir::new(src_root).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", src_root.display()))?;
        if entry.file_type().is_dir() {
            let meta = fs::metadata(entry.path())
                .with_context(|| format!("stat dir {}", entry.path().display()))?;
            let atime = meta
                .accessed()
                .with_context(|| format!("read atime {}", entry.path().display()))?;
            let mtime = meta
                .modified()
                .with_context(|| format!("read mtime {}", entry.path().display()))?;
            map.insert(entry.path().to_path_buf(), (atime, mtime));
        }
    }
    Ok(map)
}

// ── candidate scan ────────────────────────────────────────────────────────────

struct ScanResult {
    candidates: Vec<PathBuf>,
    image_count: usize,
    video_count: usize,
    symlink_count: usize,
    probe_failures: Vec<(PathBuf, String)>,
}

fn scan_candidates(src_root: &Path) -> Result<ScanResult> {
    let mut res = ScanResult {
        candidates: Vec::new(),
        image_count: 0,
        video_count: 0,
        symlink_count: 0,
        probe_failures: Vec::new(),
    };

    for entry in walkdir::WalkDir::new(src_root).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", src_root.display()))?;
        // Skip symlinks entirely (mirrors py symlink_count behaviour)
        if entry.path_is_symlink() {
            res.symlink_count += 1;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();

        if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            res.candidates.push(path.to_path_buf());
            res.image_count += 1;
            continue;
        }
        if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            let (codec, err) = probe_video_codec(path);
            if let Some(e) = err {
                res.probe_failures.push((path.to_path_buf(), e));
                continue;
            }
            if let Some(c) = codec
                && TARGET_VIDEO_CODECS.contains(&c.as_str())
            {
                res.candidates.push(path.to_path_buf());
                res.video_count += 1;
            }
        }
    }
    Ok(res)
}

// ── destination layout mirror ─────────────────────────────────────────────────

fn ensure_destination_layout(
    src_root: &Path,
    dest_root: &Path,
    dir_meta: &DirTimestamps,
    dry_run: bool,
) -> Result<usize> {
    let mut created = 0;
    // Sorted by path length ascending — create parents before children
    let mut dirs: Vec<&PathBuf> = dir_meta.keys().collect();
    dirs.sort_by_key(|p| p.as_os_str().len());

    for src_dir in dirs {
        let rel = src_dir
            .strip_prefix(src_root)
            .with_context(|| format!("strip_prefix failed for {}", src_dir.display()))?;
        let dest_dir = if rel == Path::new(".") {
            dest_root.to_path_buf()
        } else {
            dest_root.join(rel)
        };
        if dest_dir.is_dir() {
            continue;
        }
        created += 1;
        if !dry_run {
            fs::create_dir_all(&dest_dir)
                .with_context(|| format!("create dest dir {}", dest_dir.display()))?;
        }
    }
    Ok(created)
}

// ── timestamp restoration ─────────────────────────────────────────────────────

fn restore_directory_times(
    src_root: &Path,
    dest_root: &Path,
    dir_meta: &DirTimestamps,
) -> Result<()> {
    restore_directory_times_with(src_root, dest_root, dir_meta, set_times)
}

fn restore_directory_times_with<F>(
    src_root: &Path,
    dest_root: &Path,
    dir_meta: &DirTimestamps,
    mut set_times_fn: F,
) -> Result<()>
where
    F: FnMut(&Path, SystemTime, SystemTime) -> Result<()>,
{
    // Restore deepest dirs first (reverse length sort)
    let mut dirs: Vec<&PathBuf> = dir_meta.keys().collect();
    dirs.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));

    for src_dir in dirs {
        let (atime, mtime) = dir_meta[src_dir];
        let rel = src_dir
            .strip_prefix(src_root)
            .with_context(|| format!("strip_prefix failed for {}", src_dir.display()))?;
        let dest_dir = if rel == Path::new(".") {
            dest_root.to_path_buf()
        } else {
            dest_root.join(rel)
        };
        if dest_dir.is_dir() {
            set_times_fn(&dest_dir, atime, mtime)?;
        }
        if src_dir.is_dir() {
            set_times_fn(src_dir, atime, mtime)?;
        }
    }
    Ok(())
}

fn set_times(path: &Path, atime: SystemTime, mtime: SystemTime) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    fn to_timeval(t: SystemTime) -> Result<libc::timeval> {
        let dur = t
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("SystemTime before UNIX_EPOCH")?;
        Ok(libc::timeval {
            tv_sec: foundation::numeric_cast::u64_to_i64_strict(dur.as_secs(), "tv_sec")
                .context("tv_sec overflow")?,
            #[cfg(target_os = "macos")]
            tv_usec: dur.subsec_micros().try_into().expect("tv_usec overflow"),
            #[cfg(not(target_os = "macos"))]
            tv_usec: dur.subsec_micros().into(),
        })
    }

    let cpath = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path contains NUL: {}", path.display()))?;
    let times = [to_timeval(atime)?, to_timeval(mtime)?];
    // SAFETY: cpath is valid NUL-terminated; times is a local 2-element array.
    let rc = unsafe { libc::utimes(cpath.as_ptr(), times.as_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("restore failed for directory {}", path.display()));
    }
    Ok(())
}

// ── prune empty source dirs ───────────────────────────────────────────────────

fn prune_empty_source_directories(dir_meta: &DirTimestamps) -> usize {
    let mut removed = 0;
    let mut dirs: Vec<&PathBuf> = dir_meta.keys().collect();
    dirs.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        match fs::read_dir(dir) {
            Ok(entries) => {
                let mut entries = entries;
                if entries.next().is_none() {
                    if fs::remove_dir(dir).is_ok() {
                        removed += 1;
                    } else {
                        collect_audit(
                            "PRUNE_EMPTY_SOURCE_DIR_FAILED",
                            &[("path", &dir.display().to_string())],
                        );
                    }
                }
            }
            Err(err) => eprintln!(
                "[COLLECT] read_dir failed for prune ({}): {err}",
                dir.display()
            ),
        }
    }
    removed
}

// ── validate ──────────────────────────────────────────────────────────────────

fn validate_paths(src_root: &Path, dest_root: &Path) -> bool {
    if !src_root.is_dir() {
        eprintln!("Error: Source {} is not a directory.", src_root.display());
        return false;
    }
    if dest_root.exists() && !dest_root.is_dir() {
        eprintln!(
            "Error: Destination {} exists but is not a directory.",
            dest_root.display()
        );
        return false;
    }
    let src_abs = src_root.canonicalize();
    let dest_abs = if dest_root.exists() {
        dest_root.canonicalize()
    } else {
        let parent = dest_root.parent().unwrap_or_else(|| Path::new("."));
        parent.canonicalize().map(|p| {
            dest_root
                .file_name()
                .map_or_else(|| p.clone(), |name| p.join(name))
        })
    };
    if let (Ok(src_abs), Ok(dest_abs)) = (src_abs, dest_abs)
        && dest_abs.starts_with(&src_abs)
        && dest_abs != src_abs
    {
        eprintln!("Error: Destination cannot be inside the source directory.");
        return false;
    }
    true
}

// ── probe failure display ─────────────────────────────────────────────────────

fn print_probe_failures(src_root: &Path, failures: &[(PathBuf, String)]) {
    if failures.is_empty() {
        return;
    }
    println!(">>> Video probe failures: {}", failures.len());
    for (path, err) in failures.iter().take(PROBE_FAILURE_PREVIEW) {
        let rel = path.strip_prefix(src_root).unwrap_or(path);
        println!("   - {}: {}", rel.display(), err);
    }
    let remaining = failures.len().saturating_sub(PROBE_FAILURE_PREVIEW);
    if remaining > 0 {
        println!("   ... and {remaining} more probe failures");
    }
}

// ── main collection logic ─────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn run_collection(src_root: &Path, dest_root: &Path, dry_run: bool, yes: bool) -> Result<bool> {
    if !validate_paths(src_root, dest_root) {
        return Ok(false);
    }

    println!();
    println!("{} COLLECTION TASK PREVIEW", pick_symbol("📂", "[DIR]"));
    println!("   Source:      {}", src_root.display());
    println!("   Destination: {}", dest_root.display());
    if dry_run {
        println!("   {}  DRY RUN MODE ENABLED", pick_symbol("⚠️", "[WARN]"));
    }

    // Confirmation
    if !yes {
        println!(
            "\n{}  CONFIRM: Start collecting optimized media?",
            pick_symbol("⚠️", "[WARN]")
        );
        print!("   Type 'yes' to proceed: ");
        flush_stdout();
        let mut line = String::new();
        read_stdin_line(&mut line);
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("\n{} Task cancelled by user.", pick_symbol("❌", "[ERROR]"));
            collect_audit(
                "DECLINED",
                &[
                    ("source", &src_root.display().to_string()),
                    ("destination", &dest_root.display().to_string()),
                ],
            );
            return Ok(false);
        }
    }

    collect_audit(
        "START",
        &[
            ("source", &src_root.display().to_string()),
            ("destination", &dest_root.display().to_string()),
            ("dry_run", if dry_run { "1" } else { "0" }),
        ],
    );

    println!("\n>>> Snapshotting directory structure and timestamps...");
    let dir_meta = snapshot_directories(src_root)?;

    println!(
        ">>> Scanning for optimized media in {}...",
        src_root.display()
    );
    let scan = scan_candidates(src_root)?;
    let mut removed_empty = 0usize;

    // Emit probe failures to audit
    for (path, err) in &scan.probe_failures {
        let rel = path.strip_prefix(src_root).unwrap_or(path);
        let err_trunc: String = err.replace('\n', " ").chars().take(200).collect();
        collect_audit(
            "PROBE_FAIL",
            &[("path", &rel.display().to_string()), ("error", &err_trunc)],
        );
    }
    print_probe_failures(src_root, &scan.probe_failures);

    if scan.candidates.is_empty() {
        if !dry_run {
            removed_empty = prune_empty_source_directories(&dir_meta);
            if removed_empty > 0 {
                println!(">>> Removed {removed_empty} empty source directory/directories.");
            }
        }
        if scan.symlink_count > 0 {
            println!(
                "No optimized files found ({} symlinks ignored).",
                scan.symlink_count
            );
        } else {
            println!("No optimized files found.");
        }
        collect_audit(
            "EMPTY",
            &[
                ("source", &src_root.display().to_string()),
                ("symlinks", &scan.symlink_count.to_string()),
                ("probe_failures", &scan.probe_failures.len().to_string()),
            ],
        );
        if removed_empty > 0 {
            println!("Removed Empty Source Directories: {removed_empty}");
        }
        return Ok(true);
    }

    println!(">>> Identified {} candidate files.", scan.candidates.len());
    println!(
        ">>> Candidate breakdown: {} JXL, {} HEVC video(s).",
        scan.image_count, scan.video_count
    );
    if scan.symlink_count > 0 {
        println!(
            ">>> Note: {} symlinks were ignored during the scan.",
            scan.symlink_count
        );
    }
    collect_audit(
        "SCAN",
        &[
            ("candidates", &scan.candidates.len().to_string()),
            ("images", &scan.image_count.to_string()),
            ("videos", &scan.video_count.to_string()),
            ("symlinks", &scan.symlink_count.to_string()),
        ],
    );

    let mirrored = ensure_destination_layout(src_root, dest_root, &dir_meta, dry_run)?;
    if dry_run {
        println!("--- DRY RUN MODE: No files will be moved ---");
        println!("[DRY-RUN] Would mirror {mirrored} directory/directories at destination.");
    } else {
        println!(">>> Mirrored directory skeleton: {mirrored} directory/directories.");
    }

    let mut moved = 0usize;
    let mut skipped = 0usize;
    let mut failed: Vec<(PathBuf, String)> = Vec::new();

    for src_file in &scan.candidates {
        let rel = src_file
            .strip_prefix(src_root)
            .with_context(|| format!("strip_prefix failed for {}", src_file.display()))?;
        let dest_file = dest_root.join(rel);

        if dry_run {
            println!("[DRY-RUN] Would move: {}", rel.display());
            collect_audit("DRY_RUN", &[("path", &rel.display().to_string())]);
            continue;
        }

        if let Some(parent) = dest_file.parent() {
            let _ = ensure_parent_dir(parent);
        }

        if dest_file.exists() {
            println!("   Skipping (Exists at Target): {}", rel.display());
            collect_audit(
                "SKIP",
                &[
                    ("path", &rel.display().to_string()),
                    ("reason", "target_exists"),
                ],
            );
            skipped += 1;
            continue;
        }

        match fs::rename(src_file, &dest_file) {
            Ok(()) => {
                println!("   Moved: {}", rel.display());
                collect_audit("MOVED", &[("path", &rel.display().to_string())]);
                moved += 1;
            }
            Err(e) => {
                // Cross-device move: fall back to copy + delete
                if e.raw_os_error() == Some(libc::EXDEV) {
                    match fs::copy(src_file, &dest_file).and_then(|_| fs::remove_file(src_file)) {
                        Ok(()) => {
                            println!("   Moved (cross-device): {}", rel.display());
                            collect_audit("MOVED", &[("path", &rel.display().to_string())]);
                            moved += 1;
                        }
                        Err(e2) => {
                            let msg = e2.to_string();
                            eprintln!("   FAILED: {} -> {}", rel.display(), msg);
                            let trunc: String = msg.replace('\n', " ").chars().take(200).collect();
                            collect_audit(
                                "FAIL",
                                &[("path", &rel.display().to_string()), ("error", &trunc)],
                            );
                            failed.push((rel.to_path_buf(), msg));
                        }
                    }
                } else {
                    let msg = e.to_string();
                    eprintln!("   FAILED: {} -> {}", rel.display(), msg);
                    let trunc: String = msg.replace('\n', " ").chars().take(200).collect();
                    collect_audit(
                        "FAIL",
                        &[("path", &rel.display().to_string()), ("error", &trunc)],
                    );
                    failed.push((rel.to_path_buf(), msg));
                }
            }
        }
    }

    if !dry_run {
        removed_empty = prune_empty_source_directories(&dir_meta);
        if removed_empty > 0 {
            println!(">>> Removed {removed_empty} empty source directory/directories.");
        }
        println!(">>> Restoring metadata for all mirrored directories...");
        restore_directory_times(src_root, dest_root, &dir_meta)?;
    }

    println!("\n--- COLLECTION SUMMARY ---");
    println!("Total Candidate Files:           {}", scan.candidates.len());
    println!("Candidate JXL Images:            {}", scan.image_count);
    println!("Candidate HEVC Videos:           {}", scan.video_count);
    println!("Mirrored Directories:            {mirrored}");
    println!("Successfully Relocated:          {moved}");
    println!("Skipped (Target Exists):         {skipped}");
    println!("Skipped (Symlinks):              {}", scan.symlink_count);
    println!(
        "Video Probe Failures:            {}",
        scan.probe_failures.len()
    );
    println!("Removed Empty Source Dirs:       {removed_empty}");

    if !failed.is_empty() {
        println!("Failed Relocations: {}", failed.len());
        for (p, e) in &failed {
            println!("  - {}: {}", p.display(), e);
        }
    }

    collect_audit(
        "COMPLETE",
        &[
            ("moved", &moved.to_string()),
            ("skipped", &skipped.to_string()),
            ("failed", &failed.len().to_string()),
            ("candidates", &scan.candidates.len().to_string()),
            ("dry_run", if dry_run { "1" } else { "0" }),
        ],
    );

    if dry_run {
        println!("Dry run complete. No changes were made.");
        return Ok(true);
    }

    println!(
        "Operation finished. Optimized files moved, legacy files retained, \
         empty source directories removed, directory tree mirrored."
    );
    Ok(failed.is_empty())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let src = args
        .source
        .canonicalize()
        .with_context(|| format!("resolve source path: {}", args.source.display()))?;
    let dest = if args.destination.exists() {
        args.destination
            .canonicalize()
            .with_context(|| format!("resolve dest path: {}", args.destination.display()))?
    } else {
        args.destination.clone()
    };

    let ok = run_collection(&src, &dest, args.dry_run, args.yes)?;
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_validate_paths_rejects_missing_source() {
        let dest = tempfile::tempdir().unwrap();
        assert!(!validate_paths(Path::new("/nonexistent/path"), dest.path()));
    }

    #[test]
    fn test_validate_paths_rejects_dest_inside_src() {
        let src = tempfile::tempdir().unwrap();
        let dest = src.path().join("sub");
        fs::create_dir_all(&dest).unwrap();
        // After canonicalization dest starts_with src — should reject
        assert!(!validate_paths(src.path(), &dest));
    }

    #[test]
    fn test_validate_paths_rejects_missing_dest_inside_src() {
        let src = tempfile::tempdir().unwrap();
        let dest = src.path().join("new_collect_dest");
        assert!(!validate_paths(src.path(), &dest));
    }

    #[test]
    fn test_snapshot_captures_all_dirs() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("a/b"))?;
        let snap = snapshot_directories(root.path())?;
        assert!(snap.contains_key(root.path()), "root must be snapshotted");
        assert!(snap.len() >= 3, "root + a + a/b");
        Ok(())
    }

    #[test]
    fn test_snapshot_directories_fails_closed_on_missing_root() {
        let missing = Path::new("/nonexistent/mfb_collect_snapshot_root");
        let err = snapshot_directories(missing)
            .expect_err("missing source must fail closed during snapshot");
        assert!(
            err.to_string().contains("walk"),
            "expected walk failure context, got {err:?}"
        );
    }

    #[test]
    fn test_scan_candidates_finds_jxl() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("img.jxl"), b"jxl")?;
        fs::write(root.path().join("img.jpg"), b"jpg")?;
        let scan = scan_candidates(root.path())?;
        assert_eq!(scan.image_count, 1);
        assert_eq!(scan.candidates.len(), 1);
        Ok(())
    }

    #[test]
    fn test_ensure_destination_layout_creates_dirs() -> Result<()> {
        let src = tempfile::tempdir()?;
        let dest = tempfile::tempdir()?;
        fs::create_dir_all(src.path().join("a/b"))?;
        let snap = snapshot_directories(src.path())?;
        let created = ensure_destination_layout(src.path(), dest.path(), &snap, false)?;
        assert!(created >= 2, "should create a/ and a/b/ at dest");
        assert!(dest.path().join("a/b").is_dir());
        Ok(())
    }

    #[test]
    fn test_prune_removes_empty_dirs() -> Result<()> {
        let root = tempfile::tempdir()?;
        let empty = root.path().join("empty_dir");
        let filled = root.path().join("filled_dir");
        fs::create_dir_all(&empty)?;
        fs::create_dir_all(&filled)?;
        fs::write(filled.join("file.txt"), b"data")?;

        let mut snap = DirTimestamps::new();
        snap.insert(
            empty.clone(),
            (SystemTime::UNIX_EPOCH, SystemTime::UNIX_EPOCH),
        );
        snap.insert(
            filled.clone(),
            (SystemTime::UNIX_EPOCH, SystemTime::UNIX_EPOCH),
        );

        let removed = prune_empty_source_directories(&snap);
        assert_eq!(removed, 1, "only empty_dir should be removed");
        assert!(!empty.exists());
        assert!(filled.exists());
        Ok(())
    }

    #[test]
    fn test_dry_run_moves_nothing() -> Result<()> {
        let _guard = env_lock().lock().unwrap();
        let src = tempfile::tempdir()?;
        let dest = tempfile::tempdir()?;
        fs::write(src.path().join("img.jxl"), b"jxl")?;

        let ok = run_collection(src.path(), dest.path(), true, true)?;
        assert!(ok);
        // No files should have been moved
        assert!(!dest.path().join("img.jxl").exists());
        assert!(src.path().join("img.jxl").exists());
        Ok(())
    }

    #[test]
    fn test_collect_audit_honors_session_audit_env() -> Result<()> {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir()?;
        let audit = dir.path().join("audit.jsonl");
        unsafe { std::env::set_var("MFB_SESSION_AUDIT", &audit) };

        collect_audit("TEST_EVENT", &[("b", "2"), ("a", "1")]);

        unsafe { std::env::remove_var("MFB_SESSION_AUDIT") };
        let content = fs::read_to_string(audit)?;
        assert!(content.contains("COLLECT_TEST_EVENT a=1 b=2"));
        Ok(())
    }

    #[test]
    fn test_collect_audit_matches_python_plain_line_format() -> Result<()> {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir()?;
        let audit = dir.path().join("audit.log");
        unsafe { std::env::set_var("MFB_SESSION_AUDIT", &audit) };

        collect_audit("PLAIN", &[("path", "img.jxl")]);

        unsafe { std::env::remove_var("MFB_SESSION_AUDIT") };
        let content = fs::read_to_string(audit)?;
        assert!(
            content.contains(" COLLECT_PLAIN path=img.jxl\n"),
            "audit must be timestamp + plain collect line, got {content:?}"
        );
        assert!(
            !content.trim_start().starts_with('{'),
            "collect audit must not switch Python plain-line consumers to JSONL"
        );
        Ok(())
    }

    #[test]
    fn test_skips_existing_dest_file() -> Result<()> {
        let _guard = env_lock().lock().unwrap();
        let src = tempfile::tempdir()?;
        let dest = tempfile::tempdir()?;
        fs::write(src.path().join("img.jxl"), b"src")?;
        fs::write(dest.path().join("img.jxl"), b"already_there")?;

        let ok = run_collection(src.path(), dest.path(), false, true)?;
        assert!(ok);
        // Source still exists (skipped), dest untouched
        assert!(src.path().join("img.jxl").exists());
        assert_eq!(fs::read(dest.path().join("img.jxl"))?, b"already_there");
        Ok(())
    }

    #[test]
    fn test_restore_directory_times_fails_closed_on_setter_error() -> Result<()> {
        let src = tempfile::tempdir()?;
        let dest = tempfile::tempdir()?;
        let child = src.path().join("child");
        fs::create_dir_all(&child)?;
        let snap = snapshot_directories(src.path())?;

        let err = restore_directory_times_with(src.path(), dest.path(), &snap, |path, _, _| {
            anyhow::bail!("restore failed for directory {}", path.display())
        })
        .expect_err("restore must fail when timestamp setter fails");
        assert!(
            err.to_string().contains("restore failed for directory"),
            "expected restore failure context, got {err:?}"
        );
        Ok(())
    }
}
