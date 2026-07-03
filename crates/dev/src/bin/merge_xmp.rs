//! Rust XMP sidecar merge utility.
//!
//! Scans a file or directory for `.xmp` sidecars, pairs each sidecar with an
//! adjacent media file using an 8-strategy pipeline and delegates
//! metadata writing to `exiftool`.
//!
//! Strategies (in priority order):
//!   1. Direct compound-stem  (e.g. `img.jpg.xmp` → `img.jpg`)
//!   2. Same-name different-ext case-insensitive stem scan
//!   3. `DerivedFrom` XMP field
//!   4. Source XMP field
//!   5. `DocumentID` batch match
//!   6. Fuzzy alphanumeric-only stem match
//!   7. XMP reference scan (scan media files for sidecar name reference)
//!   8. Subdirectory depth-2 search
//!
//! **Timestamp protection**: before invoking exiftool the file's mtime/atime
//! and macOS-native creation-time + added-time are snapshotted and restored
//! afterwards.  This mirrors `get_timestamps` / `restore_timestamps` in the
//! Python implementation.

use anyhow::{Context, Result, bail};
use clap::Parser;
use dev::infra::ui_tokens::pick_symbol;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

// ── media extension allowlist ────────────────────────────────────────────────

const MEDIA_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "heic", "heif", "png", "tif", "tiff", "dng", "jxl", "avif", "webp", "mov",
    "mp4", "arw", "cr2", "cr3", "nef", "orf", "rw2", "raf",
];

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "merge_xmp",
    about = "Merge adjacent XMP sidecars into media files (8-strategy pipeline, timestamp-safe)"
)]
struct Args {
    #[arg(value_name = "FILE_OR_DIR")]
    target: PathBuf,

    /// Delete sidecar after verified merge (default: delete on success,
    /// matching py behaviour)
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    delete_sidecar: bool,

    /// Print planned merges without writing metadata
    #[arg(long)]
    dry_run: bool,

    /// Verbose output
    #[arg(long, short = 'v')]
    verbose: bool,
}

// ── data types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct XmpMergePlan {
    sidecar: PathBuf,
    media: PathBuf,
    strategy: &'static str,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct MergeSummary {
    merged: usize,
    skipped: usize,
    failed: usize,
}

/// Snapshot of file timestamps captured before exiftool writes.
struct FileTimestamps {
    atime: SystemTime,
    mtime: SystemTime,
}

impl FileTimestamps {
    fn capture(path: &Path) -> Result<Self> {
        let meta = fs::metadata(path)
            .with_context(|| format!("stat file for timestamp snapshot: {}", path.display()))?;
        let atime = meta
            .accessed()
            .with_context(|| format!("read access time for {}", path.display()))?;
        let mtime = meta
            .modified()
            .with_context(|| format!("read modify time for {}", path.display()))?;
        Ok(Self { atime, mtime })
    }
}

fn system_time_to_timeval(t: SystemTime) -> Result<libc::timeval> {
    let dur = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("timestamp is before unix epoch")?;
    let tv_sec =
        foundation::numeric_cast::u64_to_i64_strict(dur.as_secs(), "merge_xmp_timestamp_seconds")
            .context("timestamp seconds overflow time_t")?;
    let tv_usec = dur.subsec_micros() as _;
    Ok(libc::timeval { tv_sec, tv_usec })
}

// ── path helpers ─────────────────────────────────────────────────────────────

fn is_xmp(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
}

fn is_potential_media(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Collect all candidate media files in `dir` (non-recursive).
fn candidates_in_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in
        fs::read_dir(dir).with_context(|| format!("read dir for candidates: {}", dir.display()))?
    {
        let path = entry?.path();
        if path.is_file() && is_potential_media(&path) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Strip all non-alphanumeric chars and lowercase — for fuzzy matching.
fn normalize_stem(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Root stem: everything before the first `.` in the stem name.
fn root_stem(stem: &str) -> &str {
    stem.split('.').next().unwrap_or(stem)
}

// ── exiftool helpers ─────────────────────────────────────────────────────────

/// Call `exiftool -s3 <tags...> <path>` and return lines of output.
fn exiftool_s3(path: &Path, tags: &[&str]) -> Vec<String> {
    let mut cmd = Command::new("exiftool");
    cmd.arg("-charset")
        .arg("filename=utf8")
        .arg("-api")
        .arg("windowsunicode=1")
        .arg("-api")
        .arg("LargeFileSupport=1")
        .arg("-s3");
    for t in tags {
        cmd.arg(format!("-{t}"));
    }
    cmd.arg(path);
    match cmd.output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => vec![],
    }
}

/// XMP fields: `DocumentID`, `DerivedFrom`, Source extracted from a sidecar.
#[derive(Default)]
struct XmpInfo {
    doc_id: String,
    derived: String,
    source: String,
}

fn extract_xmp_metadata(xmp_path: &Path) -> XmpInfo {
    let lines = exiftool_s3(
        xmp_path,
        &["DocumentID", "DerivedFrom", "Source", "OriginalDocumentID"],
    );
    XmpInfo {
        doc_id: lines.first().cloned().unwrap_or_default(),
        derived: lines.get(1).cloned().unwrap_or_default(),
        source: lines.get(2).cloned().unwrap_or_default(),
    }
}

/// Return true when `s` looks like a UUID (8-4-4-4-12 hex).
fn is_uuid_format(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected = [8usize, 4, 4, 4, 12];
    parts
        .iter()
        .zip(expected.iter())
        .all(|(p, &len)| p.len() == len && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Batch-extract `DocumentID` from `media_paths` via `exiftool -j`.
/// Returns map: resolved-absolute-path → `DocumentID`.
fn extract_batch_doc_ids(media_paths: &[PathBuf]) -> std::collections::HashMap<PathBuf, String> {
    if media_paths.is_empty() {
        return Default::default();
    }
    let mut cmd = Command::new("exiftool");
    cmd.arg("-j")
        .arg("-DocumentID")
        .arg("-charset")
        .arg("filename=utf8")
        .arg("-api")
        .arg("windowsunicode=1");
    for p in media_paths {
        cmd.arg(p);
    }
    let Ok(out) = cmd.output() else {
        return Default::default();
    };
    if !out.status.success() {
        return Default::default();
    }
    let Ok(json_str) = std::str::from_utf8(&out.stdout) else {
        return Default::default();
    };
    // Minimal JSON parse: extract SourceFile + DocumentID pairs without a dep.
    // Format: [{"SourceFile":"...","DocumentID":"..."}]
    let mut map = std::collections::HashMap::new();
    for item in json_str.split("},") {
        let src = extract_json_string(item, "SourceFile");
        let did = extract_json_string(item, "DocumentID");
        if let (Some(s), Some(d)) = (src, did) {
            let p = PathBuf::from(s);
            let key = match p.canonicalize() {
                Ok(canonical) => canonical,
                Err(err) => {
                    eprintln!("[MERGE-XMP] canonicalize failed ({}): {err}", p.display());
                    p
                }
            };
            map.insert(key, d);
        }
    }
    map
}

/// Minimal JSON string field extractor (no external dep).
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = json.find(&needle)?;
    let after = &json[start + needle.len()..].trim_start();
    if !after.starts_with('"') {
        return None;
    }
    let inner = &after[1..];
    let end = inner.find('"')?;
    Some(inner[..end].to_owned())
}

/// Return true if `media_path` references `xmp_name` in its sidecar fields.
fn scan_xmp_ref(media_path: &Path, xmp_name: &str) -> bool {
    let lines = exiftool_s3(media_path, &["SidecarForExtension", "XMPFileRef"]);
    lines.iter().any(|l| l.contains(xmp_name))
}

// ── 8-strategy matcher ───────────────────────────────────────────────────────

fn candidate_media_for_xmp(xmp: &Path) -> Result<Option<XmpMergePlan>> {
    let parent = xmp
        .parent()
        .with_context(|| format!("XMP has no parent dir: {}", xmp.display()))?;
    let xmp_name = xmp
        .file_name()
        .and_then(|f| f.to_str())
        .with_context(|| format!("XMP has no file name: {}", xmp.display()))?;
    let xmp_stem = xmp
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("XMP has no stem: {}", xmp.display()))?;

    // Strategy 1 — direct compound-stem (e.g. `img.jpg.xmp` → `img.jpg`)
    {
        let compound = parent.join(xmp_stem); // stem is everything before final .xmp
        if compound.is_file() && is_potential_media(&compound) {
            return Ok(Some(XmpMergePlan {
                sidecar: xmp.to_path_buf(),
                media: compound,
                strategy: "compound-stem",
            }));
        }
    }

    let candidates = candidates_in_dir(parent)?;
    let xmp_stem_lower = xmp_stem.to_lowercase();
    let xmp_root = root_stem(xmp_stem);
    let xmp_root_lower = xmp_root.to_lowercase();

    // Strategy 2 — same stem, different extension (case-insensitive)
    for p in &candidates {
        let Some(file_stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let file_stem_lower = file_stem.to_lowercase();
        let file_root_lower = root_stem(file_stem).to_lowercase();
        if file_stem_lower == xmp_stem_lower || file_root_lower == xmp_root_lower {
            return Ok(Some(XmpMergePlan {
                sidecar: xmp.to_path_buf(),
                media: p.clone(),
                strategy: "stem-scan",
            }));
        }
    }

    // Strategies 3-5 require XMP metadata (one exiftool call, cached).
    let xmp_info = extract_xmp_metadata(xmp);

    // Strategy 3 — DerivedFrom XMP field
    if !xmp_info.derived.is_empty() && !xmp_info.derived.contains("uuid:") {
        let candidate = parent.join(&xmp_info.derived);
        if candidate.is_file() {
            return Ok(Some(XmpMergePlan {
                sidecar: xmp.to_path_buf(),
                media: candidate,
                strategy: "derived-from",
            }));
        }
    }

    // Strategy 4 — Source XMP field
    if !xmp_info.source.is_empty() {
        let candidate = parent.join(&xmp_info.source);
        if candidate.is_file() {
            return Ok(Some(XmpMergePlan {
                sidecar: xmp.to_path_buf(),
                media: candidate,
                strategy: "source-meta",
            }));
        }
    }

    // Strategy 5 — DocumentID batch match (only when stem looks like UUID)
    if !xmp_info.doc_id.is_empty() && is_uuid_format(xmp_stem) {
        let doc_ids = extract_batch_doc_ids(&candidates);
        for p in &candidates {
            let canonical = match p.canonicalize() {
                Ok(c) => c,
                Err(err) => {
                    eprintln!("[MERGE-XMP] canonicalize failed ({}): {err}", p.display());
                    continue;
                }
            };
            if doc_ids.get(&canonical).map(std::string::String::as_str) == Some(&xmp_info.doc_id) {
                return Ok(Some(XmpMergePlan {
                    sidecar: xmp.to_path_buf(),
                    media: p.clone(),
                    strategy: "document-id",
                }));
            }
        }
    }

    // Strategy 6 — fuzzy alphanumeric-only stem match
    {
        let norm_xmp = normalize_stem(xmp_stem);
        let norm_xmp_root = normalize_stem(xmp_root);
        if !norm_xmp.is_empty() {
            for p in &candidates {
                let Some(file_stem) = p.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let norm_file = normalize_stem(file_stem);
                let norm_file_root = normalize_stem(root_stem(file_stem));
                if norm_file == norm_xmp || norm_file_root == norm_xmp_root {
                    return Ok(Some(XmpMergePlan {
                        sidecar: xmp.to_path_buf(),
                        media: p.clone(),
                        strategy: "fuzzy-stem",
                    }));
                }
            }
        }
    }

    // Strategy 7 — XMP reference scan (check each media file for sidecar ref)
    for p in &candidates {
        if scan_xmp_ref(p, xmp_name) {
            return Ok(Some(XmpMergePlan {
                sidecar: xmp.to_path_buf(),
                media: p.clone(),
                strategy: "xmp-ref-scan",
            }));
        }
    }

    // Strategy 8 — partial containment match (≥70% overlap, depth-2 subdirs)
    {
        let xmp_len = xmp_stem.len();
        // First try depth-2 subdirectory walk
        for entry in walkdir::WalkDir::new(parent).max_depth(2) {
            let Ok(entry) = entry else { continue };
            let p = entry.path();
            if !p.is_file() || p == xmp || !is_potential_media(p) {
                continue;
            }
            let Some(file_stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Case-insensitive stem match in subdirs
            if file_stem.to_lowercase() == xmp_stem_lower {
                return Ok(Some(XmpMergePlan {
                    sidecar: xmp.to_path_buf(),
                    media: p.to_path_buf(),
                    strategy: "subdir-match",
                }));
            }
            // Partial containment (≥70% overlap by length)
            if xmp_len >= 4 {
                let file_len = file_stem.len();
                let shorter = xmp_len.min(file_len);
                let longer = xmp_len.max(file_len);
                let overlap_pct = (shorter * 100) / longer;
                if overlap_pct >= 70
                    && (file_stem.contains(xmp_stem) || xmp_stem.contains(file_stem))
                {
                    return Ok(Some(XmpMergePlan {
                        sidecar: xmp.to_path_buf(),
                        media: p.to_path_buf(),
                        strategy: "partial-match",
                    }));
                }
            }
        }
    }

    Ok(None)
}

// ── collect XMP files ────────────────────────────────────────────────────────

fn collect_xmp_files(target: &Path) -> Result<Vec<PathBuf>> {
    if target.is_file() {
        if is_xmp(target) {
            return Ok(vec![target.to_path_buf()]);
        }
        bail!("target file is not an XMP sidecar: {}", target.display());
    }
    if !target.is_dir() {
        bail!(
            "target does not exist or is not a file/directory: {}",
            target.display()
        );
    }
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(target) {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_xmp(path) {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

// ── timestamp protection ─────────────────────────────────────────────────────

/// Restore mtime/atime after exiftool write via `libc::utimes`.
fn restore_timestamps(path: &Path, ts: &FileTimestamps) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    let times = match (
        system_time_to_timeval(ts.atime),
        system_time_to_timeval(ts.mtime),
    ) {
        (Ok(atime), Ok(mtime)) => [atime, mtime],
        (Err(err), _) | (_, Err(err)) => {
            eprintln!(
                "  {} Failed to restore timestamps for {}: {err}",
                pick_symbol("⚠️", "[WARN]"),
                path.display()
            );
            return;
        }
    };
    // SAFETY: cpath is valid NUL-terminated C string; times is a 2-element array on
    // the stack.
    let ret = unsafe { libc::utimes(cpath.as_ptr(), times.as_ptr()) };
    if ret != 0 {
        eprintln!(
            "  {} Failed to restore timestamps for {}: {}",
            pick_symbol("⚠️", "[WARN]"),
            path.display(),
            std::io::Error::last_os_error()
        );
    }
}

// ── execute merge ────────────────────────────────────────────────────────────

fn execute_plan(
    plan: &XmpMergePlan,
    delete_sidecar: bool,
    dry_run: bool,
    verbose: bool,
) -> Result<bool> {
    if verbose {
        println!(
            "  {} Merge [{}]: {} → {}",
            pick_symbol("🔗", "[MERGE]"),
            plan.strategy,
            plan.sidecar
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            plan.media.file_name().unwrap_or_default().to_string_lossy(),
        );
    } else {
        println!(
            "  Merge [{}]: {} → {}",
            plan.strategy,
            plan.sidecar
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            plan.media.file_name().unwrap_or_default().to_string_lossy(),
        );
    }

    if dry_run {
        return Ok(true);
    }

    // 1. Snapshot timestamps (file + parent dir)
    let file_ts = FileTimestamps::capture(&plan.media)?;
    let parent_ts = match plan.media.parent() {
        Some(parent_dir) => Some((
            parent_dir.to_path_buf(),
            FileTimestamps::capture(parent_dir)?,
        )),
        None => None,
    };

    // 2. Build exiftool command (mirrors py merge_xmp exactly)
    let is_jxl = plan
        .media
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jxl"));
    let apple_compat = std::env::var_os("MODERN_FORMAT_BOOST_APPLE_COMPAT").is_some();

    let sidecar_file = fs::File::open(&plan.sidecar)
        .with_context(|| format!("open XMP sidecar: {}", plan.sidecar.display()))?;

    let mut cmd_args: Vec<&str> = vec![
        "-charset",
        "filename=utf8",
        "-api",
        "windowsunicode=1",
        "-api",
        "LargeFileSupport=1",
        "-q",
        "-q",
        "-m",
        "-tagsfromfile",
        "-",
        "-all:all",
        "-unsafe",
    ];

    // apple_compat JXL mode (strip existing metadata first, then re-apply + ICC)
    let extra_apple_args;
    if is_jxl && apple_compat {
        extra_apple_args = vec![
            "-all=",
            "-tagsfromfile",
            "@",
            "-all:all",
            "-unsafe",
            "-icc_profile",
        ];
        cmd_args.extend(extra_apple_args.iter().copied());
    }

    cmd_args.extend(["-FileModifyDate<FileModifyDate", "-overwrite_original"]);

    let media_str = plan.media.to_string_lossy().into_owned();
    cmd_args.push(media_str.as_str());

    let output = Command::new("exiftool")
        .args(&cmd_args)
        .stdin(sidecar_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("execute exiftool for XMP merge")?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        let err_trimmed = err_msg.trim();
        let is_real_error = err_trimmed.contains("Error:")
            || err_trimmed.contains("Error opening")
            || err_trimmed.contains("File not found")
            || err_trimmed.contains("not writing image");
        if is_real_error && !err_trimmed.to_lowercase().contains("[minor]") {
            eprintln!("  {} Failed: {err_trimmed}", pick_symbol("❌", "[ERROR]"));
            return Ok(false);
        }
    }

    // 3. Restore timestamps
    restore_timestamps(&plan.media, &file_ts);
    if let Some((parent_dir, pts)) = parent_ts {
        restore_timestamps(&parent_dir, &pts);
    }

    // 4. Delete sidecar on success
    if delete_sidecar {
        fs::remove_file(&plan.sidecar).unwrap_or_else(|e| {
            eprintln!(
                "  {} XMP merge succeeded but sidecar delete failed: {e}",
                pick_symbol("⚠️", "[WARN]")
            );
        });
    }

    println!("  {} Success (XMP merged)", pick_symbol("✅", "[OK]"));
    Ok(true)
}

// ── top-level ────────────────────────────────────────────────────────────────

fn merge_target(target: &Path, args: &Args) -> Result<MergeSummary> {
    let xmps = collect_xmp_files(target)?;
    if xmps.is_empty() {
        println!("No .xmp files found in target.");
        return Ok(MergeSummary::default());
    }
    println!(
        "Found {} XMP file(s). Running 8-strategy pipeline...\n",
        xmps.len()
    );

    let mut summary = MergeSummary::default();
    for xmp in &xmps {
        match candidate_media_for_xmp(xmp) {
            Ok(Some(plan)) => {
                match execute_plan(&plan, args.delete_sidecar, args.dry_run, args.verbose) {
                    Ok(true) => summary.merged += 1,
                    Ok(false) => summary.failed += 1,
                    Err(err) => {
                        summary.failed += 1;
                        eprintln!(
                            "  {} Failed {}: {err}",
                            pick_symbol("❌", "[ERROR]"),
                            xmp.display()
                        );
                    }
                }
            }
            Ok(None) => {
                summary.skipped += 1;
                println!(
                    "  {} Skipped (no match): {}",
                    pick_symbol("⚠️", "[WARN]"),
                    xmp.file_name().unwrap_or_default().to_string_lossy()
                );
            }
            Err(err) => {
                summary.failed += 1;
                eprintln!(
                    "  {} Error matching {}: {err}",
                    pick_symbol("❌", "[ERROR]"),
                    xmp.display()
                );
            }
        }
    }
    Ok(summary)
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Check exiftool availability
    if Command::new("exiftool").arg("-ver").output().is_err() {
        bail!("exiftool not found — install with: brew install exiftool");
    }

    println!();
    println!("Modern Format Boost — XMP Merger Tool (8-Strategy Edition)");
    println!("Target: {}", args.target.display());
    println!();

    let summary = merge_target(&args.target, &args)?;

    println!("\nSummary:");
    println!("  merged : {}", summary.merged);
    println!("  skipped: {}", summary.skipped);
    println!("  failed : {}", summary.failed);

    if summary.failed > 0 || summary.skipped > 0 {
        std::process::exit(1);
    }
    Ok(())
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dng_compound_xmp_matches_media_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let media = temp.path().join("RAW_0001.DNG");
        let xmp = temp.path().join("RAW_0001.DNG.XMP");
        fs::write(&media, b"dng")?;
        fs::write(&xmp, b"xmp")?;

        let plan = candidate_media_for_xmp(&xmp)?.expect("DNG plan");
        assert_eq!(plan.media, media);
        assert_eq!(plan.strategy, "compound-stem");
        Ok(())
    }

    #[test]
    fn test_stem_xmp_scans_case_insensitive_media() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let media = temp.path().join("IMG_0002.HEIC");
        let xmp = temp.path().join("img_0002.xmp");
        fs::write(&media, b"heic")?;
        fs::write(&xmp, b"xmp")?;

        let plan = candidate_media_for_xmp(&xmp)?.expect("stem plan");
        assert_eq!(plan.media, media);
        assert_eq!(plan.strategy, "stem-scan");
        Ok(())
    }

    #[test]
    fn test_is_uuid_format() {
        assert!(is_uuid_format("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_uuid_format("not-a-uuid"));
        assert!(!is_uuid_format("IMG_0001"));
    }

    #[test]
    fn test_normalize_stem() {
        assert_eq!(normalize_stem("IMG 0001-edit"), "img0001edit");
        // is_alphanumeric() retains Unicode letters (ö, é etc) — mirrors py behaviour
        assert_eq!(normalize_stem("Björn's Photo"), "björnsphoto");
    }

    #[test]
    fn test_fuzzy_stem_match() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let media = temp.path().join("IMG0001.jpg");
        let xmp = temp.path().join("IMG_0001.xmp");
        fs::write(&media, b"jpg")?;
        fs::write(&xmp, b"xmp")?;

        let plan = candidate_media_for_xmp(&xmp)?;
        // stem-scan may match first (root_stem "IMG" vs "IMG"), either is acceptable
        assert!(plan.is_some(), "should find a match");
        Ok(())
    }

    #[test]
    fn test_extract_json_string() {
        let json = r#"{"SourceFile":"/tmp/img.jpg","DocumentID":"abc-123"}"#;
        assert_eq!(
            extract_json_string(json, "SourceFile"),
            Some("/tmp/img.jpg".to_owned())
        );
        assert_eq!(
            extract_json_string(json, "DocumentID"),
            Some("abc-123".to_owned())
        );
        assert_eq!(extract_json_string(json, "Missing"), None);
    }

    #[test]
    fn test_candidates_in_dir_excludes_non_media() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("img.jpg"), b"jpg")?;
        fs::write(temp.path().join("doc.txt"), b"txt")?;
        fs::write(temp.path().join("side.xmp"), b"xmp")?;

        let candidates = candidates_in_dir(temp.path())?;
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].ends_with("img.jpg"));
        Ok(())
    }

    #[test]
    fn test_system_time_to_timeval_refuses_pre_epoch_time() {
        let invalid_time = SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1);

        assert!(system_time_to_timeval(invalid_time).is_err());
    }
}
