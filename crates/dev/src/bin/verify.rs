//! Modern Format Boost - Diagnostic Analysis and Integrity Verifier in Rust.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::drag_drop::{INTEGRITY_SUMMARY_JSON_PREFIX, IntegritySummaryMachine};
use dev::infra::ui_tokens::pick_symbol;
use dev::media::scope::{
    SKIP_EXTS, classify_missing_entry, detect_true_format, integrity_stem_key,
    load_rust_outcomes_from_logs, load_session_routing, true_format_matches_processing_mode,
};
use foundation::common_utils::calculate_blake3_hash;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "verify",
    about = "MFB Conversion Analyzer & Integrity Verifier"
)]
struct Args {
    #[arg(help = "Log files or directories to scan.")]
    logs: Vec<PathBuf>,

    #[arg(long = "verify", num_args = 1..=2, help = "Source and/or optimized directories for integrity check.")]
    verify: Option<Vec<PathBuf>>,

    #[arg(short = 'o', long = "output", help = "Custom output report path.")]
    output: Option<PathBuf>,

    #[arg(
        long = "print-integrity-summary",
        help = "Print integrity summary to stdout."
    )]
    print_integrity_summary: bool,

    #[arg(
        long = "print-integrity-json",
        hide = true,
        help = "Print the machine-readable integrity result contract."
    )]
    print_integrity_json: bool,

    #[arg(
        long = "fast-img-delivery",
        help = "Verify fast-img post-delivery invariant."
    )]
    fast_img_delivery: bool,

    #[arg(
        long = "fast-img-restore",
        help = "Verify fast-img restore-jpeg invariant."
    )]
    fast_img_restore: bool,

    #[arg(
        long = "fast-img-marker-json",
        help = "Print fast-img marker lookup result as JSON for an optimized directory."
    )]
    fast_img_marker_json: Option<PathBuf>,

    #[arg(long = "mode", value_parser = ["both", "images_only", "videos_only"], default_value = "both", help = "Limit integrity verification.")]
    mode: String,

    #[arg(long = "session-audit", help = "Session verbose log(s).")]
    session_audit: Vec<PathBuf>,

    #[arg(
        long = "strategy",
        default_value = "jxl",
        help = "Encoding strategy: jxl (default) or avif (Meme Mode 表情包模式)."
    )]
    strategy: String,
}

fn file_content_blake3(path: &Path, chunk_size: usize) -> (Option<String>, Option<String>) {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return (None, Some(e.to_string())),
    };
    let mut buf = vec![0u8; chunk_size];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(e) => return (None, Some(e.to_string())),
    };
    buf.truncate(n);
    (Some(blake3::hash(&buf).to_hex().to_string()), None)
}

fn collect_media_files(
    directory: &Path,
    processing_mode: &str,
) -> Result<BTreeMap<String, Vec<PathBuf>>> {
    let mut result: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let canonical_dir = directory.canonicalize().context("canonicalize directory")?;
    for entry in walkdir::WalkDir::new(&canonical_dir) {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => return Err(anyhow::anyhow!("Walkdir error: {err}")),
        };
        let full = entry.path();
        if full.is_file() {
            let fname = match full.file_name().and_then(|f| f.to_str()) {
                Some(f) => f,
                None => continue,
            };
            if fname.starts_with('.') {
                continue;
            }
            if let Some(ext) = full.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_lowercase();
                if SKIP_EXTS.contains(&ext_lower.as_str()) {
                    continue;
                }
            }
            let true_format = detect_true_format(full)?;
            if true_format == "unknown" {
                continue;
            }
            if !true_format_matches_processing_mode(full, &true_format, processing_mode)? {
                continue;
            }
            let rel = match full.strip_prefix(&canonical_dir) {
                Ok(r) => r,
                Err(err) => return Err(anyhow::anyhow!("Strip prefix error: {err}")),
            };
            let key = integrity_stem_key(rel);
            result.entry(key).or_default().push(full.to_path_buf());
        }
    }
    Ok(result)
}

fn collect_regular_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let canonical = directory.canonicalize().context("canonicalize")?;
    for entry in walkdir::WalkDir::new(&canonical) {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => return Err(anyhow::anyhow!("Walkdir error: {err}")),
        };
        if entry.path().is_file() {
            result.push(entry.path().to_path_buf());
        }
    }
    Ok(result)
}

fn is_true_jpeg_file(path: &Path) -> Result<bool> {
    Ok(detect_true_format(path)? == "jpeg")
}

fn resolve_verify_dirs(verify_args: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    if verify_args.len() >= 2 {
        return Some((verify_args[0].clone(), verify_args[1].clone()));
    }
    if verify_args.len() == 1 {
        let path = &verify_args[0];
        let name = path.file_name()?.to_str()?;
        if name.ends_with("_optimized") {
            let src_name = name.strip_suffix("_optimized")?;
            let src_path = path.parent()?.join(src_name);
            if src_path.is_dir() {
                return Some((src_path, path.clone()));
            }
        } else if name.ends_with("_opt") {
            let src_name = name.strip_suffix("_opt")?;
            let src_path = path.parent()?.join(src_name);
            if src_path.is_dir() {
                return Some((src_path, path.clone()));
            }
        }
        for suffix in &["_optimized", "_opt"] {
            let opt_name = format!("{name}{suffix}");
            let opt_path = path.parent()?.join(opt_name);
            if opt_path.is_dir() {
                return Some((path.clone(), opt_path));
            }
        }
    }
    None
}

fn choose_primary_output(paths: &[PathBuf]) -> Result<PathBuf> {
    let priority = [
        "jxl", "avif", "webp", "heic", "heif", "png", "jpeg", "gif", "mp4", "mov", "webm",
    ];
    let mut detected = HashMap::new();
    for path in paths {
        let fmt = detect_true_format(path)?;
        detected.insert(path.clone(), fmt);
    }
    for true_format in &priority {
        for pp in paths {
            if detected.get(pp).map(String::as_str) == Some(*true_format) {
                return Ok(pp.clone());
            }
        }
    }
    let mut min_path = match paths.first() {
        Some(p) => p.clone(),
        None => return Err(anyhow::anyhow!("Empty paths list")),
    };
    let mut min_size = u64::MAX;
    for pp in paths {
        let metadata = fs::metadata(pp).context("get metadata")?;
        if metadata.len() < min_size {
            min_size = metadata.len();
            min_path = pp.clone();
        }
    }
    Ok(min_path)
}

#[derive(serde::Serialize, Default)]
struct IntegrityStats {
    source: String,
    optimized: String,
    scope: String,
    source_files: usize,
    optimized_files: usize,
    matched: usize,
    ambiguous: usize,
    missing: usize,
    extra: usize,
    mismatched_types: usize,
    integrity_failures: usize,
    has_warnings: bool,
    source_total_size: u64,
    optimized_total_size: u64,
    pipeline_handoff: usize,
    vid_pipeline_failed: usize,
    vid_pipeline_unverified: usize,
    count_delta: isize,
    expected_count_delta: isize,
    count_matches_with_handoff: bool,
    count_fully_explained: bool,
    explained_gaps: usize,
    skipped_sources: usize,
    failed_sources: usize,
    source_remaining_files: usize,
    optimized_path_label: String,
    source_files_label: String,
    optimized_files_label: String,
    source_probe_errors: usize,
    optimized_probe_errors: usize,
    restore_manifest_errors: usize,
    verified_deleted_sources: usize,
    count_status_label: Option<String>,
    tier2_recorded: usize,
    tier2_verified_deleted: usize,
}

fn same_path(p1: &Path, p2: &Path) -> bool {
    let c1 = match p1.canonicalize() {
        Ok(c) => c,
        Err(_) => p1.to_path_buf(),
    };
    let c2 = match p2.canonicalize() {
        Ok(c) => c,
        Err(_) => p2.to_path_buf(),
    };
    c1 == c2
}

fn fast_img_marker_candidates(optimized_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut list = Vec::new();
    let marker_dir = foundation::process_lock::get_mfb_root()
        .map_err(|e| format!("MFB state root unavailable: {e}"))?
        .join("fast_img")
        .join("markers");
    if marker_dir.is_dir() {
        let entries = fs::read_dir(&marker_dir)
            .map_err(|e| format!("fast-img marker dir unreadable: {marker_dir:?}: {e}"))?;
        let mut state_markers = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| format!("fast-img marker dir unreadable: {marker_dir:?}: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                state_markers.push(path);
            }
        }
        state_markers.sort();
        list.extend(state_markers);
    }
    list.push(optimized_dir.join(".mfb_wc"));
    list.push(optimized_dir.join("fastmode_img_marker.json"));
    if let Some(parent) = optimized_dir.parent() {
        list.push(parent.join("fastmode_img_marker.json"));
    }
    Ok(list)
}

fn load_fast_img_marker_for_optimized(
    optimized_dir: &Path,
) -> (Option<serde_json::Value>, Option<PathBuf>, Option<String>) {
    let candidates = match fast_img_marker_candidates(optimized_dir) {
        Ok(candidates) => candidates,
        Err(err) => return (None, None, Some(err)),
    };
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                return (
                    None,
                    Some(path.clone()),
                    Some(format!("fast-img marker unreadable: {path:?}: {e}")),
                );
            }
        };
        let val: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                if path.file_name().and_then(|f| f.to_str()) == Some(".mfb_wc") {
                    return (
                        None,
                        Some(path.clone()),
                        Some(format!("fast-img marker unreadable: {path:?}: {e}")),
                    );
                }
                continue;
            }
        };
        let working_copy = match val.get("working_copy").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        if !same_path(Path::new(working_copy), optimized_dir) {
            continue;
        }
        let src_jpeg_count = val
            .get("src_jpeg_count")
            .and_then(serde_json::Value::as_i64);
        match src_jpeg_count {
            Some(c) if c >= 0 => {}
            _ => {
                return (
                    None,
                    Some(path.clone()),
                    Some(format!(
                        "fast-img marker has invalid src_jpeg_count: {path:?}"
                    )),
                );
            }
        }
        if let Some(skipped) = val.get("skipped_sources")
            && !skipped.is_object()
        {
            return (
                None,
                Some(path.clone()),
                Some(format!(
                    "fast-img marker has invalid skipped_sources: {path:?}"
                )),
            );
        }
        if let Some(failed) = val.get("failed_sources")
            && !failed.is_object()
        {
            return (
                None,
                Some(path.clone()),
                Some(format!(
                    "fast-img marker has invalid failed_sources: {path:?}"
                )),
            );
        }
        return (Some(val), Some(path), None);
    }
    (
        None,
        None,
        Some("fast-img marker missing for optimized directory".to_string()),
    )
}

fn print_fast_img_marker_json(optimized_dir: &Path) -> Result<()> {
    let (marker, marker_path, marker_error) = load_fast_img_marker_for_optimized(optimized_dir);
    let payload = serde_json::json!({
        "marker": marker,
        "marker_path": marker_path.map(|p| p.to_string_lossy().to_string()),
        "marker_error": marker_error,
    });
    println!("{}", serde_json::to_string(&payload)?);
    Ok(())
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        anyhow::bail!("invalid odd-length hex");
    }
    let mut bytes = Vec::new();
    let mut chars = s.chars();
    while let (Some(c1), Some(c2)) = (chars.next(), chars.next()) {
        let b1 = c1
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("invalid hex"))? as u8;
        let b2 = c2
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("invalid hex"))? as u8;
        bytes.push((b1 << 4) | b2);
    }
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestoreJpegManifestRecord {
    source_rel: String,
    output_rel: String,
    source_blake3: String,
    output_blake3: String,
    xmp_rel: Option<String>,
    xmp_blake3: Option<String>,
    source_deleted: bool,
}

fn is_safe_manifest_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn load_restore_jpeg_manifest(
    restored_dir: &Path,
) -> (Vec<RestoreJpegManifestRecord>, Vec<String>) {
    let manifest = restored_dir.join(".mfb_restore_jpeg_manifest.tsv");
    if !manifest.is_file() {
        return (Vec::new(), Vec::new());
    }
    let content = match fs::read_to_string(&manifest) {
        Ok(c) => c,
        Err(e) => {
            return (
                Vec::new(),
                vec![format!("restore manifest unreadable: {manifest:?}: {e}")],
            );
        }
    };
    let mut records = Vec::new();
    let mut errors = Vec::new();
    for (line_no, raw_line) in content.lines().enumerate() {
        let line_idx = line_no + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("source_rel_hex\toutput_rel_hex\t") {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() != 5 && parts.len() != 7 && parts.len() != 11 && parts.len() != 12 {
            errors.push(format!(
                "line {line_idx}: expected 5, 7, 11, or legacy 12 TSV fields, got {}",
                parts.len()
            ));
            continue;
        }
        let source_rel_hex = parts[0];
        let output_rel_hex = parts[1];
        let source_hash = parts[2];
        let (reconstruction_hash, output_hash, xmp_rel_hex, xmp_hash, source_deleted) =
            if parts.len() == 11 || parts.len() == 12 {
                let source_deleted_index = if parts.len() == 11 { 10 } else { 11 };
                (
                    parts[3],
                    parts[4],
                    parts[5],
                    parts[6],
                    parts[source_deleted_index],
                )
            } else if parts.len() == 7 {
                (parts[3], parts[3], parts[4], parts[5], parts[6])
            } else {
                (parts[3], parts[3], "", "", parts[4])
            };
        if parts.len() == 11 || parts.len() == 12 {
            if reconstruction_hash != output_hash {
                errors.push(format!(
                    "line {line_idx}: reconstruction and restored JPEG hashes differ"
                ));
                continue;
            }
            match parts[7].parse::<u64>() {
                Ok(value) if value > 0 => {}
                Ok(_) => {
                    errors.push(format!(
                        "line {line_idx}: verified_unix_seconds must be a positive integer"
                    ));
                    continue;
                }
                Err(error) => {
                    errors.push(format!(
                        "line {line_idx}: verified_unix_seconds is not a valid integer: {error}"
                    ));
                    continue;
                }
            }
            if parts[8].trim().is_empty() {
                errors.push(format!("line {line_idx}: missing MFB version"));
                continue;
            }
            let decode_optional_hex = |value: &str| {
                if value.is_empty() {
                    return Ok(String::new());
                }
                hex_decode(value)
                    .and_then(|bytes| String::from_utf8(bytes).map_err(anyhow::Error::from))
            };
            match decode_optional_hex(parts[9]) {
                Ok(version) if !version.trim().is_empty() => {}
                Ok(_) => {
                    errors.push(format!("line {line_idx}: missing djxl version"));
                    continue;
                }
                Err(_) => {
                    errors.push(format!("line {line_idx}: invalid djxl version hex/UTF-8"));
                    continue;
                }
            }
            if parts.len() == 12 && !parts[10].is_empty() && decode_optional_hex(parts[10]).is_err()
            {
                errors.push(format!("line {line_idx}: invalid Photos UUID hex/UTF-8"));
                continue;
            }
        };

        let source_rel = match hex_decode(source_rel_hex)
            .map_err(|_e| ())
            .and_then(|b| String::from_utf8(b).map_err(|_e| ()))
        {
            Ok(s) => s,
            Err(_err) => {
                errors.push(format!(
                    "line {line_idx}: invalid source_rel hex/UTF-8 field"
                ));
                continue;
            }
        };
        let output_rel = match hex_decode(output_rel_hex)
            .map_err(|_e| ())
            .and_then(|b| String::from_utf8(b).map_err(|_e| ()))
        {
            Ok(s) => s,
            Err(_err) => {
                errors.push(format!(
                    "line {line_idx}: invalid output_rel hex/UTF-8 field"
                ));
                continue;
            }
        };
        let (xmp_rel, xmp_blake3) = if xmp_rel_hex.is_empty() && xmp_hash.is_empty() {
            (None, None)
        } else if xmp_rel_hex.is_empty() || xmp_hash.trim().is_empty() {
            errors.push(format!(
                "line {line_idx}: XMP path and hash fields must both be present"
            ));
            continue;
        } else {
            let xmp_rel = match hex_decode(xmp_rel_hex)
                .map_err(|_e| ())
                .and_then(|b| String::from_utf8(b).map_err(|_e| ()))
            {
                Ok(value) => value,
                Err(_err) => {
                    errors.push(format!("line {line_idx}: invalid xmp_rel hex/UTF-8 field"));
                    continue;
                }
            };
            (Some(xmp_rel), Some(xmp_hash.to_string()))
        };
        let source_deleted = match source_deleted {
            "true" => true,
            "false" => false,
            _ => {
                errors.push(format!(
                    "line {line_idx}: source_deleted must be true or false"
                ));
                continue;
            }
        };
        if !is_safe_manifest_relative_path(&source_rel)
            || !is_safe_manifest_relative_path(&output_rel)
            || xmp_rel
                .as_deref()
                .is_some_and(|path| !is_safe_manifest_relative_path(path))
        {
            errors.push(format!(
                "line {line_idx}: manifest paths must be safe relative paths"
            ));
            continue;
        }
        if source_hash.trim().is_empty() || output_hash.trim().is_empty() {
            errors.push(format!("line {line_idx}: missing manifest hash field"));
            continue;
        }
        records.push(RestoreJpegManifestRecord {
            source_rel,
            output_rel,
            source_blake3: source_hash.to_string(),
            output_blake3: output_hash.to_string(),
            xmp_rel,
            xmp_blake3,
            source_deleted,
        });
    }
    (records, errors)
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn resolve_source_directory_for_post_cleanup(source_dir: &Path) -> Result<(PathBuf, bool)> {
    match fs::metadata(source_dir) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                anyhow::bail!("source_dir is not a directory: {}", source_dir.display());
            }
            Ok((
                source_dir
                    .canonicalize()
                    .context("canonicalize source_dir")?,
                true,
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(source_dir) {
                Ok(_) => anyhow::bail!(
                    "source_dir is a dangling symlink or unavailable filesystem object: {}",
                    source_dir.display()
                ),
                Err(link_error) if link_error.kind() == std::io::ErrorKind::NotFound => {
                    Ok((source_dir.to_path_buf(), false))
                }
                Err(link_error) => Err(link_error).with_context(|| {
                    format!("inspect missing source_dir {}", source_dir.display())
                }),
            }
        }
        Err(error) => {
            Err(error).with_context(|| format!("inspect source_dir {}", source_dir.display()))
        }
    }
}

fn run_fast_img_delivery_check(
    source_dir: &Path,
    optimized_dir: &Path,
    report: &mut String,
    processing_mode: &str,
    strategy: &str,
) -> Result<IntegrityStats> {
    let (source_dir, source_dir_exists) = resolve_source_directory_for_post_cleanup(source_dir)?;
    let optimized_dir = if optimized_dir.exists() {
        optimized_dir
            .canonicalize()
            .context("canonicalize optimized_dir")?
    } else {
        optimized_dir.to_path_buf()
    };

    report.push_str("── FAST-IMG DELIVERY VERIFICATION ─────────────────────────────\n");
    report.push_str(&format!("Source:    {}\n", source_dir.display()));
    report.push_str(&format!("Optimized: {}\n\n", optimized_dir.display()));

    let mut stats = IntegrityStats {
        source: source_dir.to_string_lossy().to_string(),
        optimized: optimized_dir.to_string_lossy().to_string(),
        scope: processing_mode.to_string(),
        optimized_path_label: "Optimized".to_string(),
        source_files_label: if strategy == "avif" {
            "Recorded source static images".to_string()
        } else {
            "Recorded source JPEGs".to_string()
        },
        optimized_files_label: if strategy == "avif" {
            "Optimized AVIF files".to_string()
        } else {
            "Optimized JXL files".to_string()
        },
        ..Default::default()
    };

    let mut source_true_jpegs = Vec::new();
    let mut source_probe_errors = Vec::new();
    let source_files = if source_dir_exists {
        collect_regular_files(&source_dir)?
    } else {
        Vec::new()
    };
    for path in source_files {
        if path
            .file_name()
            .and_then(|f| f.to_str())
            .is_none_or(|s| s.starts_with('.'))
        {
            continue;
        }
        match is_true_jpeg_file(&path) {
            Ok(true) => {
                source_true_jpegs.push(path);
            }
            Ok(false) => {}
            Err(e) => {
                source_probe_errors.push((path, e));
            }
        }
    }

    let mut optimized_outputs = Vec::new();
    let mut unexpected_outputs = Vec::new();
    let mut optimized_probe_errors = Vec::new();
    let optimized_files = if optimized_dir.exists() {
        collect_regular_files(&optimized_dir)?
    } else {
        Vec::new()
    };
    for path in optimized_files {
        if let Some(name) = path.file_name().and_then(|f| f.to_str()) {
            if name.starts_with('.') || name == "fastmode_img_marker.json" || name == ".mfb_wc" {
                continue;
            }
        } else {
            continue;
        }
        match detect_true_format(&path) {
            Ok(true_format) => {
                if true_format == strategy {
                    optimized_outputs.push(path);
                } else {
                    unexpected_outputs.push((path, true_format));
                }
            }
            Err(e) => {
                optimized_probe_errors.push((path, e));
            }
        }
    }

    let (marker_opt, _path_opt, marker_error) = load_fast_img_marker_for_optimized(&optimized_dir);
    let mut recorded_source_jpegs: u64 = 0;
    let mut skipped_source_rels = HashSet::new();
    let mut failed_source_rels = HashSet::new();
    let mut successful_source_rels = HashSet::new();
    let mut skipped_sources = HashMap::new();
    let mut failed_sources = HashMap::new();

    let mut tier2_recorded = 0;
    let mut tier2_verified_deleted = 0;
    let mut tier2_unexpected_remaining = Vec::new();
    let mut tier2_missing_proof = Vec::new();

    if let Some(ref m) = marker_opt {
        recorded_source_jpegs = m
            .get("src_jpeg_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        if let Some(skipped) = m.get("skipped_sources").and_then(|v| v.as_object()) {
            for (k, v) in skipped {
                skipped_source_rels.insert(k.clone());
                skipped_sources.insert(k.clone(), v.clone());
            }
        }
        if let Some(failed) = m.get("failed_sources").and_then(|v| v.as_object()) {
            for (k, v) in failed {
                failed_source_rels.insert(k.clone());
                failed_sources.insert(k.clone(), v.clone());
            }
        }
        if let Some(encoded) = m.get("blake3_log").and_then(|v| v.as_object()) {
            successful_source_rels.extend(encoded.keys().cloned());
        }
        if let Some(arr) = m.get("tier2_imported_assets").and_then(|v| v.as_array()) {
            tier2_recorded = arr.len();
            for item in arr {
                let Some(rel) = item.get("rel_path").and_then(|v| v.as_str()) else {
                    continue;
                };
                let photos_uuid = item.get("photos_uuid").and_then(|v| v.as_str());
                let source_path = source_dir.join(rel);
                let exists = source_path.exists();

                if exists {
                    tier2_unexpected_remaining.push(rel.to_string());
                } else if photos_uuid.is_some() && !photos_uuid.unwrap().is_empty() {
                    tier2_verified_deleted += 1;
                } else {
                    tier2_missing_proof.push(rel.to_string());
                }
            }
        }
    }

    if strategy == "avif" {
        source_true_jpegs.clear();
        successful_source_rels
            .iter()
            .chain(&skipped_source_rels)
            .chain(&failed_source_rels)
            .map(|rel| source_dir.join(rel))
            .filter(|path| path.is_file())
            .for_each(|path| source_true_jpegs.push(path));
        source_true_jpegs.sort();
        source_true_jpegs.dedup();
    }

    let mut skipped_sources_present = Vec::new();
    let mut failed_sources_present = Vec::new();
    let mut unexpected_source_true_jpegs = Vec::new();
    for path in &source_true_jpegs {
        let rel = path
            .strip_prefix(&source_dir)?
            .to_string_lossy()
            .to_string();
        if skipped_source_rels.contains(&rel) {
            skipped_sources_present.push(path.clone());
        } else if failed_source_rels.contains(&rel) {
            failed_sources_present.push(path.clone());
        } else {
            unexpected_source_true_jpegs.push(path.clone());
        }
    }

    let mut skipped_sources_missing = Vec::new();
    for rel in &skipped_source_rels {
        if !source_dir.join(rel).is_file() {
            skipped_sources_missing.push(rel.clone());
        }
    }
    skipped_sources_missing.sort();

    let mut failed_sources_missing = Vec::new();
    for rel in &failed_source_rels {
        if !source_dir.join(rel).is_file() {
            failed_sources_missing.push(rel.clone());
        }
    }
    failed_sources_missing.sort();

    let mut optimized_size = 0u64;
    for path in &optimized_outputs {
        match std::fs::metadata(path) {
            Ok(meta) => optimized_size += meta.len(),
            Err(err) => {
                optimized_probe_errors.push((
                    path.clone(),
                    anyhow::anyhow!("failed to stat optimized file: {err}"),
                ));
            }
        }
    }

    let expected_delivery_count = (recorded_source_jpegs as usize)
        .saturating_sub(skipped_sources.len())
        .saturating_sub(failed_sources.len());

    let mut marker_ok = false;
    let mut marker_status = "MISSING";
    if marker_opt.is_some() {
        if marker_error.is_none() {
            marker_ok = true;
            marker_status = "VALID";
        } else {
            marker_status = "INVALID";
        }
    }

    stats.source_files = recorded_source_jpegs as usize;
    stats.optimized_files = optimized_outputs.len();
    stats.skipped_sources = skipped_source_rels.len();
    stats.failed_sources = failed_source_rels.len();
    stats.source_remaining_files = source_true_jpegs.len();
    stats.source_probe_errors = source_probe_errors.len();
    stats.optimized_probe_errors = optimized_probe_errors.len();
    stats.extra = unexpected_outputs.len() + optimized_probe_errors.len();
    stats.count_delta = (optimized_outputs.len() as isize) - (expected_delivery_count as isize);
    stats.matched = optimized_outputs.len();
    stats.explained_gaps = skipped_source_rels.len() + failed_source_rels.len();
    stats.optimized_total_size = optimized_size;
    stats.tier2_recorded = tier2_recorded;
    stats.tier2_verified_deleted = tier2_verified_deleted;

    let mut integrity_failures = 0;
    if !marker_ok {
        integrity_failures += 1;
    }
    if !unexpected_source_true_jpegs.is_empty() {
        integrity_failures += unexpected_source_true_jpegs.len();
    }
    if !unexpected_outputs.is_empty() {
        integrity_failures += unexpected_outputs.len();
    }
    if marker_opt.is_some() && expected_delivery_count != optimized_outputs.len() {
        integrity_failures += 1;
    }
    if optimized_outputs.is_empty() && expected_delivery_count > 0 {
        integrity_failures += 1;
    }
    if !skipped_sources_missing.is_empty() {
        integrity_failures += skipped_sources_missing.len();
    }
    if !failed_sources_missing.is_empty() {
        integrity_failures += failed_sources_missing.len();
    }
    if !tier2_missing_proof.is_empty() {
        integrity_failures += tier2_missing_proof.len();
    }
    if !tier2_unexpected_remaining.is_empty() {
        integrity_failures += tier2_unexpected_remaining.len();
    }
    integrity_failures += source_probe_errors.len() + optimized_probe_errors.len();

    // Ponytail: explicit warning when nothing was produced because all sources were
    // skipped/failed.
    if expected_delivery_count == 0
        && recorded_source_jpegs > 0
        && (!skipped_sources.is_empty() || !failed_sources.is_empty())
    {
        // Surface a visible warning in the report while preserving the programmatic
        // integrity outcome (some callers/tests expect zero integrity failures
        // when all sources were intentionally skipped). Avoid modifying
        // stats.has_warnings to keep existing programmatic behavior/tests stable.
        report.push_str(
            "\nWARNING: No optimized outputs produced because all recorded source files were \
             skipped or failed.\n",
        );
        report.push_str(
            "  Review per-file skip/failure reasons above; this often indicates bulk decode \
             failures (truncated/CMYK) or missing helper tools.\n\n",
        );
    }

    let count_matches = integrity_failures == 0;
    stats.count_matches_with_handoff = count_matches;
    stats.count_fully_explained = count_matches;
    stats.count_status_label = if count_matches {
        Some(if strategy == "avif" {
            "FAST_IMG_AVIF_MEME_DELIVERY".to_string()
        } else {
            "FAST_IMG_JXL_ONLY_DELIVERY".to_string()
        })
    } else {
        None
    };

    report.push_str(&format!(
        "Working copy marker:      {} ({})\n",
        pick_symbol("📊", "[MARKER]"),
        marker_status
    ));
    if let Some(ref err) = marker_error {
        report.push_str(&format!("  Error:                  {err}\n"));
    }
    let source_kind = if strategy == "avif" {
        "static images"
    } else {
        "JPEGs"
    };
    report.push_str(&format!(
        "Recorded source {source_kind}:       {recorded_source_jpegs}\n"
    ));
    report.push_str(&format!(
        "Recorded skipped sources:    {}\n",
        skipped_source_rels.len()
    ));
    report.push_str(&format!(
        "Recorded failed sources:     {}\n",
        failed_source_rels.len()
    ));
    let ext_name = if strategy == "avif" { "AVIF" } else { "JXL" };
    report.push_str(&format!(
        "Expected optimized {ext_name}s:     {expected_delivery_count}\n"
    ));
    report.push_str(&format!(
        "Optimized {ext_name} files:         {}\n",
        optimized_outputs.len()
    ));
    report.push_str(&format!(
        "Source probe errors:         {}\n",
        source_probe_errors.len()
    ));
    report.push_str(&format!(
        "Optimized probe errors:      {}\n",
        optimized_probe_errors.len()
    ));
    report.push_str(&format!(
        "Unexpected optimized files:  {}\n",
        unexpected_outputs.len()
    ));
    if tier2_recorded > 0 {
        report.push_str(&format!(
            "Recorded tier-2 lossy files: {}\n",
            tier2_recorded
        ));
        report.push_str(&format!(
            "Verified tier-2 deleted:     {}\n",
            tier2_verified_deleted
        ));
    }
    report.push('\n');

    if integrity_failures > 0 {
        stats.has_warnings = true;
        stats.integrity_failures = integrity_failures;
        report.push_str(&format!(
            "{} INVARIANT VIOLATIONS DETECTED (Unsafe delivery state):\n",
            pick_symbol("❌", "[FAIL]")
        ));
        if !marker_ok {
            report.push_str("  - Missing or invalid working copy marker file\n");
        }
        if !unexpected_source_true_jpegs.is_empty() {
            report.push_str(&format!(
                "  - {} unexpected source {} remained under source (not deleted):\n",
                unexpected_source_true_jpegs.len(),
                if strategy == "avif" {
                    "static image(s)"
                } else {
                    "JPEG(s)"
                }
            ));
            for p in &unexpected_source_true_jpegs {
                report.push_str(&format!(
                    "      - {}\n",
                    p.strip_prefix(&source_dir)?.display()
                ));
            }
        }
        if !unexpected_outputs.is_empty() {
            let ext_name = if strategy == "avif" { "AVIF" } else { "JXL" };
            report.push_str(&format!(
                "  - {} non-{ext_name} output(s) found in optimized directory:\n",
                unexpected_outputs.len()
            ));
            for (p, fmt) in &unexpected_outputs {
                report.push_str(&format!(
                    "      - {} (detected true format: {})\n",
                    p.strip_prefix(&optimized_dir)?.display(),
                    fmt
                ));
            }
        }
        if marker_opt.is_some() && expected_delivery_count != optimized_outputs.len() {
            let ext_name = if strategy == "avif" { "AVIF" } else { "JXL" };
            report.push_str(&format!(
                "  - Delivered {ext_name} count mismatch: expected {} but got {}\n",
                expected_delivery_count,
                optimized_outputs.len()
            ));
        }
        if !skipped_sources_missing.is_empty() {
            report.push_str(&format!(
                "  - {} skipped sources are missing from source folder (should remain):\n",
                skipped_sources_missing.len()
            ));
            for rel in &skipped_sources_missing {
                report.push_str(&format!("      - {rel}\n"));
            }
        }
        if !failed_sources_missing.is_empty() {
            report.push_str(&format!(
                "  - {} failed sources are missing from source folder (should remain):\n",
                failed_sources_missing.len()
            ));
            for rel in &failed_sources_missing {
                report.push_str(&format!("      - {rel}\n"));
            }
        }
        if !source_probe_errors.is_empty() {
            report.push_str(&format!(
                "  - Source format probe errors ({}):\n",
                source_probe_errors.len()
            ));
            for (p, e) in &source_probe_errors {
                report.push_str(&format!(
                    "      - {}: {}\n",
                    p.strip_prefix(&source_dir)?.display(),
                    e
                ));
            }
        }
        if !optimized_probe_errors.is_empty() {
            report.push_str(&format!(
                "  - Optimized format probe errors ({}):\n",
                optimized_probe_errors.len()
            ));
            for (p, e) in &optimized_probe_errors {
                report.push_str(&format!(
                    "      - {}: {}\n",
                    p.strip_prefix(&optimized_dir)?.display(),
                    e
                ));
            }
        }
        if !tier2_missing_proof.is_empty() {
            report.push_str(&format!(
                "  - {} tier-2 modern lossy files deleted without Photos/iCloud proof:\n",
                tier2_missing_proof.len()
            ));
            for rel in &tier2_missing_proof {
                report.push_str(&format!("      - {}\n", rel));
            }
        }
        if !tier2_unexpected_remaining.is_empty() {
            report.push_str(&format!(
                "  - {} tier-2 modern lossy files remained under source (not deleted):\n",
                tier2_unexpected_remaining.len()
            ));
            for rel in &tier2_unexpected_remaining {
                report.push_str(&format!("      - {}\n", rel));
            }
        }
    } else {
        report.push_str(&format!(
            "{} FAST-IMG DELIVERY INVARIANTS PASS\n",
            pick_symbol("✓", "[OK]")
        ));
        report.push_str(&format!(
            "  - All non-skipped/non-failed source {source_kind} were successfully deleted\n"
        ));
        report.push_str(&format!(
            "  - Optimized directory contains only {ext_name} delivery files\n"
        ));
        if tier2_recorded > 0 {
            report.push_str(
                "  - All tier-2 modern lossy files were successfully deleted after verification\n",
            );
        }
    }

    Ok(stats)
}

fn is_restore_jpeg_audit_marker(restored_dir: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(restored_dir) else {
        return false;
    };
    let mut components = relative.components();
    let Some(std::path::Component::Normal(session_name)) = components.next() else {
        return false;
    };
    let Some(std::path::Component::Normal(group_name)) = components.next() else {
        return false;
    };
    let Some(session_name) = session_name.to_str() else {
        return false;
    };
    if !session_name.starts_with("Audit_")
        || !restored_dir
            .join(session_name)
            .join(".mfb_restore_jpeg_audit.tsv")
            .is_file()
    {
        return false;
    }
    let expected_suffix = match group_name.to_str() {
        Some("Reconstruction Blocked") => ".mfb-recovery-needed.txt",
        Some("Needs Review") => ".mfb-needs-review.txt",
        _ => return false,
    };
    let mut file_name = None;
    for component in components {
        let std::path::Component::Normal(name) = component else {
            return false;
        };
        file_name = Some(name);
    }
    file_name
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(expected_suffix))
}

fn run_fast_img_restore_check(
    source_dir: &Path,
    restored_dir: &Path,
    report: &mut String,
    strategy: &str,
) -> Result<IntegrityStats> {
    let (source_dir, source_dir_exists) = resolve_source_directory_for_post_cleanup(source_dir)?;
    let restored_dir = restored_dir
        .canonicalize()
        .context("canonicalize restored_dir")?;

    report.push_str("── FAST-IMG RESTORE VERIFICATION ───────────────────────────────\n");
    report.push_str(&format!("Source:    {}\n", source_dir.display()));
    report.push_str(&format!("Restored:  {}\n\n", restored_dir.display()));

    let ext_name = if strategy == "avif" { "AVIF" } else { "JXL" };
    let mut stats = IntegrityStats {
        source: source_dir.to_string_lossy().to_string(),
        optimized: restored_dir.to_string_lossy().to_string(),
        scope: "images_only".to_string(),
        optimized_path_label: "Restored".to_string(),
        source_files_label: format!("Source {ext_name} files"),
        optimized_files_label: "Restored JPEG files".to_string(),
        ..Default::default()
    };

    let mut source_outputs = BTreeMap::new();
    let mut source_probe_errors = Vec::new();
    let source_files = if source_dir_exists {
        collect_regular_files(&source_dir)?
    } else {
        Vec::new()
    };
    for path in source_files {
        if let Some(name) = path.file_name().and_then(|f| f.to_str()) {
            if name.starts_with('.') || name == "fastmode_img_marker.json" || name == ".mfb_wc" {
                continue;
            }
        } else {
            continue;
        }
        match detect_true_format(&path) {
            Ok(true_format) => {
                if true_format == strategy {
                    let rel = path.strip_prefix(&source_dir)?;
                    source_outputs.insert(integrity_stem_key(rel), path);
                }
            }
            Err(e) => {
                source_probe_errors.push((path, e));
            }
        }
    }

    let mut restored_jpeg = BTreeMap::new();
    let mut restored_xmp_candidates = Vec::new();
    let mut non_jpeg_outputs = Vec::new();
    let mut restored_probe_errors = Vec::new();
    for path in collect_regular_files(&restored_dir)? {
        if path
            .file_name()
            .and_then(|f| f.to_str())
            .is_none_or(|s| s.starts_with('.'))
        {
            continue;
        }
        if path.file_name().and_then(|f| f.to_str()) == Some(".mfb_restore_jpeg_manifest.tsv") {
            continue;
        }
        if is_restore_jpeg_audit_marker(&restored_dir, &path) {
            continue;
        }
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp"))
        {
            restored_xmp_candidates.push(path);
            continue;
        }
        match detect_true_format(&path) {
            Ok(true_format) => {
                if true_format == "jpeg" {
                    let rel = path.strip_prefix(&restored_dir)?;
                    restored_jpeg.insert(integrity_stem_key(rel), path);
                } else {
                    non_jpeg_outputs.push((path, true_format));
                }
            }
            Err(e) => {
                restored_probe_errors.push((path, e));
            }
        }
    }
    let mut restored_xmp_sidecars = BTreeMap::new();
    for path in restored_xmp_candidates {
        let rel = path.strip_prefix(&restored_dir)?;
        let key = integrity_stem_key(rel);
        if !restored_jpeg.contains_key(&key) {
            non_jpeg_outputs.push((path, "orphan_xmp".to_string()));
            continue;
        }
        if let Err(error) = foundation::metadata::validate_xmp_sidecar(&path) {
            non_jpeg_outputs.push((path, format!("invalid_xmp: {error}")));
            continue;
        }
        restored_xmp_sidecars.insert(key, path);
    }

    let (manifest_records, mut restore_manifest_errors) = load_restore_jpeg_manifest(&restored_dir);
    if !source_dir_exists && manifest_records.is_empty() && restore_manifest_errors.is_empty() {
        restore_manifest_errors.push(
            "source directory was removed but the restore manifest contains no proof records"
                .to_string(),
        );
    }
    let mut manifest_sources = HashMap::new();
    let mut manifest_deleted_sources = HashSet::new();
    let mut seen_manifest_keys = HashSet::new();
    for record in manifest_records {
        let source_rel = &record.source_rel;
        let output_rel = &record.output_rel;
        let source_key = integrity_stem_key(Path::new(source_rel));
        let output_key = integrity_stem_key(Path::new(output_rel));
        if source_key != output_key {
            restore_manifest_errors.push(format!(
                "manifest key mismatch: source={source_rel} output={output_rel}"
            ));
            continue;
        }
        if !seen_manifest_keys.insert(source_key.clone()) {
            restore_manifest_errors.push(format!("duplicate manifest source key: {source_rel}"));
            continue;
        }
        let source_path = source_dir.join(source_rel);
        if let Some(xmp_rel) = record.xmp_rel.as_deref() {
            if integrity_stem_key(Path::new(xmp_rel)) != output_key {
                restore_manifest_errors.push(format!(
                    "manifest XMP key mismatch: output={output_rel} xmp={xmp_rel}"
                ));
            } else {
                let xmp_path = restored_dir.join(xmp_rel);
                if !xmp_path.is_file() {
                    restore_manifest_errors
                        .push(format!("manifest XMP sidecar is missing: {xmp_rel}"));
                } else if let Err(error) = foundation::metadata::validate_xmp_sidecar(&xmp_path) {
                    restore_manifest_errors.push(format!(
                        "manifest XMP sidecar is invalid: {xmp_rel}: {error}"
                    ));
                } else {
                    match calculate_blake3_hash(&xmp_path) {
                        Ok(actual_hash)
                            if record.xmp_blake3.as_deref() == Some(actual_hash.as_str()) => {}
                        Ok(_) => restore_manifest_errors
                            .push(format!("manifest XMP BLAKE3 mismatch: {xmp_rel}")),
                        Err(error) => restore_manifest_errors.push(format!(
                            "failed to hash manifest XMP sidecar {xmp_rel}: {error}"
                        )),
                    }
                }
            }
        }
        if record.source_deleted {
            if source_path.exists() {
                restore_manifest_errors.push(format!(
                    "manifest claims deleted source still exists: {source_rel}"
                ));
                continue;
            }
            let extension_sidecar = source_path.with_extension("xmp");
            let appended_sidecar = source_dir.join(format!("{source_rel}.xmp"));
            if extension_sidecar.exists() || appended_sidecar.exists() {
                restore_manifest_errors.push(format!(
                    "manifest deleted source left XMP sidecar: {source_rel}"
                ));
                continue;
            }
            manifest_deleted_sources.insert(source_key.clone());
        } else {
            if !source_path.is_file() {
                restore_manifest_errors
                    .push(format!("manifest retained source is missing: {source_rel}"));
                continue;
            }
            match calculate_blake3_hash(&source_path) {
                Ok(actual) if actual == record.source_blake3 => {}
                Ok(actual) => {
                    restore_manifest_errors.push(format!(
                        "retained source BLAKE3 mismatch: {source_rel}; expected={} actual={actual}",
                        record.source_blake3
                    ));
                    continue;
                }
                Err(err) => {
                    restore_manifest_errors.push(format!(
                        "failed to hash retained source {source_rel}: {err}"
                    ));
                    continue;
                }
            }
        }
        manifest_sources.insert(source_key, record);
    }

    let mut expected_keys = HashSet::new();
    for k in manifest_sources.keys() {
        expected_keys.insert(k.clone());
    }

    let mut retained_ineligible = Vec::new();
    let mut reconstruction_probe_errors = Vec::new();
    for (key, path) in &source_outputs {
        if restored_jpeg.contains_key(key) || manifest_sources.contains_key(key) {
            expected_keys.insert(key.clone());
            continue;
        }
        match foundation::jxl_utils::probe_jpeg_reconstruction_eligibility(path) {
            Ok(foundation::jxl_utils::JpegReconstructionEligibility::Exact) => {
                expected_keys.insert(key.clone());
            }
            Ok(foundation::jxl_utils::JpegReconstructionEligibility::PixelOnly) => {
                retained_ineligible.push((
                    path.clone(),
                    "pixel-decodable JXL has no exact JPEG reconstruction data".to_string(),
                ));
            }
            Ok(foundation::jxl_utils::JpegReconstructionEligibility::AdvertisedButRejected {
                diagnostic,
            }) => retained_ineligible.push((
                path.clone(),
                format!("advertised JPEG reconstruction rejected by djxl: {diagnostic}"),
            )),
            Err(reason) => reconstruction_probe_errors.push((path.clone(), reason)),
        }
    }

    let mut missing_keys = Vec::new();
    for k in &expected_keys {
        if !restored_jpeg.contains_key(k) {
            missing_keys.push(k.clone());
        }
    }
    missing_keys.sort();

    let mut extra_keys = Vec::new();
    for k in restored_jpeg.keys() {
        if !expected_keys.contains(k) {
            extra_keys.push(k.clone());
        }
    }
    extra_keys.sort();

    let mut hash_mismatched_restored_jpegs = Vec::new();
    for (key, record) in &manifest_sources {
        let output_rel = &record.output_rel;
        let expected_output = restored_dir.join(output_rel);
        if expected_output.is_file() {
            let output_hash = match calculate_blake3_hash(&expected_output) {
                Ok(hash) => hash,
                Err(err) => {
                    restore_manifest_errors.push(format!(
                        "failed to hash restored JPEG {}: {err}",
                        expected_output.display()
                    ));
                    continue;
                }
            };
            if output_hash != record.output_blake3 {
                hash_mismatched_restored_jpegs.push((key.clone(), record.clone(), output_hash));
            }
        }
    }

    let source_candidate_count =
        expected_keys.len() + retained_ineligible.len() + reconstruction_probe_errors.len();
    stats.source_files = source_candidate_count;
    stats.source_remaining_files = source_outputs.len();
    stats.verified_deleted_sources = manifest_deleted_sources.len();
    stats.optimized_files = restored_jpeg.len();
    stats.source_probe_errors = source_probe_errors.len();
    stats.optimized_probe_errors = restored_probe_errors.len();
    stats.restore_manifest_errors = restore_manifest_errors.len();
    stats.matched = restored_jpeg.len();
    stats.missing = missing_keys.len();
    stats.extra = extra_keys.len() + restored_probe_errors.len();
    stats.mismatched_types = non_jpeg_outputs.len();
    stats.count_delta = (restored_jpeg.len() + retained_ineligible.len()) as isize
        - source_candidate_count as isize;
    stats.explained_gaps =
        missing_keys.len() + extra_keys.len() + non_jpeg_outputs.len() + retained_ineligible.len();

    let integrity_failures = restore_manifest_errors.len()
        + missing_keys.len()
        + extra_keys.len()
        + hash_mismatched_restored_jpegs.len()
        + non_jpeg_outputs.len()
        + source_probe_errors.len()
        + reconstruction_probe_errors.len()
        + restored_probe_errors.len();

    let count_matches = integrity_failures == 0;
    stats.count_matches_with_handoff = count_matches;
    stats.count_fully_explained = count_matches;
    stats.count_status_label = if count_matches {
        Some("FAST_IMG_JPEG_RESTORE".to_string())
    } else {
        None
    };

    report.push_str("Restore mode:   JXL -> JPEG via djxl\n");
    report.push_str("Scope:          fast_img_restore\n\n");

    report.push_str(&format!(
        "Source JXL files:           {}\n",
        source_candidate_count
    ));
    report.push_str(&format!(
        "Source remaining JXL files: {}\n",
        source_outputs.len()
    ));
    report.push_str(&format!(
        "Manifest verified deleted source JXLs: {}\n",
        manifest_deleted_sources.len()
    ));
    report.push_str(&format!(
        "Manifest verified retained source JXLs: {}\n",
        manifest_sources.len() - manifest_deleted_sources.len()
    ));
    report.push_str(&format!(
        "Restored JPEG files:        {}\n",
        restored_jpeg.len()
    ));
    report.push_str(&format!(
        "Restored XMP sidecars:      {}\n",
        restored_xmp_sidecars.len()
    ));
    report.push_str(&format!(
        "Source probe errors:        {}\n",
        source_probe_errors.len()
    ));
    report.push_str(&format!(
        "Restored probe errors:      {}\n",
        restored_probe_errors.len()
    ));
    report.push_str(&format!(
        "Restore manifest errors:    {}\n",
        restore_manifest_errors.len()
    ));
    report.push_str(&format!(
        "Non-JPEG restored outputs:  {}\n\n",
        non_jpeg_outputs.len()
    ));

    if !retained_ineligible.is_empty() {
        report.push_str(&format!(
            "--- Safely retained JXLs without exact JPEG reconstruction ({}) ---\n",
            retained_ineligible.len()
        ));
        for (path, reason) in &retained_ineligible {
            report.push_str(&format!(
                "  ~ {}: {reason}\n",
                path.strip_prefix(&source_dir)?.display()
            ));
        }
        report.push_str(
            "  These files remain valid JXL assets; no pixel-to-JPEG fallback or source deletion occurred.\n\n",
        );
    }

    if integrity_failures > 0 {
        stats.has_warnings = true;
        stats.integrity_failures = integrity_failures;
        report.push_str(&format!(
            "{} RESTORE INVARIANT VIOLATIONS DETECTED (Unsafe restore state):\n",
            pick_symbol("❌", "[FAIL]")
        ));
        if !restore_manifest_errors.is_empty() {
            report.push_str(&format!(
                "--- Restore manifest errors ({}) ---\n",
                restore_manifest_errors.len()
            ));
            for err in &restore_manifest_errors {
                report.push_str(&format!("  ! {err}\n"));
            }
            report.push('\n');
        }
        if !missing_keys.is_empty() {
            report.push_str(&format!(
                "--- Missing restored JPEG outputs ({}) ---\n",
                missing_keys.len()
            ));
            for key in &missing_keys {
                if let Some(path) = source_outputs.get(key) {
                    report.push_str(&format!(
                        "  ! {}\n",
                        path.strip_prefix(&source_dir)?.display()
                    ));
                } else if let Some(record) = manifest_sources.get(key) {
                    report.push_str(&format!("  ! {}\n", record.source_rel));
                }
            }
            report.push('\n');
        }
        if !extra_keys.is_empty() {
            report.push_str(&format!(
                "--- Extra restored JPEG outputs ({}) ---\n",
                extra_keys.len()
            ));
            for key in &extra_keys {
                if let Some(path) = restored_jpeg.get(key) {
                    report.push_str(&format!(
                        "  + {}\n",
                        path.strip_prefix(&restored_dir)?.display()
                    ));
                }
            }
            report.push('\n');
        }
        if !non_jpeg_outputs.is_empty() {
            report.push_str(&format!(
                "--- Non-JPEG restored outputs ({}) ---\n",
                non_jpeg_outputs.len()
            ));
            for (path, fmt) in &non_jpeg_outputs {
                report.push_str(&format!(
                    "  x {} [true_format={}]\n",
                    path.strip_prefix(&restored_dir)?.display(),
                    fmt
                ));
            }
            report.push('\n');
        }
        if !hash_mismatched_restored_jpegs.is_empty() {
            report.push_str("--- Content hash mismatches ---\n");
            for (_k, record, actual) in &hash_mismatched_restored_jpegs {
                report.push_str(&format!(
                    "  - Content hash mismatch for restored JPEG: {}\n      Expected (manifest): \
                     {}\n      Actual (file):     {}\n",
                    record.output_rel, record.output_blake3, actual
                ));
            }
            report.push('\n');
        }
        if !source_probe_errors.is_empty() {
            report.push_str(&format!(
                "--- Source JXL format probe errors ({}) ---\n",
                source_probe_errors.len()
            ));
            for (p, e) in &source_probe_errors {
                report.push_str(&format!(
                    "  ! {}: {}\n",
                    p.strip_prefix(&source_dir)?.display(),
                    e
                ));
            }
            report.push('\n');
        }
        if !reconstruction_probe_errors.is_empty() {
            report.push_str(&format!(
                "--- Source JXL reconstruction probe errors ({}) ---\n",
                reconstruction_probe_errors.len()
            ));
            for (path, error) in &reconstruction_probe_errors {
                report.push_str(&format!(
                    "  ! {}: {error}\n",
                    path.strip_prefix(&source_dir)?.display()
                ));
            }
            report.push('\n');
        }
        if !restored_probe_errors.is_empty() {
            report.push_str(&format!(
                "--- Restored JPEG format probe errors ({}) ---\n",
                restored_probe_errors.len()
            ));
            for (p, e) in &restored_probe_errors {
                report.push_str(&format!(
                    "  ! {}: {}\n",
                    p.strip_prefix(&restored_dir)?.display(),
                    e
                ));
            }
            report.push('\n');
        }
    } else {
        report.push_str(&format!(
            "{} RESTORE INVARIANTS PASS\n",
            pick_symbol("✓", "[OK]")
        ));
        report.push_str("  - All manifested source files were restored\n");
        report.push_str("  - Retained source and restored JPEG BLAKE3 hashes match the manifest\n");
    }

    Ok(stats)
}

fn run_integrity_check(
    source_dir: &Path,
    optimized_dir: &Path,
    report: &mut String,
    processing_mode: &str,
    session_audit_paths: &[PathBuf],
    bundle_log_dir: Option<&Path>,
    explicit_log_paths: Option<&[PathBuf]>,
) -> Result<IntegrityStats> {
    report.push_str("── INTEGRITY VERIFICATION ─────────────────────────────────────\n");
    report.push_str(&format!("Source:    {}\n", source_dir.display()));
    report.push_str(&format!("Optimized: {}\n\n", optimized_dir.display()));

    let mut stats = IntegrityStats {
        source: source_dir.to_string_lossy().to_string(),
        optimized: optimized_dir.to_string_lossy().to_string(),
        scope: processing_mode.to_string(),
        optimized_path_label: "Optimized".to_string(),
        source_files_label: "Source files".to_string(),
        optimized_files_label: "Optimized files".to_string(),
        ..Default::default()
    };

    if !source_dir.is_dir() || !optimized_dir.is_dir() {
        report.push_str("❌ Error: Source or Optimized directory missing.\n\n");
        return Ok(stats);
    }

    let source_files = collect_media_files(source_dir, processing_mode)?;
    let optimized_files = collect_media_files(optimized_dir, processing_mode)?;

    let mut src_collisions = BTreeMap::new();
    for (k, v) in &source_files {
        if v.len() > 1 {
            src_collisions.insert(k.clone(), v.clone());
        }
    }
    let mut opt_collisions = BTreeMap::new();
    for (k, v) in &optimized_files {
        if v.len() > 1 {
            opt_collisions.insert(k.clone(), v.clone());
        }
    }

    let src_count: usize = source_files.values().map(std::vec::Vec::len).sum();
    let opt_count: usize = optimized_files.values().map(std::vec::Vec::len).sum();

    stats.source_files = src_count;
    stats.optimized_files = opt_count;

    report.push_str(&format!("Scope:           {processing_mode}\n"));
    report.push_str(&format!("Source files:    {src_count}\n"));
    report.push_str(&format!("Optimized files: {opt_count}\n"));

    let delta = opt_count as isize - src_count as isize;
    stats.count_delta = delta;

    let mut log_paths = Vec::new();
    if let Some(explicit) = explicit_log_paths {
        log_paths.extend(explicit.iter().cloned());
    } else if let Some(bundle) = bundle_log_dir {
        log_paths.extend(collect_bundle_run_logs(bundle)?);
    }

    let routing = match load_session_routing(session_audit_paths) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Warning: Failed to load session routing: {e}");
            HashMap::new()
        }
    };
    let rust_outcomes = match load_rust_outcomes_from_logs(&log_paths, Some(source_dir)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Warning: Failed to load rust outcomes: {e}");
            HashMap::new()
        }
    };

    let mut true_missing = Vec::new();
    let mut pipeline_handoff = Vec::new();
    let mut vid_pipeline_failed = Vec::new();
    let mut vid_pipeline_unverified = Vec::new();
    let mut matched = Vec::new();
    let mut extra = Vec::new();
    let mut ambiguous = Vec::new();

    for (key, src_paths) in &source_files {
        if optimized_files.contains_key(key) {
            let opt_paths = &optimized_files[key];
            if src_paths.len() == 1 && opt_paths.len() == 1 {
                matched.push((key.clone(), src_paths[0].clone(), opt_paths[0].clone()));
            } else if src_paths.len() == 1 && opt_paths.len() > 1 {
                let primary = choose_primary_output(opt_paths)?;
                matched.push((key.clone(), src_paths[0].clone(), primary));
                ambiguous.push((key.clone(), src_paths.clone(), opt_paths.clone()));
            } else {
                ambiguous.push((key.clone(), src_paths.clone(), opt_paths.clone()));
            }
        } else {
            for p in src_paths {
                let (category, note) = classify_missing_entry(
                    p,
                    processing_mode,
                    Some(&routing),
                    Some(&rust_outcomes),
                    Some(source_dir),
                )?;
                if category == "pipeline_handoff" {
                    pipeline_handoff.push((key.clone(), p.clone(), note));
                } else if category == "vid_pipeline_failed" {
                    vid_pipeline_failed.push((key.clone(), p.clone(), note));
                } else if category == "vid_pipeline_unverified" {
                    vid_pipeline_unverified.push((key.clone(), p.clone(), note));
                } else {
                    true_missing.push((key.clone(), p.clone(), note));
                }
            }
        }
    }

    for (key, opt_paths) in &optimized_files {
        if !source_files.contains_key(key) {
            for p in opt_paths {
                extra.push((key.clone(), p.clone()));
            }
        }
    }

    stats.matched = matched.len();
    stats.ambiguous = ambiguous.len();
    stats.missing = true_missing.len();
    stats.pipeline_handoff = pipeline_handoff.len();
    stats.vid_pipeline_failed = vid_pipeline_failed.len();
    stats.vid_pipeline_unverified = vid_pipeline_unverified.len();
    stats.extra = extra.len();

    for paths in source_files.values() {
        for p in paths {
            stats.source_total_size += match fs::metadata(p) {
                Ok(m) => m.len(),
                Err(_err) => 0,
            };
        }
    }
    for paths in optimized_files.values() {
        for p in paths {
            stats.optimized_total_size += match fs::metadata(p) {
                Ok(m) => m.len(),
                Err(_err) => 0,
            };
        }
    }

    let integrity_failures =
        true_missing.len() + vid_pipeline_failed.len() + vid_pipeline_unverified.len();
    stats.integrity_failures = integrity_failures;

    if !src_collisions.is_empty() {
        report
            .push_str("⚠️ WARNING: Duplicate source stems detected (Unsafe for 1-to-1 mapping):\n");
        for (key, paths) in &src_collisions {
            let mut hash_results = Vec::new();
            for p in paths {
                hash_results.push(file_content_blake3(p, 65536));
            }
            let unique_hashes: HashSet<String> =
                hash_results.iter().filter_map(|(h, _)| h.clone()).collect();
            let label = if unique_hashes.len() == 1 {
                "IDENTICAL content".to_string()
            } else {
                format!("{} DISTINCT files", unique_hashes.len())
            };
            report.push_str(&format!(
                "  Key '{key}' maps to {} files ({label}):\n",
                paths.len()
            ));
            for (p, (digest, err)) in paths.iter().zip(hash_results.iter()) {
                if let Some(d) = digest {
                    report.push_str(&format!(
                        "    - {}  [blake3:{d}]\n",
                        p.strip_prefix(source_dir)?.display()
                    ));
                } else if let Some(e) = err {
                    report.push_str(&format!(
                        "    - {}  [WARN] hash_read_failed: {e}\n",
                        p.strip_prefix(source_dir)?.display()
                    ));
                }
            }
        }
        report.push('\n');
    }

    if !opt_collisions.is_empty() {
        report.push_str("⚠️ WARNING: Duplicate optimized stems detected (Potential overwrites):\n");
        for (key, paths) in &opt_collisions {
            let mut hash_results = Vec::new();
            for p in paths {
                hash_results.push(file_content_blake3(p, 65536));
            }
            let unique_hashes: HashSet<String> =
                hash_results.iter().filter_map(|(h, _)| h.clone()).collect();
            let label = if unique_hashes.len() == 1 {
                "IDENTICAL content".to_string()
            } else {
                format!("{} DISTINCT files", unique_hashes.len())
            };
            report.push_str(&format!(
                "  Key '{key}' maps to {} files ({label}):\n",
                paths.len()
            ));
            for (p, (digest, err)) in paths.iter().zip(hash_results.iter()) {
                if let Some(d) = digest {
                    report.push_str(&format!(
                        "    - {}  [blake3:{d}]\n",
                        p.strip_prefix(optimized_dir)?.display()
                    ));
                } else if let Some(e) = err {
                    report.push_str(&format!(
                        "    - {}  [WARN] hash_read_failed: {e}\n",
                        p.strip_prefix(optimized_dir)?.display()
                    ));
                }
            }
        }
        report.push('\n');
    }

    if integrity_failures > 0 {
        stats.has_warnings = true;
        report.push_str(&format!(
            "{} INTEGRITY CHECKS FAILED:\n",
            pick_symbol("❌", "[FAIL]")
        ));
        for (_k, p, note) in &true_missing {
            report.push_str(&format!(
                "  - Missing optimized: {} ({})\n",
                p.strip_prefix(source_dir)?.display(),
                note
            ));
        }
        for (_k, p, note) in &vid_pipeline_failed {
            report.push_str(&format!(
                "  - Video pipeline failed/skipped: {} ({})\n",
                p.strip_prefix(source_dir)?.display(),
                note
            ));
        }
        for (_k, p, note) in &vid_pipeline_unverified {
            report.push_str(&format!(
                "  - Video pipeline unverified: {} ({})\n",
                p.strip_prefix(source_dir)?.display(),
                note
            ));
        }
    } else {
        report.push_str(&format!(
            "{} INTEGRITY CHECKS CLEAN\n\n",
            pick_symbol("✓", "[OK]")
        ));
    }

    if !extra.is_empty() {
        stats.has_warnings = true;
        report.push_str("⚠️ WARNING: Extra optimized files found (Not in source directory):\n");
        for (_k, p) in &extra {
            report.push_str(&format!(
                "  - Extra: {}\n",
                p.strip_prefix(optimized_dir)?.display()
            ));
        }
        report.push('\n');
    }

    if !ambiguous.is_empty() {
        stats.has_warnings = true;
        report.push_str("⚠️ WARNING: Ambiguous mapping stems (Collision risk):\n");
        for (k, src_paths, opt_paths) in &ambiguous {
            report.push_str(&format!("  Key '{k}' maps to:\n"));
            for p in src_paths {
                report.push_str(&format!(
                    "    Source:    {}\n",
                    p.strip_prefix(source_dir)?.display()
                ));
            }
            for p in opt_paths {
                report.push_str(&format!(
                    "    Optimized: {}\n",
                    p.strip_prefix(optimized_dir)?.display()
                ));
            }
        }
        report.push('\n');
    }

    if !pipeline_handoff.is_empty() {
        report.push_str("ℹ️ Expected Handoff Gaps (Excluded from missing total):\n");
        for (_k, p, note) in &pipeline_handoff {
            report.push_str(&format!(
                "  - Handoff: {} ({})\n",
                p.strip_prefix(source_dir)?.display(),
                note
            ));
        }
        report.push('\n');
    }

    let explained = pipeline_handoff.len();
    stats.explained_gaps = explained;
    if delta.unsigned_abs() == explained {
        stats.count_matches_with_handoff = true;
        stats.count_fully_explained = true;
    } else {
        stats.count_matches_with_handoff = false;
        stats.count_fully_explained = false;
    }

    Ok(stats)
}

fn collect_bundle_run_logs(log_dir: &Path) -> Result<Vec<PathBuf>> {
    if !log_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = HashSet::new();
    for entry in WalkDir::new(log_dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let is_run_log = (name.starts_with("img_run_") || name.starts_with("vid_run_"))
            && name.ends_with(".log");
        let is_jsonl =
            (name.starts_with("img_") || name.starts_with("vid_")) && name.ends_with(".jsonl");
        if is_run_log || is_jsonl {
            paths.insert(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
        }
    }
    let mut sorted: Vec<_> = paths.into_iter().collect();
    sorted.sort();
    Ok(sorted)
}

#[derive(Debug, Clone)]
struct LogConversionEntry {
    log: String,
    source: String,
    target: String,
    status: String,
    details: String,
}

#[derive(Debug, Clone)]
struct LogUncertainEntry {
    file: String,
    reason: String,
    probability: String,
    log: String,
    matching_folders: Vec<String>,
}

fn path_within_filter(path_str: &str, filter_dir_abs: Option<&Path>) -> bool {
    let Some(filter) = filter_dir_abs else {
        return true;
    };
    match Path::new(path_str).canonicalize() {
        Ok(abs) => abs.starts_with(filter),
        Err(_) => true,
    }
}

fn source_has_modern_true_format(source: &str) -> Result<bool> {
    let modern = ["webp", "avif", "jxl", "heic", "heif"];
    let fmt = detect_true_format(Path::new(source))?;
    Ok(modern.contains(&fmt.as_str()))
}

fn parse_conversion_line(line: &str) -> Option<(String, String, String, bool)> {
    let arrow_idx = line.find('→')?;
    let source = line[..arrow_idx]
        .split('>')
        .next_back()
        .unwrap_or(&line[..arrow_idx])
        .trim()
        .to_string();
    let rest = line[arrow_idx + '→'.len_utf8()..].trim();
    let open_paren = rest.rfind('(')?;
    let close_paren = rest.rfind(')')?;
    if close_paren <= open_paren {
        return None;
    }
    let target = rest[..open_paren].trim().to_string();
    let details = rest[open_paren + 1..close_paren].trim().to_string();
    let success = line.contains('✅');
    Some((source, target, details, success))
}

fn parse_logs(
    log_paths: &[PathBuf],
    report: &mut String,
    filter_dir: Option<&Path>,
) -> Result<(usize, usize)> {
    let target_formats = ["GIF", "MOV", "MP4", "HEVC", "AV1"];
    let log_dir_path = dev::infra::log_paths::unified_log_dir();
    let filter_dir_abs = match filter_dir {
        Some(d) => Some(d.canonicalize()?),
        None => None,
    };

    let mut results = Vec::new();
    let mut uncertain_cases: Vec<LogUncertainEntry> = Vec::new();
    let mut source_probe_errors = Vec::new();

    let mut files = Vec::new();
    for path in log_paths {
        if path.is_dir() {
            for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
                let p = entry.path();
                if p.is_file()
                    && (p.extension().and_then(|e| e.to_str()) == Some("log")
                        || p.file_name().and_then(|n| n.to_str()) == Some("error"))
                {
                    files.push(p.to_path_buf());
                }
            }
        } else if path.is_file() {
            files.push(path.clone());
        }
    }

    for log_file in files {
        let log_name = log_file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("log")
            .to_string();
        let content = fs::read_to_string(&log_file)
            .with_context(|| format!("log file unreadable: {}", log_file.display()))?;
        let mut current_file: Option<String> = None;
        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(stripped) = line.strip_prefix("checking ") {
                current_file = Some(stripped.trim().to_string());
            }
            if let Some(ref cur) = current_file
                && !path_within_filter(cur, filter_dir_abs.as_deref())
            {
                continue;
            }

            if let Some((source, target, details, success)) = parse_conversion_line(line) {
                if !path_within_filter(&source, filter_dir_abs.as_deref()) {
                    continue;
                }
                let source_is_modern = match source_has_modern_true_format(&source) {
                    Ok(v) => v,
                    Err(err) => {
                        source_probe_errors.push((
                            source.clone(),
                            log_name.clone(),
                            err.to_string(),
                        ));
                        false
                    }
                };
                let upper_target = target.to_ascii_uppercase();
                let upper_details = details.to_ascii_uppercase();
                if source_is_modern
                    && target_formats
                        .iter()
                        .any(|f| upper_target.contains(f) || upper_details.contains(f))
                {
                    results.push(LogConversionEntry {
                        log: log_name.clone(),
                        source,
                        target,
                        status: if success {
                            "SUCCESS".to_string()
                        } else {
                            "FAILED".to_string()
                        },
                        details,
                    });
                }
            }

            if line.contains("🔄 Animated→")
                && let Some(colon_idx) = line.rfind(':')
            {
                let source = line[colon_idx + 1..].trim().to_string();
                if path_within_filter(&source, filter_dir_abs.as_deref()) {
                    let source_is_modern = match source_has_modern_true_format(&source) {
                        Ok(v) => v,
                        Err(err) => {
                            source_probe_errors.push((
                                source.clone(),
                                log_name.clone(),
                                err.to_string(),
                            ));
                            false
                        }
                    };
                    if source_is_modern {
                        let mid = line
                            .split("🔄 Animated→")
                            .nth(1)
                            .unwrap_or("")
                            .split(':')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        results.push(LogConversionEntry {
                            log: log_name.clone(),
                            source,
                            target: format!("CONVERTED TO {mid}"),
                            status: "PROCESSING/UNKNOWN".to_string(),
                            details: mid,
                        });
                    }
                }
            }

            let is_uncertain = line.contains("Tree uncertain")
                || line.contains("Loop DB unavailable or disabled — running tree without KNN")
                || line.contains("Tree-only result remained uncertain");
            if is_uncertain && let Some(ref cur) = current_file {
                let reason = if line.contains("Tree uncertain") {
                    line.split('(')
                        .nth(1)
                        .and_then(|s| s.split(')').next())
                        .unwrap_or("N/A")
                        .to_string()
                } else if line.contains("Loop DB unavailable") {
                    "KNN Bypassed (DB Unavailable)".to_string()
                } else {
                    line.split('(')
                        .nth(1)
                        .and_then(|s| s.split(')').next())
                        .unwrap_or("N/A")
                        .to_string()
                };
                let prob = if line.contains("prob=") {
                    line.split("prob=")
                        .nth(1)
                        .and_then(|s| s.split(']').next())
                        .unwrap_or("N/A")
                        .to_string()
                } else {
                    "N/A".to_string()
                };
                if !uncertain_cases
                    .iter()
                    .any(|c| c.file == *cur && c.log == log_name)
                {
                    let stem = Path::new(cur)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    let matching_folders = if log_dir_path.is_dir() {
                        fs::read_dir(&log_dir_path)?
                            .filter_map(Result::ok)
                            .filter_map(|entry| {
                                let p = entry.path();
                                if p.is_dir()
                                    && p.file_name()
                                        .and_then(|n| n.to_str())
                                        .is_some_and(|n| n.contains(stem))
                                {
                                    p.file_name().map(|n| n.to_string_lossy().into_owned())
                                } else {
                                    None
                                }
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    uncertain_cases.push(LogUncertainEntry {
                        file: cur.clone(),
                        reason,
                        probability: prob,
                        log: log_name.clone(),
                        matching_folders,
                    });
                }
            }
        }
    }

    let mut unique_results = Vec::new();
    let mut seen_res = HashSet::new();
    for r in results {
        let key = (r.source.clone(), r.target.clone());
        if seen_res.insert(key) {
            unique_results.push(r);
        }
    }

    let mut unique_uncertain = Vec::new();
    let mut seen_unc = HashSet::new();
    for c in uncertain_cases {
        if seen_unc.insert(c.file.clone()) {
            unique_uncertain.push(c);
        }
    }

    let mut unique_probe_errors = Vec::new();
    let mut seen_probe = HashSet::new();
    for (source, log_name, detail) in source_probe_errors {
        let key = (source.clone(), log_name.clone(), detail.clone());
        if seen_probe.insert(key) {
            unique_probe_errors.push((source, log_name, detail));
        }
    }

    report.push_str("── LOOP INTENT EDGE CASES (UNCERTAIN / KNN BYPASSED) ──────────\n");
    if unique_uncertain.is_empty() {
        report.push_str("No uncertain loop intent cases found.\n\n");
    } else {
        for (i, c) in unique_uncertain.iter().enumerate() {
            report.push_str(&format!(
                "[{}] FILE: {}\n    REASON: {}\n    PROB:   {}\n    LOG:    {}\n",
                i + 1,
                c.file,
                c.reason,
                c.probability,
                c.log
            ));
            if !c.matching_folders.is_empty() {
                report.push_str(&format!("    FOLDERS: {}\n", c.matching_folders.join(", ")));
            }
            report.push_str(&format!("{}\n", "-".repeat(40)));
        }
        report.push('\n');
    }

    report.push_str("── MODERN TO LEGACY CONVERSIONS ───────────────────────────────\n");
    if unique_results.is_empty() {
        report.push_str("No conversions found.\n");
    } else {
        for (i, r) in unique_results.iter().enumerate() {
            report.push_str(&format!(
                "[{}] SOURCE: {}\n    TARGET: {}\n    STATUS: {}\n    INFO:   {}\n    LOG:    {}\n",
                i + 1,
                r.source,
                r.target,
                r.status,
                r.details,
                r.log
            ));
            report.push_str(&format!("{}\n", "-".repeat(40)));
        }
    }

    if !unique_probe_errors.is_empty() {
        report.push('\n');
        report.push_str("── LOG SOURCE FORMAT PROBE ERRORS ─────────────────────────────\n");
        for (i, (source, log_name, detail)) in unique_probe_errors.iter().enumerate() {
            report.push_str(&format!(
                "[{}] SOURCE: {}\n    LOG:    {}\n    ERROR:  {}\n",
                i + 1,
                source,
                log_name,
                detail
            ));
            report.push_str(&format!("{}\n", "-".repeat(40)));
        }
    }

    Ok((unique_results.len(), unique_uncertain.len()))
}

fn main() -> Result<()> {
    foundation::init_ghost_mode().context("initialize ghost mode")?;
    let args = Args::parse();

    if let Some(ref optimized_dir) = args.fast_img_marker_json {
        return print_fast_img_marker_json(optimized_dir);
    }

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let output_report = if let Some(ref p) = args.output {
        p.clone()
    } else {
        let log_dir = dev::infra::log_paths::unified_log_dir();
        fs::create_dir_all(&log_dir)?;
        log_dir.join(format!("diagnostic_report_{timestamp}.txt"))
    };
    if let Some(parent) = output_report.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut report = String::new();
    report.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    report.push_str("      MODERN FORMAT BOOST - DIAGNOSTIC ANALYSIS REPORT\n");
    report.push_str(&format!(
        "      Generated at: {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    report.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");

    let mut source_dir_context = None;
    let mut integrity_stats = None;
    let session_audit_paths = args
        .session_audit
        .iter()
        .map(|p| match p.canonicalize() {
            Ok(c) => c,
            Err(_) => p.clone(),
        })
        .collect::<Vec<_>>();
    let mut bundle_log_dir = Some(dev::infra::log_paths::unified_log_dir());
    let mut explicit_log_paths = None;

    if !args.logs.is_empty() {
        let mut files = Vec::new();
        for p in &args.logs {
            if p.is_file() {
                files.push(match p.canonicalize() {
                    Ok(c) => c,
                    Err(_) => p.clone(),
                });
            }
        }
        if files.is_empty() {
            let first_log = match args.logs[0].canonicalize() {
                Ok(c) => c,
                Err(_) => args.logs[0].clone(),
            };
            if first_log.is_dir() {
                bundle_log_dir = Some(first_log);
            }
        } else {
            explicit_log_paths = Some(files);
            bundle_log_dir = None;
        }
    }

    if let Some(ref verify_args) = args.verify {
        let resolved = resolve_verify_dirs(verify_args);
        if let Some((src, opt)) = resolved {
            source_dir_context = Some(src.clone());
            if args.fast_img_delivery {
                integrity_stats = Some(run_fast_img_delivery_check(
                    &src,
                    &opt,
                    &mut report,
                    &args.mode,
                    &args.strategy,
                )?);
            } else if args.fast_img_restore {
                integrity_stats = Some(run_fast_img_restore_check(
                    &src,
                    &opt,
                    &mut report,
                    &args.strategy,
                )?);
            } else {
                integrity_stats = Some(run_integrity_check(
                    &src,
                    &opt,
                    &mut report,
                    &args.mode,
                    &session_audit_paths,
                    bundle_log_dir.as_deref(),
                    explicit_log_paths.as_deref(),
                )?);
            }
        } else {
            report.push_str(&format!(
                "{} Error: Could not resolve paired directory for {}\n\n",
                pick_symbol("❌", "[ERROR]"),
                verify_args[0].display()
            ));
        }
    }

    let mut log_inputs = args
        .logs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if log_inputs.is_empty()
        && !args.fast_img_delivery
        && !args.fast_img_restore
        && let Some(ref dir) = bundle_log_dir
    {
        log_inputs.push(dir.to_string_lossy().to_string());
    }

    if !log_inputs.is_empty() {
        let log_paths_buf = log_inputs.iter().map(PathBuf::from).collect::<Vec<_>>();
        let (conv_count, unc_count) =
            parse_logs(&log_paths_buf, &mut report, source_dir_context.as_deref())?;
        println!(
            "{} Total conversion events: {}",
            pick_symbol("📈", "[CHART]"),
            conv_count
        );
        println!("🔭 Uncertain loop cases: {unc_count}");
    }

    if args.print_integrity_summary {
        if let Some(ref stats) = integrity_stats {
            let delta = stats.count_delta;
            let mut delta_text = "MATCH".to_string();
            if stats.integrity_failures > 0 {
                let issue_count = stats.integrity_failures;
                delta_text = format!(
                    "MISMATCH ({} invariant issue{})",
                    issue_count,
                    if issue_count == 1 { "" } else { "s" }
                );
            } else if stats.count_matches_with_handoff {
                let handoff_n = stats.pipeline_handoff;
                if handoff_n > 0 && delta == stats.expected_count_delta {
                    delta_text = format!(
                        "MATCH ({} expected handoff gap{})",
                        handoff_n,
                        if handoff_n == 1 { "" } else { "s" }
                    );
                }
            } else if stats.count_fully_explained {
                let direction = if delta > 0 { "more" } else { "fewer" };
                let explained = stats.explained_gaps;
                delta_text = format!(
                    "EXPLAINED ({} {}; all {} listed below)",
                    delta.abs(),
                    direction,
                    explained
                );
            } else {
                let direction = if delta > 0 { "more" } else { "fewer" };
                let explained = stats.explained_gaps;
                let unexplained = delta.unsigned_abs() - explained;
                if unexplained > 0 {
                    delta_text = format!(
                        "MISMATCH ({} {direction}; {unexplained} still unexplained)",
                        delta.abs()
                    );
                } else {
                    delta_text = format!("MISMATCH ({} {direction} in optimized)", delta.abs());
                }
            }

            println!("{} Integrity summary", pick_symbol("🔎", "[CHECK]"));
            println!("   Source:    {}", stats.source);
            println!("   {}: {}", stats.optimized_path_label, stats.optimized);
            println!("   Scope:           {}", stats.scope);
            println!(
                "   {:<34}{}",
                format!("{}:", stats.source_files_label),
                stats.source_files
            );
            println!(
                "   {:<34}{}",
                format!("{}:", stats.optimized_files_label),
                stats.optimized_files
            );
            if stats.tier2_recorded > 0 {
                println!(
                    "   {:<34}{}",
                    "Recorded tier-2 modern lossy:", stats.tier2_recorded
                );
                println!(
                    "   {:<34}{}",
                    "Verified tier-2 deleted:", stats.tier2_verified_deleted
                );
            }
            if stats.skipped_sources > 0 {
                println!(
                    "   {:<34}{}",
                    "Recorded skipped JPEGs:", stats.skipped_sources
                );
            }
            if stats.failed_sources > 0 {
                println!(
                    "   {:<34}{}",
                    "Recorded failed JPEGs:", stats.failed_sources
                );
            }
            if stats.source_remaining_files > 0 {
                println!(
                    "   {:<34}{}",
                    "Source files remaining:", stats.source_remaining_files
                );
            }
            println!("   Count status:    {delta_text}");
            println!("   Matched:         {}", stats.matched);
            println!("   Ambiguous:       {}", stats.ambiguous);
            println!(
                "   Missing:         {} (static / data-loss risk)",
                stats.missing
            );
            println!(
                "   Handoff gaps:    {} (expected gap — scope / vid static ignore)",
                stats.pipeline_handoff
            );
            println!(
                "   Vid failures:    {} (vid failed/skipped when encode required)",
                stats.vid_pipeline_failed
            );
            println!(
                "   Vid unverified:  {} (attach session/bundle logs for mfb::audit)",
                stats.vid_pipeline_unverified
            );
            println!("   Extra:           {}", stats.extra);
            println!("   Type mismatch:   {}", stats.mismatched_types);
            println!("   Integrity Issues:{}", stats.integrity_failures);
            println!(
                "   Integrity:      {}",
                if stats.has_warnings {
                    "WARNINGS"
                } else {
                    "CLEAN"
                }
            );
            if stats.source_total_size > 0 {
                let src_size = stats.source_total_size;
                let opt_size = stats.optimized_total_size;
                let savings = src_size.saturating_sub(opt_size);
                let savings_pct = if src_size > 0 {
                    (savings as f64 / src_size as f64) * 100.0
                } else {
                    0.0
                };
                println!(
                    "   Space saved:     {} ({:.1}%)",
                    format_size(savings),
                    savings_pct
                );
            }
        } else {
            println!("🔎 Integrity summary: unavailable (source/optimized pair not resolved)");
        }
    }

    if args.print_integrity_json {
        let stats = integrity_stats
            .as_ref()
            .context("machine integrity summary requested without a completed integrity check")?;
        let summary = IntegritySummaryMachine {
            has_warnings: stats.has_warnings,
            issue_count: stats.integrity_failures,
            source_count: stats.source_files,
            optimized_count: stats.optimized_files,
            skipped_count: stats.skipped_sources,
            failed_count: stats.failed_sources,
            source_remaining_count: stats.source_remaining_files,
            verified_deleted_count: stats.verified_deleted_sources,
        };
        println!(
            "{INTEGRITY_SUMMARY_JSON_PREFIX}{}",
            serde_json::to_string(&summary).context("serialize machine integrity summary")?
        );
    }

    println!(
        "{} Full report generated: {}",
        pick_symbol("📊", "[STATS]"),
        output_report.display()
    );
    fs::write(&output_report, &report)?;
    if let Some(stats) = integrity_stats
        && stats.has_warnings
    {
        anyhow::bail!(
            "integrity verification found {} issue(s); see {}",
            stats.integrity_failures.max(1),
            output_report.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundation::ToolBuilder;
    use serial_test::serial;

    #[test]
    fn test_restore_audit_marker_exemption_is_scoped_to_owned_session() {
        let temp = tempfile::tempdir().unwrap();
        let session = temp.path().join("Audit_1_2");
        let marker = session
            .join("Reconstruction Blocked/nested")
            .join("photo.jxl.mfb-recovery-needed.txt");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(
            session.join(".mfb_restore_jpeg_audit.tsv"),
            "# MFB_RESTORE_JPEG_AUDIT_V2\n",
        )
        .unwrap();
        fs::write(&marker, "marker").unwrap();

        assert!(is_restore_jpeg_audit_marker(temp.path(), &marker));
        assert!(!is_restore_jpeg_audit_marker(
            temp.path(),
            &temp.path().join("spoof.mfb-recovery-needed.txt")
        ));
        assert!(!is_restore_jpeg_audit_marker(
            temp.path(),
            &session
                .join("Other")
                .join("photo.jxl.mfb-recovery-needed.txt")
        ));
    }

    #[test]
    fn test_collect_bundle_run_logs_finds_nested_jsonl_and_run_logs() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(temp.path().join("img_run_20260101.log"), "log").unwrap();
        fs::write(nested.join("vid_run_20260101.log"), "log").unwrap();
        fs::write(nested.join("img_trace.jsonl"), "{}\n").unwrap();
        let paths = collect_bundle_run_logs(temp.path()).unwrap();
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn test_parse_logs_writes_conversion_and_uncertain_sections() {
        let temp = tempfile::tempdir().unwrap();
        let media = temp.path().join("photo.heic");
        fs::write(&media, b"fake").unwrap();
        let log = temp.path().join("img_run_test.log");
        let media_str = media.display().to_string();
        fs::write(
            &log,
            format!(
                "checking {media_str}\n{media_str} → MOV (hevc) (encode ok) ✅\nTree uncertain \
                 (low confidence) [prob=0.42] falling back to Layer 6 KNN\n"
            ),
        )
        .unwrap();
        let mut report = String::new();
        let (conv, unc) = parse_logs(&[log], &mut report, Some(temp.path())).unwrap();
        assert_eq!(unc, 1);
        assert!(report.contains("MODERN TO LEGACY CONVERSIONS"));
        assert!(report.contains("LOOP INTENT EDGE CASES"));
        let _ = conv;
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn duplicate_fingerprint_uses_blake3_over_the_requested_prefix() {
        let temp = tempfile::NamedTempFile::new().expect("temp file");
        fs::write(temp.path(), b"abcdef").expect("write test payload");
        let (digest, error) = file_content_blake3(temp.path(), 3);
        assert_eq!(digest, Some(blake3::hash(b"abc").to_hex().to_string()));
        assert!(error.is_none());
    }

    #[test]
    fn test_load_restore_jpeg_manifest() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(".mfb_restore_jpeg_manifest.tsv");

        let tsv_content = "\
# mock comment
source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted
7372632f66696c65312e6a7067\t6f75742f66696c65312e6a7067\thash1\thash2\ttrue
";
        fs::write(&manifest_path, tsv_content).unwrap();

        let (records, errors) = load_restore_jpeg_manifest(tempdir.path());
        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_rel, "src/file1.jpg");
        assert_eq!(records[0].output_rel, "out/file1.jpg");
        assert_eq!(records[0].source_blake3, "hash1");
        assert_eq!(records[0].output_blake3, "hash2");
        assert!(records[0].source_deleted);
    }

    #[test]
    fn test_load_restore_jpeg_manifest_with_xmp_proof() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(".mfb_restore_jpeg_manifest.tsv");
        let tsv_content = "\
# MFB_RESTORE_JPEG_MANIFEST_V2
source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\txmp_rel_hex\txmp_blake3\tsource_deleted
7372632f66696c65312e4a584c\t6f75742f66696c65312e6a7067\thash1\thash2\t6f75742f66696c65312e786d70\txmp-hash\ttrue
";
        fs::write(&manifest_path, tsv_content).unwrap();

        let (records, errors) = load_restore_jpeg_manifest(tempdir.path());
        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].xmp_rel.as_deref(), Some("out/file1.xmp"));
        assert_eq!(records[0].xmp_blake3.as_deref(), Some("xmp-hash"));
        assert!(records[0].source_deleted);
    }

    #[test]
    fn test_load_restore_jpeg_manifest_v3_validates_reconstruction_and_toolchain() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(".mfb_restore_jpeg_manifest.tsv");
        let tsv_content = "\
# MFB_RESTORE_JPEG_MANIFEST_V3
source_rel_hex\toutput_rel_hex\tsource_jxl_blake3\treconstruction_jpeg_blake3\trestored_jpeg_blake3\txmp_rel_hex\txmp_blake3\tverified_unix_seconds\tmfb_version\tdjxl_version_hex\tsource_deleted
7372632f66696c65312e4a584c\t6f75742f66696c65312e6a7067\thash1\tjpeg-hash\tjpeg-hash\t\t\t1\t0.11.3\t646a786c20302e31332e30\tfalse
";
        fs::write(&manifest_path, tsv_content).unwrap();

        let (records, errors) = load_restore_jpeg_manifest(tempdir.path());
        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_blake3, "hash1");
        assert_eq!(records[0].output_blake3, "jpeg-hash");
        assert!(!records[0].source_deleted);
    }

    #[test]
    fn test_load_restore_jpeg_manifest_v3_rejects_nonidentical_reconstruction_hash() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(".mfb_restore_jpeg_manifest.tsv");
        let tsv_content = "\
# MFB_RESTORE_JPEG_MANIFEST_V3
source_rel_hex\toutput_rel_hex\tsource_jxl_blake3\treconstruction_jpeg_blake3\trestored_jpeg_blake3\txmp_rel_hex\txmp_blake3\tverified_unix_seconds\tmfb_version\tdjxl_version_hex\tsource_deleted
7372632f66696c65312e4a584c\t6f75742f66696c65312e6a7067\thash1\treconstructed\trewritten\t\t\t1\t0.11.3\t646a786c20302e31332e30\tfalse
";
        fs::write(&manifest_path, tsv_content).unwrap();

        let (records, errors) = load_restore_jpeg_manifest(tempdir.path());
        assert!(records.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("reconstruction and restored JPEG hashes differ"));
    }

    #[test]
    fn test_load_restore_jpeg_manifest_rejects_unsafe_and_odd_hex_paths() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(".mfb_restore_jpeg_manifest.tsv");
        let tsv_content = "\
source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted
2e2e2f6573636170652e4a584c\t6f75742e6a7067\thash1\thash2\tfalse
6f6464f\t6f75742e6a7067\thash1\thash2\tfalse
";
        fs::write(&manifest_path, tsv_content).unwrap();

        let (records, errors) = load_restore_jpeg_manifest(tempdir.path());
        assert!(records.is_empty());
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("safe relative paths"));
        assert!(errors[1].contains("invalid source_rel hex"));
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_accepts_deleted_sources_and_jxl_only_output() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(optimized.join("day1")).unwrap();
        assert!(!source.exists());

        fs::write(optimized.join("day1").join("photo.JXL"), b"\xff\x0aencoded").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 1,
            "skipped_sources": {},
            "failed_sources": {}
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 1);
        assert_eq!(stats.source_remaining_files, 0);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.integrity_failures, 0);
        assert!(!stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_meme_delivery_rejects_recorded_static_source_left_behind() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(
            source.join("meme.png"),
            b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR",
        )
        .unwrap();
        fs::write(
            optimized.join("meme.avif"),
            b"\x00\x00\x00\x14ftypavif\x00\x00\x00\x00avif",
        )
        .unwrap();
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 1,
            "strategy": "avif",
            "blake3_log": {
                "meme.png": {
                    "out_rel": "meme.avif",
                    "src": "source-hash",
                    "out": "output-hash",
                    "library_asset": null
                }
            },
            "skipped_sources": {},
            "failed_sources": {}
        });
        fs::write(
            optimized.join("fastmode_img_marker.json"),
            serde_json::to_string(&marker_data).unwrap(),
        )
        .unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "avif")
                .unwrap();

        assert_eq!(stats.source_remaining_files, 1);
        assert!(stats.integrity_failures > 0);
        assert!(report.contains("unexpected source static image"));
        assert!(report.contains("Optimized AVIF files:"));
    }

    #[test]
    #[serial]
    fn test_fast_img_marker_lookup_reads_state_root_marker() {
        let tempdir = tempfile::tempdir().unwrap();
        let state_root = tempdir.path().join("mfb_state");
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        let marker_dir = state_root.join("fast_img").join("markers");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();
        fs::create_dir_all(&marker_dir).unwrap();

        unsafe { std::env::set_var("MFB_HOME_ROOT", &state_root) };
        let marker_path = marker_dir.join("run.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 1,
            "stage": "gate1_failed",
            "blake3_log": {},
            "skipped_sources": {},
            "failed_sources": {}
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let (marker, found_path, marker_error) = load_fast_img_marker_for_optimized(&optimized);

        unsafe { std::env::remove_var("MFB_HOME_ROOT") };
        assert_eq!(found_path, Some(marker_path));
        assert!(
            marker_error.is_none(),
            "unexpected marker error: {marker_error:?}"
        );
        assert_eq!(
            marker
                .as_ref()
                .and_then(|m| m.get("stage"))
                .and_then(|v| v.as_str()),
            Some("gate1_failed")
        );
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_accepts_recorded_skipped_sources_remaining() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(source.join("skipped.bin"), b"\xff\xd8\xff\xe0jpeg").unwrap();
        fs::write(optimized.join("photo.JXL"), b"\xff\x0aencoded").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 2,
            "skipped_sources": {
                "skipped.bin": {
                    "src": "recorded-source-blake3",
                    "reason": "lossless JPEG encode failed after strict cascade"
                }
            },
            "failed_sources": {}
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.source_remaining_files, 1);
        assert_eq!(stats.skipped_sources, 1);
        assert_eq!(stats.integrity_failures, 0);
        assert!(!stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_accepts_all_sources_skipped_with_no_jxl_outputs() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(source.join("missing_eoi.bin"), b"\xff\xd8\xff\xe0jpeg").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 1,
            "skipped_sources": {
                "missing_eoi.bin": {
                    "src": "recorded-source-blake3",
                    "reason": "Skipped: JPEG cannot be reversibly encoded; source remains unmodified"
                }
            },
            "failed_sources": {}
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 1);
        assert_eq!(stats.optimized_files, 0);
        assert_eq!(stats.source_remaining_files, 1);
        assert_eq!(stats.skipped_sources, 1);
        assert_eq!(stats.integrity_failures, 0);
        assert!(!stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_accepts_recorded_failed_sources_remaining() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(source.join("djxl_failed.jpg"), b"\xff\xd8\xff\xe0jpeg").unwrap();
        fs::write(optimized.join("converted.JXL"), b"\xff\x0aencoded").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 2,
            "skipped_sources": {},
            "failed_sources": {
                "djxl_failed.jpg": {
                    "src": "failed-source-blake3",
                    "reason": "pixel-diff: djxl exited non-zero decoding output.JXL"
                }
            }
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.source_remaining_files, 1);
        assert_eq!(stats.failed_sources, 1);
        assert_eq!(stats.integrity_failures, 0);
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_rejects_missing_marker_proof() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(optimized.join("photo.JXL"), b"\xff\x0aencoded").unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 0);
        assert_eq!(stats.integrity_failures, 1);
        assert!(stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_rejects_remaining_true_jpeg_and_non_jxl_output() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(source.join("remaining.bin"), b"\xff\xd8\xff\xe0jpeg").unwrap();
        fs::write(optimized.join("photo.jpg"), b"not-jxl").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 1,
            "skipped_sources": {},
            "failed_sources": {}
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 1);
        assert_eq!(stats.source_remaining_files, 1);
        assert_eq!(stats.optimized_files, 0);
        assert_eq!(stats.extra, 1);
        assert!(stats.integrity_failures >= 3);
        assert!(stats.has_warnings);
    }

    #[test]
    fn test_fast_img_jpeg_probe_matches_rust_magic_detector() {
        let tempdir = tempfile::tempdir().unwrap();
        let true_jpeg = tempdir.path().join("camera.bin");
        let png_with_jpg_ext = tempdir.path().join("not-a-jpeg.jpg");
        let truncated = tempdir.path().join("truncated.jpg");

        fs::write(&true_jpeg, b"\xff\xd8\xff\xe1jpeg").unwrap();
        fs::write(&png_with_jpg_ext, b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").unwrap();
        fs::write(&truncated, b"\xff\xd8").unwrap();

        assert_eq!(detect_true_format(&true_jpeg).unwrap(), "jpeg");
        assert!(is_true_jpeg_file(&true_jpeg).unwrap());
        assert_eq!(detect_true_format(&png_with_jpg_ext).unwrap(), "png");
        assert!(!is_true_jpeg_file(&png_with_jpg_ext).unwrap());
        assert_eq!(detect_true_format(&truncated).unwrap(), "unknown");
        assert!(!is_true_jpeg_file(&truncated).unwrap());
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_rejects_spoofed_jxl_extension() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(optimized.join("photo.JXL"), b"not a jpeg xl codestream").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 1,
            "skipped_sources": {},
            "failed_sources": {}
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.optimized_files, 0);
        assert_eq!(stats.extra, 1);
        assert!(stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_restore_check_accepts_jxl_to_jpeg_roundtrip() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(restored.join("nested")).unwrap();

        fs::write(
            source.join("nested").join("camera.JXL"),
            b"\xff\x0atrue-jxl",
        )
        .unwrap();
        fs::write(
            restored.join("nested").join("camera.jpeg"),
            b"\xff\xd8\xff\xe0true-jpeg",
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert_eq!(stats.source_files, 1);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.matched, 1);
        assert_eq!(stats.count_delta, 0);
        assert_eq!(stats.integrity_failures, 0);
        assert!(!stats.has_warnings);
    }

    #[test]
    #[serial]
    #[rustfmt::skip]
    fn test_fast_img_restore_check_accepts_manifest_verified_deleted_sources() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(restored.join("nested")).unwrap();
        assert!(!source.exists());

        let camera_path = restored.join("nested").join("camera.jpg");
        fs::write(&camera_path, b"\xff\xd8\xff\xe0true-jpeg").unwrap();
        let output_hash = calculate_blake3_hash(&camera_path).unwrap();

        let manifest_content = format!(
            "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted\n\
             6e65737465642f63616d6572612e4a584c\t6e65737465642f63616d6572612e6a7067\t\
             source-blake3\t{output_hash}\ttrue\n"
        );
        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            manifest_content,
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert_eq!(stats.source_files, 1);
        assert_eq!(stats.source_remaining_files, 0);
        assert_eq!(stats.verified_deleted_sources, 1);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.matched, 1);
        assert_eq!(stats.integrity_failures, 0);
        assert!(!stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_restore_check_rejects_removed_source_without_manifest() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(&restored).unwrap();
        fs::write(restored.join("camera.jpg"), b"\xff\xd8\xff\xe0true-jpeg").unwrap();
        assert!(!source.exists());

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert!(stats.has_warnings);
        assert_eq!(stats.restore_manifest_errors, 1);
        assert!(report.contains(
            "source directory was removed but the restore manifest contains no proof records"
        ));
    }

    #[test]
    #[serial]
    #[rustfmt::skip]
    fn test_fast_img_restore_check_accepts_manifest_verified_retained_sources() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(restored.join("nested")).unwrap();

        let source_path = source.join("nested").join("camera.JXL");
        let output_path = restored.join("nested").join("camera.jpg");
        fs::write(&source_path, b"\xff\x0atrue-jxl").unwrap();
        fs::write(&output_path, b"\xff\xd8\xff\xe0true-jpeg").unwrap();
        let source_hash = calculate_blake3_hash(&source_path).unwrap();
        let output_hash = calculate_blake3_hash(&output_path).unwrap();

        let manifest_content = format!(
            "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted\n\
             6e65737465642f63616d6572612e4a584c\t6e65737465642f63616d6572612e6a7067\t\
             {source_hash}\t{output_hash}\tfalse\n"
        );
        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            manifest_content,
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert_eq!(stats.source_files, 1);
        assert_eq!(stats.source_remaining_files, 1);
        assert_eq!(stats.verified_deleted_sources, 0);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.integrity_failures, 0, "{report}");
        assert!(!stats.has_warnings);
        assert!(report.contains("Manifest verified retained source JXLs: 1"));
    }

    #[test]
    #[serial]
    fn test_fast_img_restore_check_explains_non_reconstructible_retained_jxl() {
        if !foundation::CjxlBuilder::new().check_available()
            || !foundation::DjxlBuilder::new().check_available()
            || !foundation::tool_builders::JxlinfoBuilder::new().check_available()
        {
            return;
        }
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&restored).unwrap();

        let pixels = source.join("pixels.ppm");
        let retained = source.join("pixels-only.JXL");
        fs::write(&pixels, b"P6\n1 1\n255\n\x28\x32\x3c").unwrap();
        let encoded = std::process::Command::new(foundation::constants::TOOL_CJXL)
            .arg(&pixels)
            .arg(&retained)
            .arg("--distance=0")
            .output()
            .unwrap();
        fs::remove_file(&pixels).unwrap();
        assert!(
            encoded.status.success(),
            "test cjxl failed: {}",
            String::from_utf8_lossy(&encoded.stderr)
        );

        let restored_jpeg = restored.join("healthy.jpg");
        fs::write(&restored_jpeg, b"\xff\xd8\xff\xe0true-jpeg").unwrap();
        let output_hash = calculate_blake3_hash(&restored_jpeg).unwrap();
        let manifest_content = format!(
            "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted\n\
             6865616c7468792e4a584c\t6865616c7468792e6a7067\tsource-blake3\t{output_hash}\ttrue\n"
        );
        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            manifest_content,
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.count_delta, 0);
        assert_eq!(stats.integrity_failures, 0, "{report}");
        assert!(!stats.has_warnings);
        assert!(report.contains("Safely retained JXLs without exact JPEG reconstruction (1)"));
    }

    #[test]
    #[serial]
    #[rustfmt::skip]
    fn test_fast_img_restore_check_rejects_manifest_claim_when_source_still_exists() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(restored.join("nested")).unwrap();

        // Source file JXL still exists (which manifests say is deleted)
        fs::write(
            source.join("nested").join("camera.JXL"),
            b"\xff\x0atrue-jxl",
        )
        .unwrap();
        fs::write(
            restored.join("nested").join("camera.jpg"),
            b"\xff\xd8\xff\xe0true-jpeg",
        )
        .unwrap();

        let manifest_content = "\
source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted
6e65737465642f63616d6572612e4a584c\t6e65737465642f63616d6572612e6a7067\tsource-blake3\toutput-blake3\ttrue
";
        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            manifest_content,
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert_eq!(stats.source_files, 1);
        assert!(stats.integrity_failures >= 1);
        assert!(stats.has_warnings);
    }

    #[test]
    #[serial]
    #[rustfmt::skip]
    fn test_fast_img_restore_check_rejects_manifest_deleted_source_with_xmp_leftover() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(restored.join("nested")).unwrap();

        // XMP sidecar exists under source dir:
        fs::write(
            source.join("nested").join("camera.JXL.xmp"),
            b"<x:xmpmeta/>",
        )
        .unwrap();
        fs::write(
            restored.join("nested").join("camera.jpg"),
            b"\xff\xd8\xff\xe0true-jpeg",
        )
        .unwrap();

        let manifest_content = "\
source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted
6e65737465642f63616d6572612e4a584c\t6e65737465642f63616d6572612e6a7067\tsource-blake3\toutput-blake3\ttrue
";
        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            manifest_content,
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert!(stats.integrity_failures >= 1);
        assert!(stats.has_warnings);
    }

    #[test]
    #[serial]
    #[rustfmt::skip]
    fn test_fast_img_restore_check_rejects_duplicate_manifest_deleted_source() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(restored.join("nested")).unwrap();

        fs::write(
            restored.join("nested").join("camera.jpg"),
            b"\xff\xd8\xff\xe0true-jpeg",
        )
        .unwrap();

        let manifest_content = "\
source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted
6e65737465642f63616d6572612e4a584c\t6e65737465642f63616d6572612e6a7067\tsource-blake3\toutput-blake3\ttrue
6e65737465642f63616d6572612e4a584c\t6e65737465642f63616d6572612e6a7067\tsource-blake3\toutput-blake3\ttrue
";
        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            manifest_content,
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert!(stats.integrity_failures >= 1);
        assert!(stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_restore_check_rejects_unreadable_sources_and_non_jpeg_outputs() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album_optimized");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&restored).unwrap();

        fs::write(source.join("missing.JXL"), b"\xff\x0atrue-jxl").unwrap();
        fs::write(source.join("wrong.JXL"), b"\xff\x0atrue-jxl").unwrap();
        fs::write(restored.join("wrong.png"), b"\x89PNG\r\n\x1a\nnot-jpeg").unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();

        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.optimized_files, 0);
        assert_eq!(stats.missing, 0);
        assert_eq!(stats.mismatched_types, 1);
        assert!(stats.integrity_failures >= 3);
        assert!(stats.has_warnings);
        assert!(report.contains("Source JXL reconstruction probe errors (2)"));
    }

    #[test]
    fn test_fast_img_jpeg_probe_surfaces_io_errors() {
        let tempdir = tempfile::tempdir().unwrap();
        let missing = tempdir.path().join("missing.jpg");
        let res = is_true_jpeg_file(&missing);
        assert!(res.is_err());
        let err_str = res.err().unwrap().to_string();
        assert!(err_str.contains("No such file or directory") || err_str.contains("missing.jpg"));
    }

    #[test]
    fn test_integrity_collection_uses_true_format_not_spoofed_extension() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        fs::create_dir_all(&source).unwrap();
        let disguised_jpeg = source.join("camera.bin");
        let fake_jpeg = source.join("fake.jpg");
        fs::write(&disguised_jpeg, b"\xff\xd8\xff\xe0jpeg").unwrap();
        fs::write(&fake_jpeg, b"not an image").unwrap();

        let collected = collect_media_files(&source, "images_only").unwrap();
        assert!(collected.contains_key("camera"));
        assert_eq!(
            collected.get("camera").unwrap(),
            &vec![disguised_jpeg.canonicalize().unwrap()]
        );
        assert!(!collected.contains_key("fake"));
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_records_media_probe_errors() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();
        let source_bad = source.join("bad-source.jpg");
        let optimized_bad = optimized.join("bad-output.jxl");
        fs::write(&source_bad, b"\xff\xd8\xff\xe0bad").unwrap();
        fs::write(&optimized_bad, b"\xff\x0abad").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms1 = fs::metadata(&source_bad).unwrap().permissions();
            perms1.set_mode(0o000);
            fs::set_permissions(&source_bad, perms1).unwrap();

            let mut perms2 = fs::metadata(&optimized_bad).unwrap().permissions();
            perms2.set_mode(0o000);
            fs::set_permissions(&optimized_bad, perms2).unwrap();
        }

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 1,
            "skipped_sources": {},
            "failed_sources": {}
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Result::Ok(meta) = fs::metadata(&source_bad) {
                let mut perms = meta.permissions();
                perms.set_mode(0o644);
                let _ = fs::set_permissions(&source_bad, perms);
            }
            if let Result::Ok(meta) = fs::metadata(&optimized_bad) {
                let mut perms = meta.permissions();
                perms.set_mode(0o644);
                let _ = fs::set_permissions(&optimized_bad, perms);
            }
        }

        assert_eq!(stats.source_probe_errors, 1);
        assert_eq!(stats.optimized_probe_errors, 1);
        assert!(stats.integrity_failures >= 2);
        assert!(report.contains("Source format probe errors"));
        assert!(report.contains("Optimized format probe errors"));
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_accepts_tier2_modern_lossy_assets() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 0,
            "skipped_sources": {},
            "failed_sources": {},
            "tier2_imported_assets": [
                {
                    "rel_path": "photo.webp",
                    "blake3": "abc",
                    "sync_status": "success",
                    "quarantined": false,
                    "photos_uuid": "some-uuid"
                }
            ]
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.tier2_recorded, 1);
        assert_eq!(stats.tier2_verified_deleted, 1);
        assert_eq!(stats.integrity_failures, 0);
        assert!(!stats.has_warnings);
        assert!(report.contains("Recorded tier-2 lossy files: 1"));
        assert!(report.contains("Verified tier-2 deleted:     1"));
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_rejects_tier2_missing_proof() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 0,
            "skipped_sources": {},
            "failed_sources": {},
            "tier2_imported_assets": [
                {
                    "rel_path": "photo.webp",
                    "blake3": "abc",
                    "sync_status": "success",
                    "quarantined": false,
                    "photos_uuid": null
                }
            ]
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.tier2_recorded, 1);
        assert_eq!(stats.tier2_verified_deleted, 0);
        assert!(stats.integrity_failures >= 1);
        assert!(stats.has_warnings);
        assert!(report.contains("deleted without Photos/iCloud proof"));
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_rejects_tier2_unexpected_remaining() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(source.join("photo.webp"), b"fake content").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 0,
            "skipped_sources": {},
            "failed_sources": {},
            "tier2_imported_assets": [
                {
                    "rel_path": "photo.webp",
                    "blake3": "abc",
                    "sync_status": "success",
                    "quarantined": false,
                    "photos_uuid": "some-uuid"
                }
            ]
        });
        fs::write(&marker_path, serde_json::to_string(&marker_data).unwrap()).unwrap();

        let mut report = String::new();
        let stats =
            run_fast_img_delivery_check(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.tier2_recorded, 1);
        assert_eq!(stats.tier2_verified_deleted, 0);
        assert!(stats.integrity_failures >= 1);
        assert!(stats.has_warnings);
        assert!(report.contains("remained under source (not deleted)"));
    }
}
