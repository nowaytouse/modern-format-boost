//! Modern Format Boost - Diagnostic Analysis and Integrity Verifier in Rust.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::drag_drop::{
    INTEGRITY_SUMMARY_JSON_PREFIX, INTEGRITY_UNAVAILABLE_JSON_PREFIX, IntegritySummaryMachine,
    IntegrityUnavailableMachine, IntegrityUnavailableReason,
};
use dev::infra::ui_tokens::pick_symbol;
use dev::media::scope::{
    SKIP_EXTS, classify_missing_entry, detect_true_format, integrity_stem_key,
    load_rust_outcomes_from_logs, load_session_routing, true_format_matches_processing_mode,
};
use foundation::common_utils::calculate_blake3_hash;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "verify",
    version,
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

    #[arg(long, hide = true, requires_all = ["verify", "fast_img_delivery"])]
    fast_img_photos_cleanup_recovery: Option<String>,

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

    #[arg(
        long,
        value_name = "WORKING_COPY",
        conflicts_with_all = ["verify", "fast_img_marker_json", "fast_img_jpeg_execution", "fast_img_delivery", "fast_img_restore", "logs", "output", "session_audit", "print_integrity_summary", "print_integrity_json"],
        help = "Observe remaining files in one exact Fast IMG working copy without cleaning it."
    )]
    fast_img_cleanup_inventory: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PRIVATE_SEARCH_DIRECTORY",
        conflicts_with_all = ["verify", "fast_img_marker_json", "fast_img_jpeg_execution", "fast_img_cleanup_inventory", "fast_img_delivery", "fast_img_restore", "logs", "output", "session_audit", "print_integrity_summary", "print_integrity_json"],
        help = "Read the ownership reservation of an exact Fast IMG full JPEG effort scratch directory."
    )]
    fast_img_jpeg_scratch: Option<PathBuf>,

    #[arg(
        long,
        conflicts_with_all = ["verify", "fast_img_marker_json", "fast_img_jpeg_execution", "fast_img_cleanup_inventory", "fast_img_jpeg_scratch", "fast_img_delivery", "fast_img_restore", "logs", "output", "session_audit", "print_integrity_summary", "print_integrity_json"],
        help = "List bounded Fast IMG full JPEG effort scratch reservations without changing them."
    )]
    fast_img_jpeg_scratch_list: bool,

    #[arg(
        long,
        value_name = "WORKING_COPY",
        group = "encoder_execution",
        requires = "source_relative",
        conflicts_with_all = ["verify", "fast_img_marker_json", "fast_img_cleanup_inventory", "fast_img_jpeg_scratch", "fast_img_jpeg_scratch_list", "fast_img_delivery", "fast_img_restore", "logs", "output", "session_audit", "print_integrity_summary", "print_integrity_json"],
        help = "Page through historical Fast IMG AVIF encoder evidence for one exact source."
    )]
    fast_img_avif_execution: Option<PathBuf>,

    #[arg(
        long,
        value_name = "WORKING_COPY",
        group = "encoder_execution",
        requires = "source_relative",
        conflicts_with_all = ["verify", "fast_img_marker_json", "fast_img_delivery", "fast_img_restore", "logs", "output", "session_audit", "print_integrity_summary", "print_integrity_json"],
        help = "Print historical Fast IMG JPEG encoder evidence for one exact source."
    )]
    fast_img_jpeg_execution: Option<PathBuf>,

    #[arg(
        long,
        value_name = "WORKING_COPY",
        group = "encoder_execution",
        requires = "source_relative",
        conflicts_with_all = ["verify", "fast_img_marker_json", "fast_img_cleanup_inventory", "fast_img_jpeg_scratch", "fast_img_jpeg_scratch_list", "fast_img_delivery", "fast_img_restore", "logs", "output", "session_audit", "print_integrity_summary", "print_integrity_json", "after_receipt"],
        help = "Read the saved Fast IMG admission for one exact source."
    )]
    fast_img_admission: Option<PathBuf>,

    #[arg(
        long,
        value_name = "WORKING_COPY",
        group = "encoder_execution",
        requires = "source_relative",
        conflicts_with_all = ["verify", "fast_img_marker_json", "fast_img_cleanup_inventory", "fast_img_jpeg_scratch", "fast_img_jpeg_scratch_list", "fast_img_delivery", "fast_img_restore", "logs", "output", "session_audit", "print_integrity_summary", "print_integrity_json", "after_receipt"],
        help = "Read committed Fast IMG delivery and cleanup evidence for one exact source."
    )]
    fast_img_source_evidence: Option<PathBuf>,

    #[arg(
        long,
        value_name = "SOURCE_KEY",
        requires = "encoder_execution",
        help = "Exact relative source path recorded by Fast IMG."
    )]
    source_relative: Option<String>,

    #[arg(
        long,
        value_name = "CURSOR",
        requires = "encoder_execution",
        help = "Page through encoder evidence. Use 'start' or the previous page's next_cursor."
    )]
    after_receipt: Option<String>,

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

fn file_content_blake3(path: &Path) -> (Option<String>, Option<String>) {
    match calculate_blake3_hash(path) {
        Ok(digest) => (Some(digest), None),
        Err(error) => (None, Some(error.to_string())),
    }
}

fn collect_media_files(
    directory: &Path,
    processing_mode: &str,
) -> Result<BTreeMap<String, Vec<PathBuf>>> {
    let mut result: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let canonical_dir = directory.canonicalize().context("canonicalize directory")?;
    for entry in walkdir::WalkDir::new(&canonical_dir) {
        foundation::batch_control::checkpoint()?;
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
    for (index, entry) in walkdir::WalkDir::new(&canonical).into_iter().enumerate() {
        if index % 64 == 0 {
            foundation::batch_control::checkpoint()?;
        }
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
    reconstruction_probe_errors: usize,
    restore_manifest_errors: usize,
    verified_deleted_sources: usize,
    count_status_label: Option<String>,
    tier2_recorded: usize,
    tier2_verified_deleted: usize,
    verified_local_aae: usize,
    photos_local_aae_retention: bool,
}

fn integrity_count_status(stats: &IntegrityStats) -> String {
    let unresolved = stats.source_probe_errors
        + stats.optimized_probe_errors
        + stats.reconstruction_probe_errors;
    if unresolved > 0 {
        return format!("UNCONFIRMED ({unresolved} file classifications unresolved)");
    }
    let delta = stats.count_delta;
    let mut delta_text = "MATCH".to_string();
    if stats.count_matches_with_handoff {
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
        let unexplained = delta.unsigned_abs().saturating_sub(explained);
        if unexplained > 0 {
            delta_text = format!(
                "MISMATCH ({} {direction}; {unexplained} still unexplained)",
                delta.abs()
            );
        } else {
            delta_text = format!("MISMATCH ({} {direction} in optimized)", delta.abs());
        }
    }
    delta_text
}

fn load_fast_img_marker_for_optimized(
    optimized_dir: &Path,
) -> (Option<serde_json::Value>, Option<PathBuf>, Option<String>) {
    let database = match foundation::fast_img_store::database_path() {
        Ok(path) => path,
        Err(error) => {
            return (
                None,
                None,
                Some(format!("fast-img state database unavailable: {error}")),
            );
        }
    };
    match foundation::pipeline::verification::read_marker(optimized_dir) {
        Ok(marker) => match serde_json::to_value(marker) {
            Ok(value) => (Some(value), Some(database), None),
            Err(error) => (
                None,
                Some(database),
                Some(format!("fast-img diagnostic export failed: {error}")),
            ),
        },
        Err(error) => (
            None,
            Some(database),
            Some(format!(
                "fast-img state unavailable for {}: {error}",
                optimized_dir.display()
            )),
        ),
    }
}

fn print_fast_img_marker_json(optimized_dir: &Path) -> Result<()> {
    let (marker, marker_path, marker_error) = load_fast_img_marker_for_optimized(optimized_dir);
    let payload = serde_json::json!({
        "marker": marker,
        "marker_database_path": marker_path,
        "marker_working_copy": optimized_dir,
        "marker_error": marker_error,
    });
    println!("{}", serde_json::to_string(&payload)?);
    Ok(())
}

use foundation::restore_jpeg_manifest::RestoreJpegManifestRecord;

fn load_restore_jpeg_manifest(
    restored_dir: &Path,
) -> (Vec<RestoreJpegManifestRecord>, Vec<String>) {
    foundation::restore_jpeg_manifest::load_manifest(restored_dir)
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
                    let path = foundation::pipeline::verification::canonical_working_copy_path(
                        source_dir,
                    )?;
                    Ok((path, false))
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
    photos_cleanup_recovery: Option<&str>,
) -> Result<IntegrityStats> {
    run_fast_img_delivery_check_for_scope(
        source_dir,
        optimized_dir,
        report,
        processing_mode,
        strategy,
        photos_cleanup_recovery,
        |marker| {
            let _custody = foundation::fast_img::verify_output_photos_custody(marker, true)?;
            Ok(())
        },
    )
}

fn verified_fast_img_aae_outputs(
    source: &Path,
    output: &Path,
    strategy: &str,
    marker: Option<&serde_json::Value>,
    media: &[PathBuf],
    unexpected: &[(PathBuf, String)],
) -> Result<(HashSet<PathBuf>, bool, Vec<PathBuf>)> {
    use foundation::fast_img_store::{FastImgAdmissionSidecar, read_admission_plan};
    use foundation::pipeline::verification::{
        WorkingCopyMarker, validate_no_symlink_components, verify_regular_file_blake3,
    };

    let sidecars: Vec<_> = unexpected
        .iter()
        .filter(|(_, reason)| reason == "unverified AAE companion")
        .map(|(path, _)| path)
        .collect();
    let Some(marker) = marker
        .and_then(|value| serde_json::from_value::<WorkingCopyMarker>(value.clone()).ok())
        .filter(|marker| {
            marker.is_terminal()
                && marker.src_dir == source
                && marker.working_copy == output
                && marker.strategy == strategy
        })
    else {
        return Ok((HashSet::new(), false, Vec::new()));
    };
    let admission = read_admission_plan(&marker).context(IntegrityUnavailableReason::Checkpoint)?;
    if sidecars.is_empty()
        && admission.as_ref().is_none_or(|plan| {
            marker.blake3_log.keys().all(|key| {
                !matches!(
                    plan.get(key).and_then(|entry| entry.aae.as_ref()),
                    Some(
                        FastImgAdmissionSidecar::Present { .. }
                            | FastImgAdmissionSidecar::Unresolved
                    )
                )
            })
        })
    {
        return Ok((HashSet::new(), false, Vec::new()));
    }

    let media: HashSet<&Path> = media.iter().map(PathBuf::as_path).collect();
    let sidecar_keys: HashSet<_> = sidecars
        .iter()
        .map(|path| path.with_extension("aae"))
        .collect();
    let mut owners = HashMap::new();
    let mut source_stems = HashMap::new();
    for source_rel in marker
        .blake3_log
        .keys()
        .chain(marker.skipped_sources.keys())
        .chain(marker.failed_sources.keys())
        .chain(marker.tier2_failed_sources.keys())
        .chain(
            marker
                .tier2_imported_assets
                .iter()
                .map(|asset| &asset.rel_path),
        )
    {
        let path = Path::new(source_rel);
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            let key = path.with_file_name(stem.to_lowercase());
            *source_stems.entry(key).or_insert(0usize) += 1;
        }
    }
    for (source_rel, delivery) in &marker.blake3_log {
        let Some(out_rel) = delivery.out_rel.as_deref() else {
            continue;
        };
        let target = output.join(out_rel);
        let key = target.with_extension("aae");
        if sidecar_keys.contains(&key) && media.contains(target.as_path()) {
            owners
                .entry(key)
                .and_modify(|record| *record = None)
                .or_insert(Some((source_rel, delivery, target)));
        }
    }

    let mut verified = HashSet::new();
    for sidecar in sidecars {
        foundation::batch_control::checkpoint()?;
        let Some(Some((source_rel, delivery, target))) = owners.get(&sidecar.with_extension("aae"))
        else {
            continue;
        };
        let proof = (|| -> Result<()> {
            let expected = admission
                .as_ref()
                .and_then(|plan| plan.get(*source_rel))
                .and_then(|entry| entry.aae.as_ref());
            let source_rel = Path::new(source_rel);
            let stem = source_rel
                .file_stem()
                .and_then(|stem| stem.to_str())
                .context("AAE source stem")?;
            anyhow::ensure!(
                source_stems.get(&source_rel.with_file_name(stem.to_lowercase())) == Some(&1)
            );
            validate_no_symlink_components(source, source_rel).map_err(anyhow::Error::msg)?;
            let source_media = source.join(source_rel);
            foundation::safety::check_apple_photos_library(&source_media)
                .map_err(anyhow::Error::msg)?;
            let source_aae = foundation::metadata::find_aae_sidecar(&source_media)?
                .context("recorded source AAE is unavailable")?;
            anyhow::ensure!(
                foundation::metadata::aae_sidecar_destination(&source_aae, target)? == *sidecar
            );
            validate_no_symlink_components(source, source_aae.strip_prefix(source)?)
                .map_err(anyhow::Error::msg)?;
            validate_no_symlink_components(output, target.strip_prefix(output)?)
                .map_err(anyhow::Error::msg)?;
            validate_no_symlink_components(output, sidecar.strip_prefix(output)?)
                .map_err(anyhow::Error::msg)?;
            verify_regular_file_blake3(target, &delivery.out)?;
            if let Some(expected) = expected {
                let FastImgAdmissionSidecar::Present {
                    relative_path,
                    blake3,
                } = expected
                else {
                    anyhow::bail!("output AAE was not resolved at admission");
                };
                anyhow::ensure!(source_aae.as_os_str() == source.join(relative_path).as_os_str());
                verify_regular_file_blake3(&source_aae, blake3)?;
                verify_regular_file_blake3(sidecar, blake3)?;
            } else {
                foundation::metadata::verify_aae_sidecar_copy(&source_aae, sidecar)?;
            }
            Ok(())
        })();
        if proof.is_ok() {
            verified.insert(sidecar.clone());
        }
    }
    let photos = !marker.photos_imported_assets.is_empty()
        || !marker.tier2_imported_assets.is_empty()
        || marker.photos_library_path.is_some();
    let mut unresolved = Vec::new();
    if let Some(admission) = admission {
        for (source_rel, delivery) in &marker.blake3_log {
            foundation::batch_control::checkpoint()?;
            let sidecar = admission
                .get(source_rel)
                .and_then(|entry| entry.aae.as_ref());
            let safe = match sidecar {
                Some(FastImgAdmissionSidecar::Present { relative_path, .. }) => {
                    delivery.out_rel.as_ref().is_some_and(|out_rel| {
                        foundation::metadata::aae_sidecar_destination(
                            &source.join(relative_path),
                            &output.join(out_rel),
                        )
                        .is_ok_and(|sidecar| verified.contains(&sidecar))
                    })
                }
                Some(FastImgAdmissionSidecar::Unresolved) => false,
                _ => true,
            };
            if !safe {
                unresolved.push(PathBuf::from(source_rel));
            }
        }
        unresolved.sort();
    }
    Ok((verified, photos, unresolved))
}

#[cfg(test)]
fn run_fast_img_delivery_check_with(
    source_dir: &Path,
    optimized_dir: &Path,
    report: &mut String,
    processing_mode: &str,
    strategy: &str,
    verify_photos: impl FnOnce(&foundation::pipeline::verification::WorkingCopyMarker) -> Result<()>,
) -> Result<IntegrityStats> {
    run_fast_img_delivery_check_for_scope(
        source_dir,
        optimized_dir,
        report,
        processing_mode,
        strategy,
        None,
        verify_photos,
    )
}

fn run_fast_img_delivery_check_for_scope(
    source_dir: &Path,
    optimized_dir: &Path,
    report: &mut String,
    processing_mode: &str,
    strategy: &str,
    photos_cleanup_recovery: Option<&str>,
    verify_photos: impl FnOnce(&foundation::pipeline::verification::WorkingCopyMarker) -> Result<()>,
) -> Result<IntegrityStats> {
    foundation::batch_control::checkpoint()?;
    let recovery_marker = photos_cleanup_recovery
        .map(|expected| {
            use dev::infra::drag_drop::{
                exact_fast_img_photos_cleanup_recovery, fast_img_cleanup_checkpoint_digest,
            };
            let marker =
                exact_fast_img_photos_cleanup_recovery(source_dir, optimized_dir, strategy)?;
            anyhow::ensure!(
                fast_img_cleanup_checkpoint_digest(&marker)? == expected,
                "Fast IMG cleanup checkpoint changed before independent verification"
            );
            Ok::<_, anyhow::Error>(marker)
        })
        .transpose()
        .context(IntegrityUnavailableReason::Checkpoint)?;
    let _uncached = foundation::common_utils::UncachedVerificationScope::new()
        .context(IntegrityUnavailableReason::UncachedVerification)?;
    let (source_dir, source_dir_exists) = resolve_source_directory_for_post_cleanup(source_dir)
        .context(IntegrityUnavailableReason::DirectoryPair)?;
    let (optimized_dir, optimized_dir_exists) = match fs::symlink_metadata(optimized_dir) {
        Ok(metadata) if metadata.is_dir() => {
            let path = optimized_dir
                .canonicalize()
                .context(IntegrityUnavailableReason::DirectoryPair)?;
            foundation::safety::check_apple_photos_library(&path)
                .map_err(anyhow::Error::msg)
                .context(IntegrityUnavailableReason::DirectoryPair)?;
            (path, true)
        }
        Ok(_) => {
            return Err(anyhow::anyhow!("optimized path is not a real directory")
                .context(IntegrityUnavailableReason::DirectoryPair));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            foundation::pipeline::verification::canonical_working_copy_path(optimized_dir)
                .context(IntegrityUnavailableReason::DirectoryPair)?,
            false,
        ),
        Err(error) => {
            return Err(
                anyhow::Error::from(error).context(IntegrityUnavailableReason::DirectoryPair)
            );
        }
    };

    let (marker_opt, _path_opt, marker_error) = load_fast_img_marker_for_optimized(&optimized_dir);
    let failed_retry = foundation::fast_img_store::failed_retry_context(&optimized_dir)
        .context(IntegrityUnavailableReason::Checkpoint)?;
    let routed_child = if let Some(context) = &failed_retry {
        let typed = foundation::pipeline::verification::read_marker(&optimized_dir)
            .context(IntegrityUnavailableReason::Checkpoint)?;
        anyhow::ensure!(
            typed.working_copy == optimized_dir
                && typed.src_dir == source_dir
                && typed.strategy == strategy
                && context.parent().src_dir == source_dir
                && typed.failed_retry_manifest_blake3.is_some()
                && context.admissions().len() == typed.src_jpeg_count,
            IntegrityUnavailableReason::Checkpoint
        );
        let marker_value =
            serde_json::to_value(&typed).context(IntegrityUnavailableReason::Checkpoint)?;
        anyhow::ensure!(
            marker_error.is_none() && marker_opt.as_ref() == Some(&marker_value),
            IntegrityUnavailableReason::Checkpoint
        );
        typed
            .failed_retry_primary_source_count
            .is_some()
            .then_some(typed)
    } else {
        None
    };

    report.push_str("── FAST-IMG DELIVERY VERIFICATION ─────────────────────────────\n");
    report.push_str(&format!("Source:    {}\n", source_dir.display()));
    report.push_str(&format!("Optimized: {}\n\n", optimized_dir.display()));

    let mut stats = IntegrityStats {
        source: source_dir.to_string_lossy().to_string(),
        optimized: optimized_dir.to_string_lossy().to_string(),
        scope: processing_mode.to_string(),
        optimized_path_label: "Optimized".to_string(),
        source_files_label: if routed_child.is_some() {
            "Recorded failed-only admissions".to_string()
        } else if strategy == "avif" {
            "Recorded source static images".to_string()
        } else {
            "Recorded JPEG-derived sources".to_string()
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
        if let Some(context) = &failed_retry {
            let mut paths = Vec::new();
            for admission in context.admissions().values() {
                foundation::batch_control::checkpoint()?;
                let path = source_dir.join(&admission.source_rel);
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                        paths.push(path)
                    }
                    Ok(_) => anyhow::bail!(
                        "failed-only verifier source is not a regular non-symlink file: {}",
                        path.display()
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).context("inspect failed-only verification source");
                    }
                }
            }
            paths
        } else {
            collect_regular_files(&source_dir)?
        }
    } else {
        Vec::new()
    };
    for path in source_files {
        foundation::batch_control::checkpoint()?;
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
    if optimized_dir_exists {
        let mut entries = WalkDir::new(&optimized_dir)
            .follow_root_links(false)
            .into_iter();
        while let Some(entry) = entries.next() {
            foundation::batch_control::checkpoint()?;
            let entry = entry?;
            let file_type = entry.file_type();
            if entry.depth() == 0 {
                anyhow::ensure!(
                    file_type.is_dir(),
                    IntegrityUnavailableReason::DirectoryPair
                );
                continue;
            }
            if file_type.is_dir() {
                if entry.file_name().to_str().is_none_or(|name| {
                    let lower = name.to_ascii_lowercase();
                    name.starts_with('.')
                        || lower.ends_with(".photoslibrary")
                        || lower.ends_with(".photolibrary")
                }) {
                    entries.skip_current_dir();
                    unexpected_outputs
                        .push((entry.into_path(), "uninspected directory".to_owned()));
                }
                continue;
            }
            let path = entry.into_path();
            if !file_type.is_file() {
                unexpected_outputs.push((path, "nonregular entry".to_owned()));
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("aae"))
            {
                unexpected_outputs.push((path, "unverified AAE companion".to_owned()));
            } else if path.file_name().is_some_and(|name| name == ".DS_Store") {
                match foundation::io_utils::is_finder_metadata(&path) {
                    Ok(true) => {}
                    Ok(false) => {
                        unexpected_outputs.push((path, "unrecognized Finder metadata".to_owned()))
                    }
                    Err(error) => optimized_probe_errors.push((path, error.into())),
                }
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| name.starts_with('.'))
            {
                unexpected_outputs.push((path, "unrecognized file name".to_owned()));
            } else {
                match foundation::image::format_detect::detect_regular_file_format(&path) {
                    Ok(format) if format.canonical_extension() == Some(strategy) => {
                        optimized_outputs.push(path)
                    }
                    Ok(format) => unexpected_outputs.push((
                        path,
                        format!(
                            "detected true format: {}",
                            format.canonical_extension().unwrap_or("unknown")
                        ),
                    )),
                    Err(error) => optimized_probe_errors.push((path, error.into())),
                }
            }
        }
    }
    let (verified_aae, photos_aae, unresolved_aae) = verified_fast_img_aae_outputs(
        &source_dir,
        &optimized_dir,
        strategy,
        marker_opt.as_ref(),
        &optimized_outputs,
        &unexpected_outputs,
    )?;
    unexpected_outputs.retain(|(path, _)| !verified_aae.contains(path));
    if !unresolved_aae.is_empty() {
        report.push_str("Recorded source AAE lacks a matching current local copy:\n");
        for path in &unresolved_aae {
            report.push_str(&format!("  - {:?}\n", path));
        }
    }

    let recovery_outputs = recovery_marker
        .as_ref()
        .map(|marker| {
            foundation::fast_img::build_fast_img_output_import_candidates(marker).map(
                |candidates| {
                    candidates
                        .into_iter()
                        .map(|candidate| candidate.path)
                        .collect::<HashSet<_>>()
                },
            )
        })
        .transpose()
        .context(IntegrityUnavailableReason::Checkpoint)?;
    let recorded_outputs = || {
        let records = marker_opt.as_ref()?.get("blake3_log")?.as_object()?;
        if records.is_empty() {
            return None;
        }
        records
            .values()
            .map(|record| Some(optimized_dir.join(record.get("out_rel")?.as_str()?)))
            .collect::<Option<HashSet<_>>>()
    };
    let mut missing_recorded_outputs = Vec::new();
    let mut observed_absent_outputs = 0;
    if let Some(mut expected) =
        recovery_outputs.or_else(|| optimized_dir_exists.then(recorded_outputs).flatten())
    {
        optimized_outputs.retain(|path| {
            if expected.remove(path) {
                return true;
            }
            unexpected_outputs.push((
                path.clone(),
                "no matching delivery checkpoint entry".to_owned(),
            ));
            false
        });
        for path in expected {
            foundation::batch_control::checkpoint()?;
            if recovery_marker.is_some()
                && matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            {
                observed_absent_outputs += 1;
            } else {
                missing_recorded_outputs.push(path);
            }
        }
        missing_recorded_outputs.sort();
    }

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
    let mut tier2_unresolved = Vec::new();
    let mut tier2_retained = 0;
    let mut tier2_failed_count = 0;

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
        if let Some(failed) = m
            .get("tier2_failed_sources")
            .and_then(|value| value.as_object())
        {
            tier2_failed_count = failed.len();
            if routed_child.is_none() {
                for (rel, failure) in failed {
                    let verification = (|| -> Result<()> {
                        foundation::pipeline::verification::validate_no_symlink_components(
                            &source_dir,
                            Path::new(rel),
                        )
                        .map_err(anyhow::Error::msg)?;
                        let source = source_dir.join(rel);
                        let expected = failure
                            .get("src")
                            .and_then(serde_json::Value::as_str)
                            .context("retained original failure has no recorded hash")?;
                        anyhow::ensure!(
                            fs::symlink_metadata(&source)?.is_file(),
                            "retained original is not a regular file"
                        );
                        anyhow::ensure!(
                            calculate_blake3_hash(&source)? == expected,
                            "retained original differs from its recorded hash"
                        );
                        Ok(())
                    })();
                    match verification {
                        Ok(()) => tier2_retained += 1,
                        Err(error) => tier2_unresolved.push((rel.as_str(), format!("{error:#}"))),
                    }
                }
            }
        }
    }

    if let Some((context, typed)) = failed_retry.as_ref().zip(routed_child.as_ref()) {
        use foundation::fast_img_store::{
            FailedRetrySidecar, FailedRetrySourceRoute, verify_failed_retry_admission,
            verify_failed_retry_retained_companions,
        };

        recorded_source_jpegs = u64::try_from(typed.primary_source_count())?;
        let receipts = typed
            .tier2_imported_assets
            .iter()
            .map(|asset| asset.rel_path.as_str())
            .collect::<HashSet<_>>();
        let originals_still_present = tier2_unexpected_remaining
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let absent = foundation::fast_img_store::validate_tier2_cleanup_resume(typed)
            .context(IntegrityUnavailableReason::Checkpoint)?;
        for admission in context
            .admissions()
            .values()
            .filter(|admission| admission.route == Some(FailedRetrySourceRoute::Original))
        {
            let rel = &admission.source_rel;
            match (
                receipts.contains(rel.as_str()),
                typed.tier2_failed_sources.get(rel),
            ) {
                (true, None) if absent.contains(rel) => {
                    let verification = (|| -> Result<()> {
                        if let FailedRetrySidecar::Present { relative_path, .. } = &admission.xmp {
                            match fs::symlink_metadata(source_dir.join(relative_path)) {
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                                Err(error) => {
                                    return Err(error)
                                        .context("inspect saved original-format XMP companion");
                                }
                                Ok(_) => anyhow::bail!(
                                    "saved original-format XMP companion still awaits cleanup"
                                ),
                            }
                        }
                        verify_failed_retry_retained_companions(typed, admission, true)
                    })();
                    if let Err(error) = verification {
                        tier2_unresolved.push((rel.as_str(), format!("{error:#}")));
                    }
                }
                (true, None) if originals_still_present.contains(rel.as_str()) => {}
                (false, Some(failure)) => {
                    if failure.src != admission.admitted_source_blake3 {
                        tier2_unresolved.push((
                            rel.as_str(),
                            "retained failure identity differs from its admission".into(),
                        ));
                        continue;
                    }
                    match verify_failed_retry_admission(&source_dir, admission) {
                        Ok(()) => tier2_retained += 1,
                        Err(error) => tier2_unresolved.push((rel.as_str(), format!("{error:#}"))),
                    }
                }
                _ => tier2_unresolved.push((
                    rel.as_str(),
                    "missing exact delivery or retained failure evidence".into(),
                )),
            }
        }
        if !typed.is_terminal() && tier2_unresolved.is_empty() {
            report.push_str(
                "  - The failed-only child still has pending delivery or source cleanup\n",
            );
        }
    }

    if strategy == "avif" {
        source_true_jpegs.clear();
    }
    // Recorded target-format inputs are in scope even when they are not JPEGs.
    source_true_jpegs.extend(
        successful_source_rels
            .iter()
            .chain(&skipped_source_rels)
            .chain(&failed_source_rels)
            .map(|rel| source_dir.join(rel))
            .filter(|path| path.is_file()),
    );
    source_true_jpegs.sort();
    source_true_jpegs.dedup();

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

    stats.source_files = routed_child
        .as_ref()
        .map_or(recorded_source_jpegs as usize, |marker| {
            marker.src_jpeg_count
        });
    stats.optimized_files = optimized_outputs.len();
    stats.skipped_sources = skipped_source_rels.len();
    stats.failed_sources = failed_source_rels.len() + tier2_failed_count;
    stats.source_remaining_files = source_true_jpegs.len() + tier2_retained;
    stats.source_probe_errors = source_probe_errors.len();
    stats.optimized_probe_errors = optimized_probe_errors.len();
    stats.extra = unexpected_outputs.len() + optimized_probe_errors.len();
    stats.missing = missing_recorded_outputs.len();
    stats.verified_local_aae = verified_aae.len();
    stats.photos_local_aae_retention = photos_aae && !verified_aae.is_empty();
    let accounted_delivery_count = optimized_outputs.len() + observed_absent_outputs;
    stats.count_delta = (accounted_delivery_count as isize) - (expected_delivery_count as isize);
    stats.matched = optimized_outputs.len();
    stats.explained_gaps = skipped_source_rels.len() + failed_source_rels.len() + tier2_retained;
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
    integrity_failures += missing_recorded_outputs.len() + unresolved_aae.len();
    if marker_opt.is_some() && expected_delivery_count != accounted_delivery_count {
        integrity_failures += 1;
    }
    if accounted_delivery_count == 0 && expected_delivery_count > 0 {
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
    integrity_failures += tier2_unresolved.len();
    if routed_child
        .as_ref()
        .is_some_and(|marker| !marker.is_terminal())
    {
        integrity_failures += 1;
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

    let count_matches = marker_ok
        && expected_delivery_count == accounted_delivery_count
        && missing_recorded_outputs.is_empty()
        && unresolved_aae.is_empty()
        && unexpected_source_true_jpegs.is_empty()
        && skipped_sources_missing.is_empty()
        && failed_sources_missing.is_empty()
        && tier2_missing_proof.is_empty()
        && tier2_unexpected_remaining.is_empty()
        && tier2_unresolved.is_empty()
        && routed_child
            .as_ref()
            .is_none_or(|marker| marker.is_terminal())
        && source_probe_errors.is_empty()
        && optimized_probe_errors.is_empty();
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
        "JPEG-derived files"
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
        stats.failed_sources
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
    if !verified_aae.is_empty() {
        report.push_str(&format!(
            "Verified local AAE companions: {}\n",
            verified_aae.len()
        ));
        if stats.photos_local_aae_retention {
            report.push_str("  - Retaining local media and AAE companions because Photos custody covers the media only.\n");
        }
    }
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
    if tier2_failed_count > 0 || routed_child.is_some() {
        report.push_str(&format!(
            "Verified retained original-format failures: {tier2_retained}\n"
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
            report.push_str(&format!(
                "  - {} unverified output entries retained in the optimized directory:\n",
                unexpected_outputs.len()
            ));
            for (p, reason) in &unexpected_outputs {
                report.push_str(&format!(
                    "      - {:?} ({reason})\n",
                    p.strip_prefix(&optimized_dir)?,
                ));
            }
        }
        for missing in &missing_recorded_outputs {
            report.push_str(&format!(
                "  - Recorded output unavailable: {:?}\n",
                missing.strip_prefix(&optimized_dir)?,
            ));
        }
        if marker_opt.is_some() && expected_delivery_count != accounted_delivery_count {
            let ext_name = if strategy == "avif" { "AVIF" } else { "JXL" };
            report.push_str(&format!(
                "  - Delivered {ext_name} count mismatch: expected {expected_delivery_count} but accounted for {accounted_delivery_count}\n",
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
        if !tier2_unresolved.is_empty() {
            report.push_str(&format!("  - {} original-format file(s) have no confirmed delivery or retained failure proof:\n", tier2_unresolved.len()));
            for (rel, reason) in &tier2_unresolved {
                report.push_str(&format!("      - {rel:?}: {}\n", reason.escape_debug()));
            }
        }
    } else {
        let typed = foundation::pipeline::verification::read_marker(&optimized_dir)
            .context(IntegrityUnavailableReason::Checkpoint)?;
        let marker_value =
            serde_json::to_value(&typed).context(IntegrityUnavailableReason::Checkpoint)?;
        anyhow::ensure!(
            typed.working_copy == optimized_dir
                && typed.src_dir == source_dir
                && typed.strategy == strategy
                && marker_opt.as_ref() == Some(&marker_value),
            IntegrityUnavailableReason::Checkpoint
        );
        if let Some(expected) = &recovery_marker {
            anyhow::ensure!(
                &typed == expected
                    && matches!(fs::symlink_metadata(&source_dir),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                IntegrityUnavailableReason::Checkpoint
            );
        }
        if !typed.photos_imported_assets.is_empty() || !typed.tier2_imported_assets.is_empty() {
            verify_photos(&typed).context(IntegrityUnavailableReason::PhotosCustody)?;
            foundation::batch_control::checkpoint()?;
            report.push_str("  - Current Photos UUIDs and complete original-resource bytes were freshly verified\n");
        }
        foundation::fast_img_store::verify_tier2_cleanup_complete(&typed)
            .context(IntegrityUnavailableReason::Checkpoint)?;
        report.push_str(&format!(
            "{} FAST-IMG DELIVERY INVARIANTS PASS\n",
            pick_symbol("✓", "[OK]")
        ));
        if recovery_marker.is_some() {
            report.push_str("  - Saved source directory is absent. This recovery did not delete source files.\n");
            report.push_str(&format!(
                "  - Recorded local outputs absent now: {observed_absent_outputs}. Current Photos originals cover them.\n"
            ));
        } else {
            report.push_str(&format!(
                "  - All non-skipped/non-failed source {source_kind} were successfully deleted\n"
            ));
        }
        report.push_str(&format!("  - Verified {ext_name} delivery inventory\n"));
        if tier2_recorded > 0 && recovery_marker.is_none() {
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
        foundation::batch_control::checkpoint()?;
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
                    match source_outputs.entry(integrity_stem_key(rel)) {
                        Entry::Vacant(entry) => {
                            entry.insert(path);
                        }
                        Entry::Occupied(entry) => source_probe_errors.push((
                            path,
                            anyhow::anyhow!(
                                "ambiguous source JXL stem also used by {}",
                                entry.get().display()
                            ),
                        )),
                    }
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
        foundation::batch_control::checkpoint()?;
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
                    match restored_jpeg.entry(integrity_stem_key(rel)) {
                        Entry::Vacant(entry) => {
                            entry.insert(path);
                        }
                        Entry::Occupied(entry) => restored_probe_errors.push((
                            path,
                            anyhow::anyhow!(
                                "ambiguous restored JPEG stem also used by {}",
                                entry.get().display()
                            ),
                        )),
                    }
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
        foundation::batch_control::checkpoint()?;
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

    for (key, path) in &restored_jpeg {
        if manifest_sources
            .get(key)
            .is_none_or(|record| restored_dir.join(&record.output_rel) != *path)
        {
            restore_manifest_errors.push(format!(
                "restored JPEG has no valid reconstruction manifest proof: {}",
                path.strip_prefix(&restored_dir)?.display()
            ));
        }
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
        foundation::batch_control::checkpoint()?;
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

    let source_candidate_count = expected_keys.len()
        + retained_ineligible.len()
        + reconstruction_probe_errors.len()
        + source_probe_errors.len();
    stats.source_files = source_candidate_count;
    stats.source_remaining_files = source_outputs.len() + source_probe_errors.len();
    stats.verified_deleted_sources = manifest_deleted_sources.len();
    stats.optimized_files = restored_jpeg.len();
    stats.source_probe_errors = source_probe_errors.len();
    stats.optimized_probe_errors = restored_probe_errors.len();
    stats.reconstruction_probe_errors = reconstruction_probe_errors.len();
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

    let count_matches = stats.count_delta == 0
        && missing_keys.is_empty()
        && extra_keys.is_empty()
        && non_jpeg_outputs.is_empty();
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

    if !manifest_sources.is_empty() {
        let mut records = manifest_sources.values().collect::<Vec<_>>();
        records.sort_unstable_by(|left, right| left.source_rel.cmp(&right.source_rel));
        report.push_str("--- Historical restoration verification ---\n");
        for record in records {
            let evidence = match &record.verification {
                Some(verified) => format!(
                    "Unix seconds {}, MFB {:?}, djxl observation {:?}",
                    verified.verified_unix_seconds, verified.mfb_version, verified.djxl_version,
                ),
                None => "unknown (legacy manifest)".to_owned(),
            };
            report.push_str(&format!("  - {:?}: {evidence}\n", record.source_rel));
        }
        report.push('\n');
    }

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

    for directory in [source_dir, optimized_dir] {
        let metadata = fs::metadata(directory)
            .with_context(|| format!("inspect verification directory {}", directory.display()))
            .context(IntegrityUnavailableReason::DirectoryPair)?;
        anyhow::ensure!(metadata.is_dir(), IntegrityUnavailableReason::DirectoryPair);
    }

    let _uncached = foundation::common_utils::UncachedVerificationScope::new()
        .context(IntegrityUnavailableReason::UncachedVerification)?;
    let source_root = source_dir
        .canonicalize()
        .context(IntegrityUnavailableReason::DirectoryPair)?;
    let output_root = optimized_dir
        .canonicalize()
        .context(IntegrityUnavailableReason::DirectoryPair)?;
    let source_files = collect_media_files(&source_root, processing_mode)?;
    let optimized_files = collect_media_files(&output_root, processing_mode)?;

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
    foundation::batch_control::checkpoint()?;
    let rust_outcomes = match load_rust_outcomes_from_logs(&log_paths, Some(source_dir)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Warning: Failed to load rust outcomes: {e}");
            HashMap::new()
        }
    };

    foundation::batch_control::checkpoint()?;
    let source_dir = source_root.as_path();
    let optimized_dir = output_root.as_path();
    let mut true_missing = Vec::new();
    let mut pipeline_handoff = Vec::new();
    let mut vid_pipeline_failed = Vec::new();
    let mut vid_pipeline_unverified = Vec::new();
    let mut matched = Vec::new();
    let mut extra = Vec::new();
    let mut ambiguous = Vec::new();

    for (key, src_paths) in &source_files {
        foundation::batch_control::checkpoint()?;
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
    let unmatched_source_collisions = src_collisions
        .keys()
        .filter(|key| !optimized_files.contains_key(*key))
        .count();
    stats.integrity_failures =
        integrity_failures + extra.len() + ambiguous.len() + unmatched_source_collisions;
    stats.has_warnings = stats.integrity_failures != 0;

    if !src_collisions.is_empty() {
        report
            .push_str("⚠️ WARNING: Duplicate source stems detected (Unsafe for 1-to-1 mapping):\n");
        for (key, paths) in &src_collisions {
            foundation::batch_control::checkpoint()?;
            let mut hash_results = Vec::new();
            for p in paths {
                foundation::batch_control::checkpoint()?;
                hash_results.push(file_content_blake3(p));
            }
            let unique_hashes: HashSet<String> =
                hash_results.iter().filter_map(|(h, _)| h.clone()).collect();
            let label = if hash_results.iter().any(|(hash, _)| hash.is_none()) {
                "content equality unavailable".to_string()
            } else if unique_hashes.len() == 1 {
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
            foundation::batch_control::checkpoint()?;
            let mut hash_results = Vec::new();
            for p in paths {
                foundation::batch_control::checkpoint()?;
                hash_results.push(file_content_blake3(p));
            }
            let unique_hashes: HashSet<String> =
                hash_results.iter().filter_map(|(h, _)| h.clone()).collect();
            let label = if hash_results.iter().any(|(hash, _)| hash.is_none()) {
                "content equality unavailable".to_string()
            } else if unique_hashes.len() == 1 {
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
    } else if !stats.has_warnings {
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
    let expected =
        isize::try_from(explained).context("handoff count exceeds verification range")?;
    stats.expected_count_delta = -expected;
    stats.count_matches_with_handoff = !stats.has_warnings && delta == -expected;
    stats.count_fully_explained = stats.count_matches_with_handoff;

    Ok(stats)
}

fn collect_bundle_run_logs(log_dir: &Path) -> Result<Vec<PathBuf>> {
    if !log_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = HashSet::new();
    for entry in WalkDir::new(log_dir).into_iter().filter_map(Result::ok) {
        foundation::batch_control::checkpoint()?;
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
                foundation::batch_control::checkpoint()?;
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
        foundation::batch_control::checkpoint()?;
        let log_name = log_file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("log")
            .to_string();
        let content = fs::read_to_string(&log_file)
            .with_context(|| format!("log file unreadable: {}", log_file.display()))?;
        let mut current_file: Option<String> = None;
        for (index, raw_line) in content.lines().enumerate() {
            if index % 256 == 0 {
                foundation::batch_control::checkpoint()?;
            }
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

fn report_integrity_unavailable<T>(result: Result<T>, requested: bool) -> Result<T> {
    if let Err(error) = &result
        && requested
    {
        let reason = error
            .downcast_ref::<IntegrityUnavailableReason>()
            .copied()
            .unwrap_or(IntegrityUnavailableReason::Verification);
        let unavailable = IntegrityUnavailableMachine::Unavailable {
            reason: Some(reason),
        };
        println!(
            "{INTEGRITY_UNAVAILABLE_JSON_PREFIX}{}",
            serde_json::to_string(&unavailable)
                .context("serialize unavailable integrity result")?
        );
    }
    result
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.fast_img_jpeg_scratch_list {
        let report = foundation::jxl_effort_policy::list_jpeg_effort_scratch()?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if let Some(directory) = args.fast_img_jpeg_scratch.as_deref() {
        let report = foundation::jxl_effort_policy::inspect_jpeg_effort_scratch(directory)?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if let Some(output) = args.fast_img_cleanup_inventory.as_deref() {
        let report = dev::infra::drag_drop::inspect_fast_img_cleanup_inventory(output);
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if let Some(working_copy) = args.fast_img_source_evidence.as_deref() {
        let source = args
            .source_relative
            .as_deref()
            .context("missing exact source key")?;
        let report = foundation::fast_img_store::read_source_evidence(working_copy, source)?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if let Some(working_copy) = args.fast_img_admission.as_deref() {
        let source = args
            .source_relative
            .as_deref()
            .context("missing exact source key")?;
        let relative = Path::new(source);
        let validated = foundation::pipeline::verification::fast_img_relative_key(
            &working_copy.join(relative),
            working_copy,
            "admission query source",
        )
        .map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            validated == source,
            "admission query requires the exact relative source key"
        );
        let marker = foundation::pipeline::verification::read_marker(working_copy)?;
        let plan = foundation::fast_img_store::read_admission_plan(&marker)?;
        let admission = plan.as_ref().and_then(|plan| plan.get(source));
        let coverage = match (&plan, admission) {
            (_, Some(_)) => "recorded",
            (Some(_), None) => "not_admitted",
            (None, None) => "unknown",
        };
        let report = serde_json::json!({
            "schema_version": 1,
            "scope": "historical_fast_img_admission",
            "source_relative": source,
            "coverage": coverage,
            "admission": admission,
        });
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if let Some(working_copy) = args
        .fast_img_jpeg_execution
        .as_deref()
        .or(args.fast_img_avif_execution.as_deref())
    {
        let source = args
            .source_relative
            .as_deref()
            .context("missing exact source key")?;
        let after = args
            .after_receipt
            .as_deref()
            .filter(|&value| value != "start");
        let report = if args.fast_img_avif_execution.is_some() || args.after_receipt.is_some() {
            let read_page = if args.fast_img_avif_execution.is_some() {
                foundation::fast_img_store::read_avif_execution_receipt_page
            } else {
                foundation::fast_img_store::read_jpeg_execution_receipt_page
            };
            let page = read_page(working_copy, source, after)?;
            let recorded = after.is_some() || !page.receipts.is_empty();
            serde_json::json!({
                "schema_version": 2,
                "scope": "historical_encoder_invocations",
                "source_relative": source,
                "coverage": if recorded { "recorded" } else { "unknown" },
                "receipts": page.receipts,
                "next_cursor": page.next_cursor,
            })
        } else {
            let receipts = foundation::fast_img_store::read_jpeg_execution_receipts_for_source(
                working_copy,
                source,
            )?;
            serde_json::json!({
                "schema_version": 1,
                "scope": "historical_encoder_invocations",
                "source_relative": source,
                "coverage": if receipts.is_empty() { "unknown" } else { "recorded" },
                "receipts": receipts,
            })
        };
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    report_integrity_unavailable(
        foundation::init_ghost_mode().context(IntegrityUnavailableReason::Startup),
        args.print_integrity_json,
    )?;

    if let Some(ref optimized_dir) = args.fast_img_marker_json {
        return print_fast_img_marker_json(optimized_dir);
    }

    let _production_lease =
        if args.verify.is_some() || args.fast_img_delivery || args.fast_img_restore {
            Some(report_integrity_unavailable(
                foundation::infra::production_lock::acquire_production_lease()
                    .context(IntegrityUnavailableReason::Startup),
                args.print_integrity_json,
            )?)
        } else {
            None
        };

    report_integrity_unavailable(
        foundation::batch_control::checkpoint().map_err(Into::into),
        args.print_integrity_json,
    )?;
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let output_report = if let Some(ref p) = args.output {
        p.clone()
    } else {
        let log_dir = dev::infra::log_paths::unified_log_dir();
        log_dir.join(format!("diagnostic_report_{timestamp}.txt"))
    };
    if let Some(parent) = output_report.parent() {
        report_integrity_unavailable(
            fs::create_dir_all(parent).context(IntegrityUnavailableReason::Report),
            args.print_integrity_json,
        )?;
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
        if args.fast_img_photos_cleanup_recovery.is_some() && verify_args.len() != 2 {
            return report_integrity_unavailable(
                Err(anyhow::Error::new(
                    IntegrityUnavailableReason::DirectoryPair,
                )),
                args.print_integrity_json,
            );
        }
        let resolved = resolve_verify_dirs(verify_args);
        if let Some((src, opt)) = resolved {
            source_dir_context = Some(src.clone());
            if args.fast_img_delivery {
                integrity_stats = Some(report_integrity_unavailable(
                    run_fast_img_delivery_check(
                        &src,
                        &opt,
                        &mut report,
                        &args.mode,
                        &args.strategy,
                        args.fast_img_photos_cleanup_recovery.as_deref(),
                    ),
                    args.print_integrity_json,
                )?);
            } else if args.fast_img_restore {
                integrity_stats = Some(report_integrity_unavailable(
                    run_fast_img_restore_check(&src, &opt, &mut report, &args.strategy),
                    args.print_integrity_json,
                )?);
            } else {
                integrity_stats = Some(report_integrity_unavailable(
                    run_integrity_check(
                        &src,
                        &opt,
                        &mut report,
                        &args.mode,
                        &session_audit_paths,
                        bundle_log_dir.as_deref(),
                        explicit_log_paths.as_deref(),
                    ),
                    args.print_integrity_json,
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
        let (conv_count, unc_count) = report_integrity_unavailable(
            parse_logs(&log_paths_buf, &mut report, source_dir_context.as_deref()),
            args.print_integrity_json,
        )?;
        println!(
            "{} Total conversion events: {}",
            pick_symbol("📈", "[CHART]"),
            conv_count
        );
        println!("🔭 Uncertain loop cases: {unc_count}");
    }

    report_integrity_unavailable(
        foundation::batch_control::checkpoint().map_err(Into::into),
        args.print_integrity_json,
    )?;
    report_integrity_unavailable(
        fs::write(&output_report, &report).context(IntegrityUnavailableReason::Report),
        args.print_integrity_json,
    )?;
    if args.print_integrity_summary {
        if let Some(ref stats) = integrity_stats {
            let delta_text = integrity_count_status(stats);

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
                    "Recorded failed sources:", stats.failed_sources
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
            if stats.photos_local_aae_retention {
                println!(
                    "   Local AAE companions retained: {} (Photos custody covers media only)",
                    stats.verified_local_aae
                );
            }
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
        let stats = report_integrity_unavailable(
            integrity_stats
                .as_ref()
                .context(IntegrityUnavailableReason::DirectoryPair),
            true,
        )?;
        let summary = IntegritySummaryMachine {
            has_warnings: stats.has_warnings,
            issue_count: stats.integrity_failures,
            source_count: stats.source_files,
            optimized_count: stats.optimized_files,
            skipped_count: stats.skipped_sources,
            failed_count: stats.failed_sources,
            source_remaining_count: stats.source_remaining_files,
            verified_deleted_count: stats.verified_deleted_sources,
            count_status: Some(integrity_count_status(stats)),
            retained_local_aae: (stats.photos_local_aae_retention && !stats.has_warnings)
                .then_some(stats.verified_local_aae),
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
    fn fast_img_admission_cli_requires_an_exact_source_and_excludes_encoder_paging() {
        let admission = ["verify", "--fast-img-admission", "/fixture/work"];
        assert!(Args::try_parse_from(admission).is_err());
        let args = admission
            .into_iter()
            .chain(["--source-relative", "nested/素材.jpg"]);
        assert_eq!(
            Args::try_parse_from(args.clone())
                .unwrap()
                .source_relative
                .as_deref(),
            Some("nested/素材.jpg")
        );
        assert!(Args::try_parse_from(args.clone().chain(["--after-receipt", "start"])).is_err());
        assert!(
            Args::try_parse_from(args.chain(["--fast-img-jpeg-execution", "/fixture/work"]))
                .is_err()
        );
    }

    #[test]
    fn source_evidence_cli_is_exact_and_separate_from_delivery_and_encoder_queries() {
        let command = ["verify", "--fast-img-source-evidence", "/fixture/work"];
        assert!(Args::try_parse_from(command).is_err());
        let exact = command
            .into_iter()
            .chain(["--source-relative", "nested/photo.jpg"]);
        assert_eq!(
            Args::try_parse_from(exact.clone())
                .unwrap()
                .fast_img_source_evidence
                .as_deref(),
            Some(Path::new("/fixture/work")),
        );
        for other in [
            ["--fast-img-admission", "/fixture/work"],
            ["--fast-img-jpeg-execution", "/fixture/work"],
            ["--verify", "/fixture/source"],
            ["--after-receipt", "start"],
        ] {
            assert!(Args::try_parse_from(exact.clone().chain(other)).is_err());
        }
    }

    #[test]
    fn fast_img_jpeg_execution_cli_requires_exact_source_and_separate_mode() {
        let args = [
            "verify",
            "--fast-img-jpeg-execution",
            "/fixture/work",
            "--source-relative",
            "nested/photo.jpg",
        ];
        let parsed = Args::try_parse_from(args).unwrap();
        assert_eq!(parsed.source_relative.as_deref(), Some("nested/photo.jpg"));
        assert!(Args::try_parse_from(&args[..3]).is_err());
        assert!(Args::try_parse_from(["verify", "--source-relative", "photo.jpg"]).is_err());
        assert!(
            Args::try_parse_from(args.into_iter().chain(["--verify", "/fixture/input"])).is_err()
        );
        let page =
            Args::try_parse_from(args.into_iter().chain(["--after-receipt", "start"])).unwrap();
        assert_eq!(page.after_receipt.as_deref(), Some("start"));
        assert!(Args::try_parse_from(["verify", "--after-receipt", "start"]).is_err());
    }

    #[test]
    fn fast_img_avif_execution_cli_requires_exact_source_and_separate_mode() {
        let args = [
            "verify",
            "--fast-img-avif-execution",
            "/fixture/work",
            "--source-relative",
            "nested/photo.png",
        ];
        let parsed = Args::try_parse_from(args).unwrap();
        assert_eq!(
            parsed.fast_img_avif_execution.as_deref(),
            Some(Path::new("/fixture/work"))
        );
        assert!(Args::try_parse_from(&args[..3]).is_err());
        assert!(Args::try_parse_from(args.into_iter().chain(["--after-receipt", "start"])).is_ok());
        assert!(
            Args::try_parse_from(
                args.into_iter()
                    .chain(["--fast-img-jpeg-execution", "/fixture/work"])
            )
            .is_err()
        );
        assert!(
            Args::try_parse_from(args.into_iter().chain(["--fast-img-jpeg-scratch-list"])).is_err()
        );
    }

    #[test]
    fn fast_img_cleanup_inventory_cli_is_a_separate_read_only_mode() {
        let base = [
            "verify",
            "--fast-img-cleanup-inventory",
            "/fixture/working-copy",
        ];
        let args = Args::try_parse_from(base).unwrap();
        assert_eq!(
            args.fast_img_cleanup_inventory.as_deref(),
            Some(Path::new("/fixture/working-copy"))
        );
        assert!(
            Args::try_parse_from(base.into_iter().chain(["--verify", "/fixture/input"])).is_err()
        );
        assert!(
            Args::try_parse_from(base.into_iter().chain(["--output", "/fixture/report"])).is_err()
        );
        assert!(
            Args::try_parse_from(
                base.into_iter()
                    .chain(["--fast-img-marker-json", "/fixture/other"])
            )
            .is_err()
        );
    }

    #[test]
    fn fast_img_jpeg_scratch_cli_is_a_separate_read_only_mode() {
        let base = [
            "verify",
            "--fast-img-jpeg-scratch",
            "/fixture/private-search",
        ];
        let args = Args::try_parse_from(base).unwrap();
        assert_eq!(
            args.fast_img_jpeg_scratch.as_deref(),
            Some(Path::new("/fixture/private-search"))
        );
        assert!(
            Args::try_parse_from(base.into_iter().chain(["--verify", "/fixture/input"])).is_err()
        );
        assert!(
            Args::try_parse_from(base.into_iter().chain(["--output", "/fixture/report"])).is_err()
        );
        assert!(
            Args::try_parse_from(
                base.into_iter()
                    .chain(["--fast-img-cleanup-inventory", "/fixture/other"])
            )
            .is_err()
        );
    }

    #[test]
    fn fast_img_jpeg_scratch_list_cli_conflicts_with_other_queries() {
        let base = ["verify", "--fast-img-jpeg-scratch-list"];
        assert!(
            Args::try_parse_from(base)
                .unwrap()
                .fast_img_jpeg_scratch_list
        );
        for conflicting in [
            ["--fast-img-jpeg-scratch", "/fixture/scratch"],
            ["--fast-img-cleanup-inventory", "/fixture/work"],
            ["--output", "/fixture/report"],
        ] {
            assert!(Args::try_parse_from(base.into_iter().chain(conflicting)).is_err());
        }
    }

    // Inventory fixtures model local files; live UUID/resource failures are tested separately.
    fn delivery_inventory_fixture(
        source: &Path,
        output: &Path,
        report: &mut String,
        mode: &str,
        strategy: &str,
    ) -> Result<IntegrityStats> {
        run_fast_img_delivery_check_with(source, output, report, mode, strategy, |_| Ok(()))
    }

    fn isolated_marker_test() -> bool {
        let thread = std::thread::current();
        let name = thread.name().unwrap();
        if std::env::var("MFB_VERIFY_MARKER_TEST").as_deref() == Ok(name) {
            return false;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--nocapture", "--test-threads=1"])
            .env("MFB_VERIFY_MARKER_TEST", name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    fn marker_test_state(root: &Path) -> foundation::common_utils::EnvGuard {
        foundation::common_utils::EnvGuard::set(
            "MFB_HOME_ROOT",
            root.join("state").to_str().unwrap(),
        )
    }

    fn write_marker_fixture(
        path: &Path,
        source: &Path,
        partial: &serde_json::Value,
    ) -> foundation::common_utils::EnvGuard {
        let output = path.parent().unwrap();
        let guard = marker_test_state(output.parent().unwrap());
        let mut value =
            serde_json::to_value(foundation::pipeline::verification::WorkingCopyMarker::new(
                source.into(),
                output.into(),
                0,
            ))
            .unwrap();
        for (name, field) in partial.as_object().unwrap() {
            value[name] = field.clone();
        }
        let marker = serde_json::from_value(value).unwrap();
        foundation::pipeline::verification::write_marker_atomic(&marker).unwrap();
        guard
    }

    #[test]
    fn machine_history_count_status_preserves_existing_verifier_labels() {
        let mut stats = IntegrityStats {
            count_matches_with_handoff: true,
            ..IntegrityStats::default()
        };
        assert_eq!(integrity_count_status(&stats), "MATCH");
        stats.pipeline_handoff = 1;
        stats.count_delta = -1;
        stats.expected_count_delta = -1;
        assert_eq!(
            integrity_count_status(&stats),
            "MATCH (1 expected handoff gap)"
        );
        stats.integrity_failures = 1;
        assert_eq!(
            integrity_count_status(&stats),
            "MATCH (1 expected handoff gap)"
        );
        stats.integrity_failures = 0;
        stats.count_matches_with_handoff = false;
        stats.count_fully_explained = true;
        stats.explained_gaps = 1;
        assert_eq!(
            integrity_count_status(&stats),
            "EXPLAINED (1 fewer; all 1 listed below)"
        );
        stats.count_fully_explained = false;
        stats.count_delta = -3;
        assert_eq!(
            integrity_count_status(&stats),
            "MISMATCH (3 fewer; 2 still unexplained)"
        );
        stats.reconstruction_probe_errors = 1;
        assert_eq!(
            integrity_count_status(&stats),
            "UNCONFIRMED (1 file classifications unresolved)"
        );
    }

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
    fn duplicate_fingerprint_hashes_bytes_after_the_old_sampling_boundary() -> Result<()> {
        let temp = tempfile::NamedTempFile::new()?;
        let mut bytes = vec![17; 65_537];
        fs::write(temp.path(), &bytes)?;
        let (first, error) = file_content_blake3(temp.path());
        assert!(error.is_none());
        bytes[65_536] = 18;
        fs::write(temp.path(), &bytes)?;
        let _uncached = foundation::common_utils::UncachedVerificationScope::new()?;
        let (digest, error) = file_content_blake3(temp.path());
        assert!(error.is_none());
        assert_ne!(first, digest);
        assert_eq!(digest, Some(blake3::hash(&bytes).to_hex().to_string()));
        Ok(())
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
    fn test_load_restore_jpeg_manifest_reports_metadata_attention() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::write(
            tempdir.path().join(".mfb_restore_jpeg_manifest.tsv"),
            "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted\n\
             63616d6572612e4a584c\t63616d6572612e6a7067\thash1\thash2\tfalse\n\
             # MFB_RESTORE_JPEG_ATTENTION\t63616d6572612e4a584c\t584d5020636f6e666c696374\n",
        )
        .unwrap();

        let (records, errors) = load_restore_jpeg_manifest(tempdir.path());
        assert_eq!(records.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("camera.JXL: XMP conflict"));
    }

    #[test]
    fn test_fast_img_restore_check_reports_metadata_attention_without_count_mismatch() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&restored).unwrap();
        let source_path = source.join("camera.JXL");
        let output_path = restored.join("camera.jpg");
        fs::write(&source_path, b"\xff\x0atrue-jxl").unwrap();
        fs::write(&output_path, b"\xff\xd8\xff\xe0true-jpeg").unwrap();
        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            format!(
                "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted\n\
                 63616d6572612e4a584c\t63616d6572612e6a7067\t{}\t{}\tfalse\n\
                 # MFB_RESTORE_JPEG_ATTENTION\t63616d6572612e4a584c\t584d5020636f6e666c696374\n",
                calculate_blake3_hash(&source_path).unwrap(),
                calculate_blake3_hash(&output_path).unwrap(),
            ),
        )
        .unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();
        assert_eq!(stats.integrity_failures, 1, "{report}");
        assert!(stats.has_warnings);
        assert_eq!(stats.missing, 0);
        assert_eq!(stats.extra, 0);
        assert_eq!(integrity_count_status(&stats), "MATCH");
        assert!(report.contains("camera.JXL: XMP conflict"));
        assert!(!report.contains("RESTORE INVARIANTS PASS"));
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
        let verification = records[0].verification.as_ref().unwrap();
        assert_eq!(verification.verified_unix_seconds, 1);
        assert_eq!(verification.mfb_version, "0.11.3");
        assert_eq!(verification.djxl_version, "djxl 0.13.0");
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
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 1);
        assert_eq!(stats.source_remaining_files, 0);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.integrity_failures, 0);
        assert!(!stats.has_warnings);
    }

    #[test]
    #[serial]
    fn fast_img_delivery_verifier_retains_unproved_outputs_and_counts_verified_aae_separately()
    -> Result<()> {
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgStageName, LibraryAssetRecord, WorkingCopyMarker,
            write_marker_atomic,
        };

        if isolated_marker_test() {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let _state = marker_test_state(&root);
        let source = root.join("source");
        let output = root.join("source_optimized");
        fs::create_dir(&source)?;
        fs::create_dir(&output)?;
        let target = output.join("photo.JXL");
        let source_aae = source.join("photo.AAE");
        let output_aae = output.join("photo.AAE");
        fs::write(&target, b"\xff\x0asynthetic")?;
        fs::write(&source_aae, b"synthetic edit record")?;
        fs::write(&output_aae, b"synthetic edit record")?;
        let mut marker = WorkingCopyMarker::new(source.clone(), output.clone(), 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.blake3_log.insert(
            "photo.jpg".into(),
            Blake3Entry {
                src: blake3::hash(b"synthetic source").to_hex().to_string(),
                out: calculate_blake3_hash(&target)?,
                out_rel: Some("photo.JXL".into()),
                library_asset: None,
            },
        );
        write_marker_atomic(&marker)?;

        let verify = |report: &mut String| {
            delivery_inventory_fixture(&source, &output, report, "images_only", "jxl")
        };
        let local = verify(&mut String::new())?;
        assert_eq!(local.verified_local_aae, 1);
        assert_eq!(local.optimized_files, 1);
        assert_eq!(local.extra, 0);
        assert!(!local.has_warnings);

        marker.photos_imported_assets.push(LibraryAssetRecord {
            rel_path: "photo.jpg".into(),
            photos_uuid: Some("synthetic-photos-uuid".into()),
            blake3: calculate_blake3_hash(&target)?,
            ..Default::default()
        });
        write_marker_atomic(&marker)?;
        let mut report = String::new();
        let mut checked_photos = false;
        let photos = run_fast_img_delivery_check_with(
            &source,
            &output,
            &mut report,
            "images_only",
            "jxl",
            |_| {
                checked_photos = true;
                Ok(())
            },
        )?;
        assert!(checked_photos);
        assert!(photos.photos_local_aae_retention);
        assert!(!photos.has_warnings);
        assert_eq!(photos.optimized_files, 1);
        assert_eq!(photos.extra, 0);
        assert!(integrity_count_status(&photos).starts_with("MATCH"));
        assert!(report.contains("Retaining local media and AAE companions"));

        fs::write(&output_aae, b"changed edit record")?;
        fs::write(output.join(".user"), b"private synthetic residual")?;
        fs::write(output.join(".DS_Store"), b"\0\0\0\x01Bud1")?;
        fs::create_dir(output.join("Private.photoslibrary"))?;
        fs::write(output.join("Private.photoslibrary/ignored"), b"private")?;
        let mut expected_extras = 3;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, output.join("linked.JXL"))?;
            expected_extras += 1;
        }
        #[cfg(not(unix))]
        let _ = &mut expected_extras;
        let mut report = String::new();
        let changed = verify(&mut report)?;
        assert_eq!(changed.verified_local_aae, 0);
        assert_eq!(changed.extra, expected_extras);
        assert!(changed.has_warnings);
        assert!(report.contains("unverified AAE companion"));
        assert!(report.contains(".user"));
        assert!(!report.contains("Private.photoslibrary/ignored"));
        assert_eq!(fs::read(&output_aae)?, b"changed edit record");

        fs::rename(&target, output.join("unowned.JXL"))?;
        let mut report = String::new();
        let substituted = verify(&mut report)?;
        assert_eq!(substituted.optimized_files, 0);
        assert_eq!(substituted.missing, 1);
        assert_eq!(substituted.extra, expected_extras + 1);
        assert!(report.contains("Recorded output unavailable: \"photo.JXL\""));
        assert!(report.contains("no matching delivery checkpoint entry"));
        Ok(())
    }

    #[test]
    #[serial]
    fn v14_delivery_requires_the_admitted_aae_even_if_both_copies_match() -> Result<()> {
        if isolated_marker_test() {
            return Ok(());
        }
        use foundation::fast_img_store::{
            FastImgAdmissionRoute, FastImgAdmissionSidecar, FastImgSourceAdmission,
            initialize_with_admission,
        };
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgStageName, WorkingCopyMarker, write_marker_atomic,
        };
        let fixture = tempfile::tempdir()?;
        let root = fixture.path().canonicalize()?;
        let _state = marker_test_state(&root);
        let source = root.join("source");
        let output = root.join("output");
        fs::create_dir(&source)?;
        fs::create_dir(&output)?;
        let source_aae = source.join("photo.AAE");
        let output_aae = output.join("photo.AAE");
        let target = output.join("photo.JXL");
        fs::write(&source_aae, b"synthetic edit")?;
        fs::write(&output_aae, b"synthetic edit")?;
        fs::write(&target, b"\xff\x0asynthetic")?;
        let source_hash = blake3::hash(b"original JPEG").to_hex().to_string();
        let mut marker = WorkingCopyMarker::new(source.clone(), output.clone(), 1);
        initialize_with_admission(
            &mut marker,
            &BTreeMap::from([(
                "photo.jpg".into(),
                FastImgSourceAdmission {
                    source_blake3: source_hash.clone(),
                    route: FastImgAdmissionRoute::Primary,
                    xmp: Some(FastImgAdmissionSidecar::Absent),
                    aae: Some(FastImgAdmissionSidecar::Present {
                        relative_path: "photo.AAE".into(),
                        blake3: calculate_blake3_hash(&source_aae)?,
                    }),
                },
            )]),
        )?;
        marker.stage = FastImgStageName::CleanupComplete;
        marker.blake3_log.insert(
            "photo.jpg".into(),
            Blake3Entry {
                src: source_hash,
                out: calculate_blake3_hash(&target)?,
                out_rel: Some("photo.JXL".into()),
                library_asset: None,
            },
        );
        write_marker_atomic(&marker)?;
        let check = |report: &mut String| {
            delivery_inventory_fixture(&source, &output, report, "images_only", "jxl")
        };
        assert_eq!(check(&mut String::new())?.verified_local_aae, 1);
        fs::write(&source_aae, b"external edit")?;
        fs::write(&output_aae, b"external edit")?;
        let mut report = String::new();
        let changed = check(&mut report)?;
        assert!(changed.has_warnings && changed.verified_local_aae == 0);
        assert!(report.contains("Recorded source AAE lacks a matching current local copy"));
        fs::remove_file(&output_aae)?;
        assert!(check(&mut String::new())?.has_warnings);
        assert_eq!(fs::read(&source_aae)?, b"external edit");
        Ok(())
    }

    #[test]
    fn standard_equal_counts_with_mismatched_paths_do_not_report_match() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let source = fixture.path().join("source");
        let output = fixture.path().join("output");
        fs::create_dir(&source)?;
        fs::create_dir(&output)?;
        fs::write(source.join("one.jpg"), b"\xff\xd8\xff\xe0synthetic")?;
        fs::write(output.join("different.jpg"), b"\xff\xd8\xff\xe0synthetic")?;
        let mut report = String::new();
        let stats = run_integrity_check(
            &source,
            &output,
            &mut report,
            "images_only",
            &[],
            None,
            None,
        )?;
        assert_eq!((stats.source_files, stats.optimized_files), (1, 1));
        assert_eq!(
            (stats.missing, stats.extra, stats.integrity_failures),
            (1, 1, 2)
        );
        assert!(stats.has_warnings);
        assert!(!integrity_count_status(&stats).starts_with("MATCH"));
        assert!(!report.contains("INTEGRITY CHECKS CLEAN"));
        Ok(())
    }

    #[test]
    fn missing_standard_verification_directory_cannot_confirm_empty_delivery() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let output = root.path().join("missing-output");
        fs::create_dir(&source)?;
        let error = run_integrity_check(
            &source,
            &output,
            &mut String::new(),
            "images_only",
            &[],
            None,
            None,
        )
        .err()
        .context("missing output must make verification unavailable")?;
        assert_eq!(
            error.downcast_ref::<IntegrityUnavailableReason>(),
            Some(&IntegrityUnavailableReason::DirectoryPair),
        );
        assert!(!output.exists());
        Ok(())
    }

    #[test]
    #[serial]
    fn delivery_verifier_cancellation_during_custody_cannot_publish_pass() -> Result<()> {
        if isolated_marker_test() {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
        }
        let control = root.path().join("control");
        fs::write(&control, "running\nsynthetic-running")?;
        let _control = foundation::common_utils::EnvGuard::set(
            "MFB_BATCH_CONTROL_FILE",
            control.to_str().context("test control path is not UTF-8")?,
        );
        let source = root.path().join("source");
        let output = root.path().join("output");
        fs::create_dir(&output)?;
        let bytes = b"\xff\x0aencoded";
        fs::write(output.join("a.JXL"), bytes)?;
        let hash = blake3::hash(bytes).to_hex().to_string();
        let partial = serde_json::json!({
            "src_jpeg_count":1, "stage":"cleanup_complete",
            "photos_imported_assets":[{
                "rel_path":"a.JXL","blake3":hash,"sync_status":"photos_local",
                "photos_uuid":"synthetic-uuid","quarantined":false,"library_blake3":null,
                "xmp_sidecar_blake3":null
            }],
            "blake3_log":{"a.jpg":{"src":"source-hash","out":hash,
                "out_rel":"a.JXL","library_asset":hash}}
        });
        let _state = write_marker_fixture(&output.join("fixture"), &source, &partial);
        let mut report = String::new();
        let error = run_fast_img_delivery_check_with(
            &source,
            &output,
            &mut report,
            "images_only",
            "jxl",
            |_| {
                let next = control.with_extension("next");
                fs::write(&next, "cancelled\nsynthetic-stop")?;
                fs::rename(next, &control)?;
                Ok(())
            },
        )
        .err()
        .context("interrupted verifier must not confirm delivery")?;
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::Interrupted),
        );
        assert!(!report.contains("DELIVERY INVARIANTS PASS"));
        assert_eq!(fs::read(output.join("a.JXL"))?, bytes);
        Ok(())
    }

    #[test]
    #[serial]
    fn delivery_check_refuses_stale_photos_receipts_before_claiming_current_integrity() {
        if isolated_marker_test() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let output = root.path().join("output");
        fs::create_dir(&output).unwrap();
        let bytes = b"\xff\x0aencoded";
        fs::write(output.join("a.JXL"), bytes).unwrap();
        let hash = blake3::hash(bytes).to_hex().to_string();
        let partial = serde_json::json!({"working_copy":output,"src_jpeg_count":1,
            "stage":"cleanup_complete", "photos_imported_assets":[{
                "rel_path":"a.JXL","blake3":hash,"sync_status":"photos_local",
                "photos_uuid":"old-uuid","quarantined":false,"library_blake3":null,
                "xmp_sidecar_blake3":null}],
            "blake3_log":{"a.jpg":{"src":"source-hash","out":hash,
                "out_rel":"a.JXL","library_asset":hash}}});
        let _state = write_marker_fixture(&output.join("fixture"), &source, &partial);
        let mut report = String::new();
        let mut calls = 0;
        let result = run_fast_img_delivery_check_with(
            &source,
            &output,
            &mut report,
            "images_only",
            "jxl",
            |_| {
                assert!(foundation::common_utils::verification_cache_disabled());
                calls += 1;
                anyhow::bail!("current Photos original no longer matches the recorded UUID")
            },
        );
        let error = result.err().expect("stale Photos custody must fail");
        assert_eq!(
            error.downcast_ref::<IntegrityUnavailableReason>(),
            Some(&IntegrityUnavailableReason::PhotosCustody)
        );
        assert_eq!(calls, 1);
        assert!(!report.contains("DELIVERY INVARIANTS PASS"));
        assert!(output.join("a.JXL").exists());
        assert!(
            run_fast_img_delivery_check(
                &source,
                &output,
                &mut String::new(),
                "images_only",
                "jxl",
                None
            )
            .is_err(),
            "historical receipts without an actual library were accepted"
        );
    }

    #[test]
    #[serial]
    fn photos_cleanup_recovery_requires_fresh_custody_for_absent_local_copies() -> Result<()> {
        if isolated_marker_test() {
            return Ok(());
        }
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgStageName, Gate1Checks, Gate2Checks, Gate3Checks,
            WorkingCopyMarker, write_marker_atomic,
        };
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        let _state = marker_test_state(&root);
        let source = root.join("source");
        let output = root.join("output");
        let existing = output.join("b.JXL");
        fs::create_dir(&output)?;
        fs::write(&existing, b"\xff\x0averified synthetic JXL")?;
        let hash = calculate_blake3_hash(&existing)?;
        let mut marker = WorkingCopyMarker::new(source.clone(), output.clone(), 2);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.gate1_checks = Gate1Checks {
            count: true.into(),
            blake3: true.into(),
            metadata: true.into(),
            size: true.into(),
            orient: true.into(),
            decode: true.into(),
        };
        marker.gate2_checks = Gate2Checks {
            count: true.into(),
            blake3_sample: true.into(),
            no_error: true.into(),
        };
        marker.gate3_checks = Gate3Checks {
            count_x3: true.into(),
            sync: true.into(),
            quarantine: true.into(),
            chain: true.into(),
        };
        marker.photos_library_path = Some(root.join("synthetic.photoslibrary"));
        for (index, stem) in ["a", "b"].into_iter().enumerate() {
            let relative = format!("{stem}.JXL");
            marker.blake3_log.insert(
                format!("{stem}.jpg"),
                Blake3Entry {
                    src: blake3::hash(stem.as_bytes()).to_hex().to_string(),
                    out: hash.clone(),
                    out_rel: Some(relative.clone()),
                    library_asset: Some(hash.clone()),
                },
            );
            marker
                .photos_imported_assets
                .push(serde_json::from_value(serde_json::json!({
                    "rel_path": relative, "blake3": hash, "sync_status": "photos_local",
                    "photos_uuid": format!("synthetic-{index}"), "quarantined": false,
                    "library_blake3": hash,
                }))?);
        }
        write_marker_atomic(&marker)?;
        let mut report = String::new();
        let standard = run_fast_img_delivery_check_with(
            &source,
            &output,
            &mut report,
            "images_only",
            "jxl",
            |_| unreachable!(),
        )?;
        assert!(standard.has_warnings && standard.missing == 1);
        let digest = dev::infra::drag_drop::fast_img_cleanup_checkpoint_digest(&marker)?;
        let check = |report: &mut String, good_custody| {
            run_fast_img_delivery_check_for_scope(
                &source,
                &output,
                report,
                "images_only",
                "jxl",
                Some(&digest),
                |verified| {
                    assert!(foundation::common_utils::verification_cache_disabled());
                    assert_eq!(verified.photos_imported_assets.len(), 2);
                    anyhow::ensure!(good_custody, "synthetic Photos original missing");
                    Ok(())
                },
            )
        };
        report.clear();
        assert!(check(&mut report, false).is_err());
        assert!(!report.contains("DELIVERY INVARIANTS PASS"));
        report.clear();
        let recovery = check(&mut report, true)?;
        assert!(!recovery.has_warnings);
        assert_eq!(
            (
                recovery.optimized_files,
                recovery.missing,
                recovery.verified_deleted_sources
            ),
            (1, 0, 0)
        );
        assert!(report.contains("Recorded local outputs absent now: 1"));
        assert!(!report.contains("were successfully deleted"));

        fs::write(output.join("a.JXL"), b"unknown replacement")?;
        report.clear();
        let wrong_format = check(&mut report, true)?;
        assert!(wrong_format.has_warnings && wrong_format.missing == 1);
        fs::remove_file(output.join("a.JXL"))?;
        fs::create_dir(&source)?;
        assert!(check(&mut String::new(), true).is_err());
        assert_eq!(fs::read(existing)?, b"\xff\x0averified synthetic JXL");
        Ok(())
    }

    #[test]
    #[serial]
    fn test_fast_img_meme_delivery_rejects_recorded_static_source_left_behind() {
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&optimized.join("fixture"), &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "avif")
                .unwrap();

        assert_eq!(stats.source_remaining_files, 1);
        assert!(stats.integrity_failures > 0);
        assert!(report.contains("unexpected source static image"));
        assert!(report.contains("Optimized AVIF files:"));
    }

    #[test]
    #[serial]
    fn test_fast_img_marker_lookup_reads_state_root_marker() {
        if isolated_marker_test() {
            return;
        }
        let tempdir = tempfile::tempdir().unwrap();
        let state_root = tempdir.path().join("mfb_state");
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        let _state =
            foundation::common_utils::EnvGuard::set("MFB_HOME_ROOT", state_root.to_str().unwrap());
        let mut marker = foundation::pipeline::verification::WorkingCopyMarker::new(
            source,
            optimized.clone(),
            1,
        );
        marker.stage = foundation::pipeline::verification::FastImgStageName::Gate1Failed;
        foundation::pipeline::verification::write_marker_atomic(&marker).unwrap();

        let (marker, found_path, marker_error) = load_fast_img_marker_for_optimized(&optimized);

        assert_eq!(
            found_path,
            Some(foundation::fast_img_store::database_path().unwrap())
        );
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
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
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
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
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
        if isolated_marker_test() {
            return;
        }
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(source.join("djxl_failed.jpg"), b"\xff\xd8\xff\xe0jpeg").unwrap();
        fs::write(source.join("retained.JXL"), b"\xff\x0aretained").unwrap();
        fs::write(optimized.join("converted.JXL"), b"\xff\x0aencoded").unwrap();

        let marker_path = optimized.join("fastmode_img_marker.json");
        let marker_data = serde_json::json!({
            "working_copy": optimized.to_string_lossy().to_string(),
            "src_jpeg_count": 3,
            "skipped_sources": {},
            "failed_sources": {
                "djxl_failed.jpg": {
                    "src": "failed-source-blake3",
                    "reason": "pixel-diff: djxl exited non-zero decoding output.JXL"
                },
                "retained.JXL": {
                    "src": "jxl-source-blake3",
                    "reason": "existing JXL exact reconstruction failed"
                }
            }
        });
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 3);
        assert_eq!(stats.optimized_files, 1);
        assert_eq!(stats.source_remaining_files, 2);
        assert_eq!(stats.failed_sources, 2);
        assert_eq!(stats.integrity_failures, 0);
    }

    #[test]
    #[serial]
    fn routed_original_only_child_verifies_retained_failure_and_detects_companion_drift()
    -> Result<()> {
        if isolated_marker_test() {
            return Ok(());
        }
        use foundation::fast_img_store::{
            FailedRetryDeliveryIntent, FailedRetrySourceRoute, capture_failed_retry_admissions,
            create_or_resume_failed_retry, inspect_failed_retry_parent,
        };
        use foundation::pipeline::verification::{
            FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName,
            SkippedSourceEntry, WorkingCopyMarker, write_marker_atomic,
        };
        let temporary = tempfile::tempdir()?;
        let _state = marker_test_state(temporary.path());
        let root = temporary.path().canonicalize()?;
        let source = root.join("source");
        fs::create_dir(&source)?;
        let media = source.join("retained.heic");
        fs::write(&media, b"synthetic original-format source")?;
        let failure = SkippedSourceEntry {
            src: calculate_blake3_hash(&media)?,
            reason: "synthetic pre-submission rejection".into(),
        };
        let mut parent = WorkingCopyMarker::new(source.clone(), root.join("source_optimized"), 1);
        parent.stage = FastImgStageName::CleanupComplete;
        parent.processing_contract = Some(FastImgProcessingContract {
            schema: 1,
            tool_policy: foundation::runtime_config::ToolPolicy::Single,
            fallback_policy: foundation::runtime_config::FallbackPolicy::Strict,
            encoding: FastImgEncodingParameters::Jxl {
                jpeg_effort: 11,
                jpeg_effort_search: foundation::runtime_config::JpegEffortSearch::Full,
                archive: false,
                allow_expert_options: false,
            },
        });
        parent
            .failed_sources
            .insert("retained.heic".into(), failure.clone());
        write_marker_atomic(&parent)?;
        let reference = inspect_failed_retry_parent(&parent.working_copy)?;
        let mut admissions = capture_failed_retry_admissions(&reference)?;
        admissions[0].route = Some(FailedRetrySourceRoute::Original);
        let naming_policy = foundation::fast_img::photos_import_naming_policy_for(
            &foundation::runtime_config::RuntimeConfig::default().photos,
        );
        let mut child = create_or_resume_failed_retry(
            &reference,
            &admissions,
            &FailedRetryDeliveryIntent::Photos {
                library_path: None,
                naming_policy,
            },
            None,
        )?;
        foundation::pipeline::verification::prepare_photos_working_copy_parent(
            &source,
            &child.working_copy,
        )?;
        fs::create_dir(&child.working_copy)?;
        let check = |report: &mut String| {
            delivery_inventory_fixture(&source, &child.working_copy, report, "images_only", "jxl")
        };
        let mut report = String::new();
        let pending = check(&mut report)?;
        assert!(
            pending.has_warnings
                && report.contains("no confirmed delivery or retained failure proof")
        );

        child.stage = FastImgStageName::CleanupComplete;
        child
            .tier2_failed_sources
            .insert("retained.heic".into(), failure);
        write_marker_atomic(&child)?;
        let mut report = String::new();
        let settled = check(&mut report)?;
        assert!(!settled.has_warnings);
        assert_eq!(
            (
                settled.source_files,
                settled.optimized_files,
                settled.failed_sources,
                settled.source_remaining_files
            ),
            (1, 0, 1, 1)
        );
        fs::write(source.join("retained.xmp"), b"unexpected companion")?;
        let mut report = String::new();
        assert!(check(&mut report)?.has_warnings);
        assert!(report.contains("companion presence changed"));
        Ok(())
    }

    #[test]
    #[serial]
    fn test_failed_child_delivery_verifies_only_its_bound_sources_and_all_owned_outputs() {
        if isolated_marker_test() {
            return;
        }
        use foundation::fast_img_store::{
            capture_failed_retry_admissions, create_or_resume_failed_retry,
            inspect_failed_retry_parent,
        };
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName,
            SkippedSourceEntry, WorkingCopyMarker, write_marker_atomic,
        };
        let temporary = tempfile::tempdir().unwrap();
        let _state = marker_test_state(temporary.path());
        let source = temporary.path().join("Album");
        fs::create_dir(&source).unwrap();
        let source = source.canonicalize().unwrap();
        let parent_output = source.parent().unwrap().join("Album_optimized");
        for name in ["retry.jpg", "still-failed.jpg", "parent-skipped.jpg"] {
            fs::write(source.join(name), b"\xff\xd8\xff\xe0synthetic jpeg").unwrap();
        }
        let source_hash = calculate_blake3_hash(&source.join("retry.jpg")).unwrap();
        let mut parent = WorkingCopyMarker::new(source.clone(), parent_output, 3);
        parent.stage = FastImgStageName::CleanupComplete;
        let defaults = foundation::runtime_config::RuntimeConfig::default();
        parent.processing_contract = Some(FastImgProcessingContract {
            schema: 1,
            tool_policy: defaults.tools.policy,
            fallback_policy: defaults.img.fallback_policy,
            encoding: FastImgEncodingParameters::Jxl {
                jpeg_effort: 11,
                jpeg_effort_search: defaults.img.jpeg_effort_search,
                archive: false,
                allow_expert_options: false,
            },
        });
        for name in ["retry.jpg", "still-failed.jpg"] {
            parent.failed_sources.insert(
                name.into(),
                SkippedSourceEntry {
                    src: source_hash.clone(),
                    reason: "synthetic parent failure".into(),
                },
            );
        }
        parent.skipped_sources.insert(
            "parent-skipped.jpg".into(),
            SkippedSourceEntry {
                src: source_hash.clone(),
                reason: "parent policy retained this media".into(),
            },
        );
        write_marker_atomic(&parent).unwrap();
        let parent_ref = inspect_failed_retry_parent(&parent.working_copy).unwrap();
        let admissions = capture_failed_retry_admissions(&parent_ref).unwrap();
        let manifest_path = source.parent().unwrap().join("synthetic-tool-state");
        fs::write(&manifest_path, b"synthetic current tool-state fixture").unwrap();
        let tools = foundation::fast_img_store::FailedRetryToolAuthority {
            identity: calculate_blake3_hash(&manifest_path).unwrap(),
            manifest_path,
        };
        let mut child = create_or_resume_failed_retry(
            &parent_ref,
            &admissions,
            &foundation::fast_img_store::FailedRetryDeliveryIntent::Local,
            Some(&tools),
        )
        .unwrap();
        fs::create_dir(&child.working_copy).unwrap();
        let output = child.working_copy.join("retry.JXL");
        fs::write(&output, b"\xff\x0asynthetic output").unwrap();
        child.blake3_log.insert(
            "retry.jpg".into(),
            Blake3Entry {
                src: source_hash.clone(),
                out: calculate_blake3_hash(&output).unwrap(),
                out_rel: Some("retry.JXL".into()),
                library_asset: None,
            },
        );
        child.failed_sources.insert(
            "still-failed.jpg".into(),
            SkippedSourceEntry {
                src: source_hash,
                reason: "retained failure in this child".into(),
            },
        );
        child.stage = FastImgStageName::CleanupComplete;
        child.encoded_count = 1;
        write_marker_atomic(&child).unwrap();
        fs::remove_file(source.join("retry.jpg")).unwrap();
        fs::write(
            source.join("new-unselected.jpg"),
            b"\xff\xd8\xff\xe0new jpeg",
        )
        .unwrap();
        let check = |source: &Path| {
            delivery_inventory_fixture(
                source,
                &child.working_copy,
                &mut String::new(),
                "images_only",
                "jxl",
            )
        };
        let stats = check(&source).unwrap();
        assert_eq!(
            (
                stats.source_files,
                stats.optimized_files,
                stats.failed_sources
            ),
            (2, 1, 1)
        );
        assert_eq!(stats.source_remaining_files, 1);
        assert_eq!(
            stats.integrity_failures, 0,
            "parent-retained and newly-added media are outside this child"
        );
        assert!(
            source.join("parent-skipped.jpg").is_file()
                && source.join("new-unselected.jpg").is_file()
        );

        fs::write(
            source.join("retry.jpg"),
            b"\xff\xd8\xff\xe0source reappeared",
        )
        .unwrap();
        assert!(
            check(&source).unwrap().integrity_failures > 0,
            "a selected successful source left behind must still fail"
        );
        fs::remove_file(source.join("retry.jpg")).unwrap();
        fs::remove_file(source.join("still-failed.jpg")).unwrap();
        assert!(
            check(&source).unwrap().integrity_failures > 0,
            "a selected failed source must remain present"
        );
        fs::write(
            source.join("still-failed.jpg"),
            b"\xff\xd8\xff\xe0synthetic jpeg",
        )
        .unwrap();
        fs::write(child.working_copy.join("unowned.txt"), b"unowned bytes").unwrap();
        assert!(
            check(&source).unwrap().integrity_failures > 0,
            "unknown child output must not be filtered by the source workset"
        );
        fs::remove_file(child.working_copy.join("unowned.txt")).unwrap();
        let other_source = temporary.path().join("other-source");
        fs::create_dir(&other_source).unwrap();
        assert!(
            check(&other_source).is_err(),
            "child scope cannot be rebound to another source"
        );
        let conn = rusqlite::Connection::open(foundation::fast_img_store::database_path().unwrap())
            .unwrap();
        conn.execute("UPDATE fast_img_retry_sources SET admitted_source_blake3=?1 WHERE source_rel='retry.jpg'",
            ["0".repeat(64)]).unwrap();
        assert!(
            check(&source).is_err(),
            "a changed manifest must not become a source filter"
        );
        assert!(output.is_file() && source.join("parent-skipped.jpg").is_file());
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_rejects_missing_marker_proof() {
        if isolated_marker_test() {
            return;
        }
        let tempdir = tempfile::tempdir().unwrap();
        let _state = marker_test_state(tempdir.path());
        let source = tempdir.path().join("Album");
        let optimized = tempdir.path().join("Album_optimized");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&optimized).unwrap();

        fs::write(optimized.join("photo.JXL"), b"\xff\x0aencoded").unwrap();

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.source_files, 0);
        assert_eq!(stats.integrity_failures, 1);
        assert!(stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_rejects_remaining_true_jpeg_and_non_jxl_output() {
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
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
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.optimized_files, 0);
        assert_eq!(stats.extra, 1);
        assert!(stats.has_warnings);
    }

    #[test]
    #[serial]
    fn test_fast_img_restore_check_rejects_matching_filenames_without_manifest_proof() {
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
        assert_eq!(stats.integrity_failures, 1);
        assert!(stats.has_warnings);
        assert_eq!(integrity_count_status(&stats), "MATCH");
        assert!(report.contains(
            "restored JPEG has no valid reconstruction manifest proof: nested/camera.jpeg"
        ));
        assert!(!report.contains("RESTORE INVARIANTS PASS"));

        fs::write(
            restored.join(".mfb_restore_jpeg_manifest.tsv"),
            format!(
                "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted\n\
                 6e65737465642f63616d6572612e4a584c\t6e65737465642f63616d6572612e6a7067\t{}\t{}\tfalse\n",
                calculate_blake3_hash(&source.join("nested/camera.JXL")).unwrap(),
                calculate_blake3_hash(&restored.join("nested/camera.jpeg")).unwrap(),
            ),
        )
        .unwrap();
        report.clear();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();
        assert_eq!(stats.integrity_failures, 1);
        assert!(stats.has_warnings);
        assert!(report.contains(
            "restored JPEG has no valid reconstruction manifest proof: nested/camera.jpeg"
        ));
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
        assert!(report.contains("\"nested/camera.JXL\": unknown (legacy manifest)"));
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
        assert_eq!(stats.restore_manifest_errors, 2);
        assert!(report.contains(
            "source directory was removed but the restore manifest contains no proof records"
        ));
    }

    #[test]
    fn test_fast_img_restore_check_reports_duplicate_source_and_output_stems() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("Album");
        let restored = tempdir.path().join("Album_restored_jpeg");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&restored).unwrap();
        fs::write(source.join("camera.JXL"), b"\xff\x0atrue-jxl").unwrap();
        fs::write(source.join("camera.jxl-copy"), b"\xff\x0atrue-jxl").unwrap();
        fs::write(restored.join("camera.jpg"), b"\xff\xd8\xff\xe0true-jpeg").unwrap();
        fs::write(restored.join("camera.jpeg"), b"\xff\xd8\xff\xe0true-jpeg").unwrap();

        let mut report = String::new();
        let stats = run_fast_img_restore_check(&source, &restored, &mut report, "jxl").unwrap();
        assert!(stats.has_warnings);
        assert_eq!(stats.source_probe_errors, 1, "{report}");
        assert_eq!(stats.optimized_probe_errors, 1, "{report}");
        assert_eq!(stats.source_files, 2);
        assert_eq!(stats.source_remaining_files, 2);
        assert_eq!(stats.extra, 1);
        assert!(!integrity_count_status(&stats).starts_with("MATCH"));
        assert!(report.contains("ambiguous source JXL stem"));
        assert!(report.contains("ambiguous restored JPEG stem"));
        assert!(!report.contains("RESTORE INVARIANTS PASS"));
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
        let encoded = std::process::Command::new(
            foundation::common_utils::resolve_tool_path(foundation::constants::TOOL_CJXL)
                .expect("available cjxl must have a resolved executable path"),
        )
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
        assert_eq!(
            integrity_count_status(&stats),
            "UNCONFIRMED (2 file classifications unresolved)"
        );
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
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
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
    fn ordinary_journaled_original_delivery_rechecks_absence_of_xmp_after_photos() -> Result<()> {
        use foundation::fast_img::{SourceUnlinkEvent, SourceUnlinkKind, Tier2CleanupObserver};
        use foundation::fast_img_store::Tier2CleanupJournal;
        use foundation::pipeline::verification::{
            FastImgStageName, LibraryAssetRecord, WorkingCopyMarker, write_marker_atomic,
        };
        if isolated_marker_test() {
            return Ok(());
        }
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        let _state = marker_test_state(&root);
        let source = root.join("source");
        let output = root.join("source_optimized");
        fs::create_dir(&source)?;
        fs::create_dir(&output)?;
        let media = source.join("original.webp");
        let xmp = source.join("original.xmp");
        fs::write(&media, b"synthetic original")?;
        fs::write(&xmp, b"<xmp/>")?;
        let hash = calculate_blake3_hash(&media)?;
        let xmp_hash = calculate_blake3_hash(&xmp)?;
        let asset = LibraryAssetRecord {
            rel_path: "original.webp".into(),
            blake3: hash.clone(),
            photos_uuid: Some("synthetic-original-uuid".into()),
            library_blake3: Some(
                blake3::hash(b"synthetic merged original")
                    .to_hex()
                    .to_string(),
            ),
            xmp_sidecar_blake3: Some(xmp_hash.clone()),
            ..Default::default()
        };
        let mut marker = WorkingCopyMarker::new(source.clone(), output.clone(), 0);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.tier2_in_progress = true;
        marker.tier2_cleanup_journal = true;
        marker.tier2_imported_assets.push(asset.clone());
        marker.photos_library_path = Some(root.join("Synthetic.photoslibrary"));
        write_marker_atomic(&marker)?;
        {
            use SourceUnlinkEvent::{Intent, Ready, Removed};
            use SourceUnlinkKind::{Media, Xmp};
            let mut journal = Tier2CleanupJournal::open(&marker)?;
            journal.observe(&asset, Intent, Xmp, &xmp, &xmp_hash)?;
            journal.observe(&asset, Intent, Media, &media, &hash)?;
            journal.observe(&asset, Ready, Media, &media, &hash)?;
            fs::remove_file(&media)?;
            journal.observe(&asset, Removed, Media, &media, &hash)?;
            fs::remove_file(&xmp)?;
            journal.observe(&asset, Removed, Xmp, &xmp, &xmp_hash)?;
        }
        marker.tier2_in_progress = false;
        write_marker_atomic(&marker)?;
        let clean =
            delivery_inventory_fixture(&source, &output, &mut String::new(), "images_only", "jxl")?;
        assert!(!clean.has_warnings);

        let mut report = String::new();
        let error = run_fast_img_delivery_check_with(
            &source,
            &output,
            &mut report,
            "images_only",
            "jxl",
            |_| {
                fs::write(&xmp, b"<xmp/>")?;
                Ok(())
            },
        )
        .err()
        .expect("restored XMP must fail final delivery verification");
        assert!(format!("{error:#}").contains("XMP changed or reappeared"));
        assert!(!report.contains("FAST-IMG DELIVERY INVARIANTS PASS"));
        assert_eq!(fs::read(&xmp)?, b"<xmp/>");
        assert!(output.is_dir());
        Ok(())
    }

    #[test]
    #[serial]
    fn test_fast_img_delivery_check_accepts_tier2_modern_lossy_assets() {
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
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
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
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
        if isolated_marker_test() {
            return;
        }
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
        let _state = write_marker_fixture(&marker_path, &source, &marker_data);

        let mut report = String::new();
        let stats =
            delivery_inventory_fixture(&source, &optimized, &mut report, "images_only", "jxl")
                .unwrap();

        assert_eq!(stats.tier2_recorded, 1);
        assert_eq!(stats.tier2_verified_deleted, 0);
        assert!(stats.integrity_failures >= 1);
        assert!(stats.has_warnings);
        assert!(report.contains("remained under source (not deleted)"));
    }
}
