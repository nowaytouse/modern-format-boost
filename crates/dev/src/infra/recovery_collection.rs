//! Collect only originals proven necessary by the JXL recovery audit.
//!
//! Folder backups are matched by the audited relative directory and basename;
//! a single audited JXL may use either one exact backup file or a backup folder.
//! Photos backups require an exact original filename plus a unique UUID or
//! album-hierarchy identity and are exported through `osxphotos`. Neither path
//! guesses from capture time when identity is ambiguous.

use anyhow::{Context, Result};
use foundation::image::format_detect::{FormatKind, detect_true_format};
use foundation::image::jxl_utils::JpegReconstructionEligibility;
use foundation::image::photos_jxl_audit::{
    PhotosBackupOriginalRecord, PhotosJxlRecoveryRecord, list_photos_jxl_recovery_records,
    photos_backup_original_candidates,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECOVERY_MANIFEST: &str = ".mfb_recovery_collection.json";
const OSXPHOTOS_REPORT: &str = ".mfb_recovery_osxphotos_report.json";
const COMPARISON_REPORT: &str = "mfb_backup_comparison.json";

#[derive(Debug, Default)]
pub struct RecoveryCollectionSummary {
    pub selected: usize,
    pub copied: usize,
    pub skipped: usize,
    pub needs_review: usize,
    pub failed: Vec<String>,
    pub manifest: Option<PathBuf>,
}

impl RecoveryCollectionSummary {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.failed.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct RecoveryComparisonSummary {
    pub matched: usize,
    pub source_only: usize,
    pub backup_only: usize,
    pub different: usize,
    pub needs_review: usize,
    pub report: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct RecoveryComparisonReport {
    schema: &'static str,
    complete: bool,
    source_kind: &'static str,
    source_identity: String,
    backup_identity: String,
    generated_unix_seconds: u64,
    matched: Vec<serde_json::Value>,
    source_only: Vec<serde_json::Value>,
    backup_only: Vec<serde_json::Value>,
    different: Vec<serde_json::Value>,
    needs_review: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PhotosComparisonReport {
    library_a: String,
    library_b: String,
    in_a_not_b: Vec<serde_json::Value>,
    in_b_not_a: Vec<serde_json::Value>,
    in_a_and_b_same: Vec<serde_json::Value>,
    in_a_and_b_different: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct RecoveryManifest {
    schema: &'static str,
    complete: bool,
    source_kind: &'static str,
    source_identity: String,
    backup_identity: String,
    generated_unix_seconds: u64,
    records: Vec<RecoveryManifestRecord>,
    failures: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RecoveryManifestIdentity {
    schema: String,
    source_identity: String,
    backup_identity: String,
}

#[derive(Debug, Clone, Serialize)]
struct RecoveryManifestRecord {
    identity: String,
    source_jxl_blake3: String,
    output_relative_path: String,
    output_blake3: String,
    sidecar: bool,
}

#[derive(Debug, Deserialize)]
struct PhotosExportReportRow {
    uuid: String,
    filename: String,
    #[serde(default)]
    exported: bool,
    #[serde(default)]
    skipped: bool,
    #[serde(default)]
    new: bool,
    #[serde(default)]
    updated: bool,
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    sidecar_xmp: bool,
}

#[derive(Debug, Clone)]
struct PhotosRecoveryMatch {
    source: PhotosJxlRecoveryRecord,
    backup: PhotosBackupOriginalRecord,
    backup_blake3: String,
}

fn is_photos_library(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".photoslibrary") || lower.ends_with(".photolibrary")
        })
}

const fn is_jpeg_original(format: FormatKind) -> bool {
    matches!(format, FormatKind::Jpeg)
}

fn checked_real_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} directory {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "{label} must be a real directory, not a file or symlink: {}",
        path.display()
    );
    fs::canonicalize(path).with_context(|| format!("resolve {label} directory {}", path.display()))
}

fn checked_real_input(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    anyhow::ensure!(
        (metadata.is_file() || metadata.is_dir()) && !metadata.file_type().is_symlink(),
        "{label} must be a real file or directory, not a symlink: {}",
        path.display()
    );
    fs::canonicalize(path).with_context(|| format!("resolve {label} {}", path.display()))
}

fn checked_destination(path: &Path, source: &Path, backup: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = checked_real_directory(parent, "recovery destination parent")?;
    let name = path
        .file_name()
        .context("recovery destination has no final component")?;
    anyhow::ensure!(
        Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "recovery destination has an unsafe final component"
    );
    let resolved = parent.join(name);
    if resolved.exists() {
        let metadata = fs::symlink_metadata(&resolved)?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "recovery destination must be a real directory: {}",
            resolved.display()
        );
    }
    anyhow::ensure!(
        !resolved.starts_with(source) && !resolved.starts_with(backup),
        "recovery destination must not be inside the audited source or backup"
    );
    validate_destination_state(&resolved, source, backup)?;
    Ok(resolved)
}

fn path_identity(path: &Path) -> Result<String> {
    foundation::process_lock::hash_path_to_hex(path)
}

fn reject_symlinks(root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry =
            entry.with_context(|| format!("inspect recovery destination {}", root.display()))?;
        anyhow::ensure!(
            !entry.path_is_symlink(),
            "recovery destination contains a symlink and is unsafe to resume: {}",
            entry.path().display()
        );
    }
    Ok(())
}

fn validate_destination_state(destination: &Path, source: &Path, backup: &Path) -> Result<()> {
    if !destination.exists() {
        return Ok(());
    }
    reject_symlinks(destination)?;
    if fs::read_dir(destination)?.next().is_none() {
        return Ok(());
    }
    let manifest_path = destination.join(RECOVERY_MANIFEST);
    let metadata = fs::symlink_metadata(&manifest_path).with_context(|| {
        format!(
            "non-empty recovery destination has no MFB proof manifest: {}",
            destination.display()
        )
    })?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "recovery proof manifest is not a regular file: {}",
        manifest_path.display()
    );
    let existing: RecoveryManifestIdentity = serde_json::from_slice(&fs::read(&manifest_path)?)
        .context("parse existing recovery proof manifest")?;
    anyhow::ensure!(
        existing.schema == "MFB_RECOVERY_COLLECTION_V1"
            && existing.source_identity == path_identity(source)?
            && existing.backup_identity == path_identity(backup)?,
        "recovery destination belongs to a different source or backup"
    );
    Ok(())
}

fn relative_string(path: &Path, root: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "{} is outside recovery root {}",
            path.display(),
            root.display()
        )
    })?;
    anyhow::ensure!(
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "unsafe recovery relative path {}",
        relative.display()
    );
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn file_hash(path: &Path) -> Result<String> {
    foundation::common_utils::calculate_blake3_hash(path)
        .with_context(|| format!("hash recovery file {}", path.display()))
}

/// Prove that a candidate backup JPEG carries the same decoded pixels as the
/// live audited JXL before any recovery copy or Photos export is attempted.
/// Filename, UUID, and album identity narrow the search; they are not payload
/// proof and must never authorize recovery on their own.
fn verify_backup_original_matches_jxl(
    source_jxl: &Path,
    backup_jpeg: &Path,
) -> Result<(String, String)> {
    anyhow::ensure!(
        detect_true_format(source_jxl)? == FormatKind::Jxl,
        "audited recovery source is not a true JXL: {}",
        source_jxl.display()
    );
    anyhow::ensure!(
        is_jpeg_original(detect_true_format(backup_jpeg)?),
        "backup recovery candidate is not a true JPEG: {}",
        backup_jpeg.display()
    );
    let source_before = file_hash(source_jxl)?;
    let backup_before = file_hash(backup_jpeg)?;
    let integrity = foundation::image::fast_img::verify_pixel_equivalence_integrity(
        backup_jpeg,
        source_jxl,
        FormatKind::Jxl,
    )
    .with_context(|| {
        format!(
            "pixel proof failed for audited JXL {} and backup JPEG {}",
            source_jxl.display(),
            backup_jpeg.display()
        )
    })?;
    let (verified_backup, verified_jxl) = match integrity {
        foundation::image::fast_img::IntegrityResult::JxlPixelEquivalent {
            source_hash,
            output_hash,
        } => (source_hash, output_hash),
        _ => anyhow::bail!(
            "recovery candidate proof did not produce the required JXL pixel-equivalence result"
        ),
    };
    anyhow::ensure!(
        verified_backup == backup_before && verified_jxl == source_before,
        "recovery input changed while pixel proof was running: JXL={} backup={}",
        source_jxl.display(),
        backup_jpeg.display()
    );
    let source_after = file_hash(source_jxl)?;
    let backup_after = file_hash(backup_jpeg)?;
    anyhow::ensure!(
        source_after == source_before && backup_after == backup_before,
        "recovery input changed after pixel proof: JXL={} backup={}",
        source_jxl.display(),
        backup_jpeg.display()
    );
    Ok((source_before, backup_before))
}

fn ensure_safe_output_parent(root: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("recovery output has no parent directory")?;
    let relative = parent.strip_prefix(root).with_context(|| {
        format!(
            "recovery output {} is outside destination {}",
            destination.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("unsafe recovery output path {}", destination.display());
        };
        current.push(name);
        if current.exists() {
            let metadata = fs::symlink_metadata(&current)?;
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "recovery output parent is not a real directory: {}",
                current.display()
            );
        } else {
            fs::create_dir(&current)
                .with_context(|| format!("create recovery directory {}", current.display()))?;
        }
    }
    anyhow::ensure!(
        fs::canonicalize(parent)?.starts_with(fs::canonicalize(root)?),
        "recovery output parent escaped destination"
    );
    Ok(())
}

fn copy_exact(root: &Path, source: &Path, destination: &Path) -> Result<(String, bool)> {
    let source_metadata = fs::symlink_metadata(source)
        .with_context(|| format!("inspect recovery source {}", source.display()))?;
    anyhow::ensure!(
        source_metadata.is_file()
            && !source_metadata.file_type().is_symlink()
            && source_metadata.len() > 0,
        "recovery source is not a non-empty regular file: {}",
        source.display()
    );
    let source_hash = file_hash(source)?;
    ensure_safe_output_parent(root, destination)?;
    if destination.exists() {
        anyhow::ensure!(
            fs::symlink_metadata(destination)?.is_file() && file_hash(destination)? == source_hash,
            "recovery destination collision differs from backup: {}",
            destination.display()
        );
        return Ok((source_hash, false));
    }
    let parent = destination
        .parent()
        .context("recovery output has no parent directory")?;
    let suffix = destination
        .extension()
        .and_then(|value| value.to_str())
        .map_or_else(|| ".tmp".to_string(), |value| format!(".{value}"));
    let mut staged = foundation::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "recovery_collection",
        parent,
        "mfb-recovery-",
        &suffix,
    )?;
    fs::copy(source, staged.path()).with_context(|| {
        format!(
            "copy recovery source {} to staging for {}",
            source.display(),
            destination.display()
        )
    })?;
    staged.as_file_mut().flush()?;
    staged.as_file().sync_all()?;
    let staged_hash = file_hash(staged.path())?;
    let source_after_hash = file_hash(source)?;
    anyhow::ensure!(
        source_hash == source_after_hash && source_hash == staged_hash,
        "backup changed during recovery copy: {}",
        source.display()
    );
    staged.persist_noclobber(destination).map_err(|error| {
        anyhow::anyhow!(
            "commit recovery output {}: {}",
            destination.display(),
            error.error
        )
    })?;
    foundation::io_utils::sync_committed_file_and_parent(destination)?;
    Ok((source_hash, true))
}

fn xmp_sidecars(media: &Path) -> Result<Vec<PathBuf>> {
    let parent = media.parent().context("backup media has no parent")?;
    let file_name = media
        .file_name()
        .and_then(|value| value.to_str())
        .context("backup media filename is not UTF-8")?;
    let stem = media
        .file_stem()
        .and_then(|value| value.to_str())
        .context("backup media stem is not UTF-8")?;
    let wanted = [format!("{file_name}.xmp"), format!("{stem}.xmp")];
    let mut sidecars = Vec::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if wanted
            .iter()
            .any(|wanted| name.eq_ignore_ascii_case(wanted))
        {
            let metadata = entry.metadata()?;
            anyhow::ensure!(
                metadata.is_file() && !entry.file_type()?.is_symlink(),
                "XMP sidecar is not a regular file: {}",
                entry.path().display()
            );
            foundation::metadata::validate_xmp_sidecar(&entry.path()).with_context(|| {
                format!("validate recovery XMP sidecar {}", entry.path().display())
            })?;
            sidecars.push(entry.path());
        }
    }
    sidecars.sort();
    sidecars.dedup();
    Ok(sidecars)
}

fn find_folder_backup_original(backup: &Path, relative_jxl: &Path) -> Result<PathBuf> {
    let source_stem = relative_jxl
        .file_stem()
        .and_then(|value| value.to_str())
        .context("audited JXL filename is not UTF-8")?;
    if backup.is_file() {
        let backup_stem = backup
            .file_stem()
            .and_then(|value| value.to_str())
            .context("backup filename is not UTF-8")?;
        anyhow::ensure!(
            backup_stem.eq_ignore_ascii_case(source_stem)
                && is_jpeg_original(detect_true_format(backup)?),
            "backup file is not the original JPEG for {}: {}",
            relative_jxl.display(),
            backup.display()
        );
        return Ok(backup.to_path_buf());
    }

    let relative_parent = relative_jxl.parent().unwrap_or_else(|| Path::new(""));
    let directory = backup.join(relative_parent);
    let mut matches = Vec::new();
    if directory.exists() {
        let canonical_backup = fs::canonicalize(backup)?;
        let metadata = fs::symlink_metadata(&directory)?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "backup relative directory is not a real directory: {}",
            directory.display()
        );
        anyhow::ensure!(
            fs::canonicalize(&directory)?.starts_with(&canonical_backup),
            "backup relative directory escaped the selected backup: {}",
            directory.display()
        );
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if !metadata.is_file() || entry.file_type()?.is_symlink() {
                continue;
            }
            let path = entry.path();
            let same_stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(source_stem));
            if same_stem && is_jpeg_original(detect_true_format(&path)?) {
                matches.push(path);
            }
        }
    }
    anyhow::ensure!(
        matches.len() == 1,
        "backup match for {} resolved to {} originals in {}; exact relative directory and basename are required",
        relative_jxl.display(),
        matches.len(),
        directory.display()
    );
    Ok(matches.remove(0))
}

fn write_manifest(
    destination: &Path,
    source: &Path,
    backup: &Path,
    source_kind: &'static str,
    complete: bool,
    records: Vec<RecoveryManifestRecord>,
    failures: Vec<String>,
) -> Result<PathBuf> {
    fs::create_dir_all(destination)?;
    let path = destination.join(RECOVERY_MANIFEST);
    let manifest = RecoveryManifest {
        schema: "MFB_RECOVERY_COLLECTION_V1",
        complete,
        source_kind,
        source_identity: path_identity(source)?,
        backup_identity: path_identity(backup)?,
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock precedes Unix epoch")?
            .as_secs(),
        records,
        failures,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    let mut staged = foundation::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "recovery_manifest",
        destination,
        "mfb-recovery-manifest-",
        ".json",
    )?;
    staged.as_file_mut().write_all(&bytes)?;
    staged.as_file_mut().write_all(b"\n")?;
    staged.as_file_mut().flush()?;
    staged.as_file().sync_all()?;
    staged.persist(&path).map_err(|error| error.error)?;
    foundation::io_utils::sync_committed_file_and_parent(&path)?;
    Ok(path)
}

fn collect_folder_recovery(
    source: &Path,
    backup: &Path,
    destination: &Path,
    dry_run: bool,
) -> Result<RecoveryCollectionSummary> {
    let source_is_file = source.is_file();
    let source_root = if source_is_file {
        source
            .parent()
            .context("single audited JXL has no parent directory")?
    } else {
        source
    };
    let source_kind = if source_is_file { "file" } else { "folder" };
    let mut candidates = Vec::new();
    if source_is_file {
        candidates.push(source.to_path_buf());
    } else {
        for entry in walkdir::WalkDir::new(source).follow_links(false) {
            let entry =
                entry.with_context(|| format!("scan audited folder {}", source.display()))?;
            if !entry.path_is_symlink() && entry.file_type().is_file() {
                candidates.push(entry.into_path());
            }
        }
    }

    let mut selected = Vec::new();
    let mut needs_review = 0_usize;
    let mut failures = Vec::new();
    for path in candidates {
        if detect_true_format(&path)? != FormatKind::Jxl {
            anyhow::ensure!(
                !source_is_file,
                "single audited source is not a JXL file: {}",
                path.display()
            );
            continue;
        }
        match foundation::image::jxl_utils::probe_jpeg_reconstruction_eligibility(&path) {
            Ok(JpegReconstructionEligibility::Exact) => {}
            Ok(
                JpegReconstructionEligibility::PixelOnly
                | JpegReconstructionEligibility::AdvertisedButRejected { .. },
            ) => selected.push(path),
            Err(error) => {
                needs_review += 1;
                failures.push(format!("{}: {error}", path.display()));
            }
        }
    }
    selected.sort();
    let mut summary = RecoveryCollectionSummary {
        selected: selected.len(),
        needs_review,
        failed: failures,
        ..RecoveryCollectionSummary::default()
    };
    let mut records = Vec::new();
    if !dry_run {
        summary.manifest = Some(write_manifest(
            destination,
            source,
            backup,
            source_kind,
            false,
            Vec::new(),
            summary.failed.clone(),
        )?);
    }
    for source_jxl in selected {
        let relative = source_jxl.strip_prefix(source_root)?;
        let original = match find_folder_backup_original(backup, relative) {
            Ok(path) => path,
            Err(error) => {
                summary.failed.push(error.to_string());
                continue;
            }
        };
        let (source_hash, backup_hash) =
            match verify_backup_original_matches_jxl(&source_jxl, &original) {
                Ok(hashes) => hashes,
                Err(error) => {
                    summary.failed.push(error.to_string());
                    continue;
                }
            };
        let output = destination
            .join(relative.parent().unwrap_or_else(|| Path::new("")))
            .join(
                original
                    .file_name()
                    .context("backup original has no filename")?,
            );
        if dry_run {
            println!(
                "[DRY-RUN] recover {} <- {} (pixel proof ✅)",
                relative.display(),
                original.display()
            );
            continue;
        }
        let mut candidate_records = Vec::new();
        match copy_exact(destination, &original, &output) {
            Ok((hash, copied)) => {
                if hash != backup_hash {
                    summary.failed.push(format!(
                        "backup JPEG changed after pixel proof: {}",
                        original.display()
                    ));
                    continue;
                }
                summary.copied += usize::from(copied);
                summary.skipped += usize::from(!copied);
                candidate_records.push(RecoveryManifestRecord {
                    identity: relative_string(&source_jxl, source_root)?,
                    source_jxl_blake3: source_hash.clone(),
                    output_relative_path: relative_string(&output, destination)?,
                    output_blake3: hash,
                    sidecar: false,
                });
            }
            Err(error) => {
                summary.failed.push(error.to_string());
                continue;
            }
        }
        let sidecars = match xmp_sidecars(&original) {
            Ok(sidecars) => sidecars,
            Err(error) => {
                summary.failed.push(error.to_string());
                Vec::new()
            }
        };
        for sidecar in sidecars {
            let sidecar_output = output
                .parent()
                .context("recovery output has no parent")?
                .join(sidecar.file_name().context("XMP sidecar has no filename")?);
            match copy_exact(destination, &sidecar, &sidecar_output) {
                Ok((hash, copied)) => {
                    summary.copied += usize::from(copied);
                    summary.skipped += usize::from(!copied);
                    candidate_records.push(RecoveryManifestRecord {
                        identity: relative_string(&source_jxl, source_root)?,
                        source_jxl_blake3: source_hash.clone(),
                        output_relative_path: relative_string(&sidecar_output, destination)?,
                        output_blake3: hash,
                        sidecar: true,
                    });
                }
                Err(error) => summary.failed.push(error.to_string()),
            }
        }
        if file_hash(&source_jxl)? != source_hash {
            summary.failed.push(format!(
                "audited JXL changed during recovery collection: {}",
                source_jxl.display()
            ));
            continue;
        }
        records.extend(candidate_records);
    }
    if !dry_run {
        summary.manifest = Some(write_manifest(
            destination,
            source,
            backup,
            source_kind,
            summary.failed.is_empty(),
            records,
            summary.failed.clone(),
        )?);
    }
    Ok(summary)
}

fn folded_filename_stem(name: &str) -> Result<String> {
    let path = Path::new(name);
    anyhow::ensure!(
        path.file_name().is_some_and(|component| component == name),
        "recovery filename is unsafe: {name}"
    );
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .context("recovery filename has no UTF-8 stem")?;
    anyhow::ensure!(!stem.is_empty(), "recovery filename has an empty stem");
    Ok(stem.to_lowercase())
}

fn photos_album_paths_overlap(left: &[Vec<String>], right: &[Vec<String>]) -> bool {
    left.iter().any(|path| right.contains(path))
}

fn select_photos_backup_match<'a>(
    source: &PhotosJxlRecoveryRecord,
    candidates: &'a [PhotosBackupOriginalRecord],
) -> Result<&'a PhotosBackupOriginalRecord> {
    let source_stem = folded_filename_stem(&source.original_filename)?;
    let mut same_stem = Vec::new();
    for candidate in candidates {
        if folded_filename_stem(&candidate.original_filename)? == source_stem {
            same_stem.push(candidate);
        }
    }
    let same_uuid = same_stem
        .iter()
        .copied()
        .filter(|candidate| candidate.uuid == source.uuid)
        .collect::<Vec<_>>();
    let album_matches = same_stem
        .iter()
        .copied()
        .filter(|candidate| photos_album_paths_overlap(&source.album_paths, &candidate.album_paths))
        .collect::<Vec<_>>();
    if same_uuid.len() == 1 {
        return Ok(same_uuid[0]);
    }
    if album_matches.len() == 1 {
        return Ok(album_matches[0]);
    }
    if same_stem.len() == 1 {
        return Ok(same_stem[0]);
    }
    let evidence = same_stem
        .iter()
        .map(|candidate| {
            format!(
                "{}@{}",
                candidate.uuid,
                candidate.capture_date.as_deref().unwrap_or("unknown-date")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::bail!(
        "backup Photos match for source UUID {} ({}) resolved to {} JPEG candidates [{}]; exact filename plus UUID or album identity must select one",
        source.uuid,
        source.original_filename,
        same_stem.len(),
        evidence
    )
}

fn resolve_photos_recovery_matches(
    backup: &Path,
    recovery: &[PhotosJxlRecoveryRecord],
) -> Result<Vec<PhotosRecoveryMatch>> {
    let requested_names = recovery
        .iter()
        .map(|record| record.original_filename.clone())
        .collect::<Vec<_>>();
    let candidates = photos_backup_original_candidates(backup, &requested_names)?;
    validate_photos_originals(&candidates)?;
    let mut used_backup_uuids = BTreeSet::new();
    let mut matches = Vec::with_capacity(recovery.len());

    for source in recovery {
        let selected = select_photos_backup_match(source, &candidates)?;
        let (source_hash, backup_blake3) =
            verify_backup_original_matches_jxl(&source.source_path, &selected.original_path)?;
        anyhow::ensure!(
            source_hash == source.source_blake3,
            "live Photos JXL {} changed since the audit; rerun the audit before backup recovery",
            source.uuid
        );
        anyhow::ensure!(
            used_backup_uuids.insert(selected.uuid.clone()),
            "backup Photos UUID {} matched more than one audited JXL",
            selected.uuid
        );
        matches.push(PhotosRecoveryMatch {
            source: source.clone(),
            backup: selected.clone(),
            backup_blake3,
        });
    }
    Ok(matches)
}

fn validate_photos_originals(originals: &[PhotosBackupOriginalRecord]) -> Result<()> {
    for original in originals {
        let format = detect_true_format(&original.original_path)?;
        anyhow::ensure!(
            is_jpeg_original(format),
            "backup Photos UUID {} resolves to {:?}, not the original JPEG required for JXL recovery",
            original.uuid,
            format
        );
    }
    Ok(())
}

fn export_photos_originals(
    backup: &Path,
    destination: &Path,
    matches: &[PhotosRecoveryMatch],
) -> Result<Vec<RecoveryManifestRecord>> {
    let uuids = matches
        .iter()
        .map(|record| record.backup.uuid.clone())
        .collect::<Vec<_>>();
    let originals = matches
        .iter()
        .map(|record| record.backup.clone())
        .collect::<Vec<_>>();
    validate_photos_originals(&originals)?;
    for matched in matches {
        anyhow::ensure!(
            file_hash(&matched.source.source_path)? == matched.source.source_blake3,
            "live Photos JXL {} changed after backup matching; recovery export was not started",
            matched.source.uuid
        );
        anyhow::ensure!(
            file_hash(&matched.backup.original_path)? == matched.backup_blake3,
            "backup Photos original {} changed after matching; recovery export was not started",
            matched.backup.uuid
        );
    }
    fs::create_dir_all(destination)?;
    let scratch = foundation::process_lock::get_mfb_tmp_dir()?;
    let mut uuid_file = tempfile::NamedTempFile::new_in(scratch)?;
    for uuid in &uuids {
        writeln!(uuid_file, "{uuid}")?;
    }
    uuid_file.flush()?;
    let report_path = destination.join(OSXPHOTOS_REPORT);
    let osxphotos = foundation::common_utils::resolve_tool_path("osxphotos")
        .context("osxphotos is required to collect originals from a Photos backup")?;
    let mut command = Command::new(osxphotos);
    command
        .arg("export")
        .arg(destination)
        .arg("--db")
        .arg(backup)
        .arg("--uuid-from-file")
        .arg(uuid_file.path())
        .args([
            "--skip-edited",
            "--skip-live",
            "--skip-raw",
            "--skip-bursts",
            "--sidecar",
            "xmp",
            "--directory",
            "{folder_album}",
            "--filename",
            "{original_name}",
            "--no-progress",
            "--update",
            "--update-errors",
            "--retry",
            "3",
            "--report",
        ])
        .arg(&report_path);
    let output = foundation::process_runner::run_command_with_liveness_timeout(
        &mut command,
        Duration::from_mins(5),
        Duration::from_hours(12),
        "export recovery originals from Photos backup",
    )?;
    anyhow::ensure!(
        output.status.success(),
        "osxphotos recovery export failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let rows: Vec<PhotosExportReportRow> =
        serde_json::from_slice(&fs::read(&report_path).context("read osxphotos recovery report")?)
            .context("parse osxphotos recovery report")?;
    let recovery_by_uuid = matches
        .iter()
        .map(|record| (record.backup.uuid.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let original_by_uuid = originals
        .iter()
        .map(|record| (record.uuid.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let expected = uuids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    anyhow::ensure!(
        expected.len() == uuids.len(),
        "resolved Photos backup contains duplicate UUID matches"
    );
    let mut media_seen = BTreeSet::new();
    let mut xmp_seen = BTreeSet::new();
    let canonical_destination = fs::canonicalize(destination)?;
    let mut records = Vec::new();
    for row in rows {
        if !expected.contains(row.uuid.as_str()) || row.missing || !row.error.trim().is_empty() {
            anyhow::bail!(
                "osxphotos recovery report rejected UUID {}: missing={} error={}",
                row.uuid,
                row.missing,
                row.error
            );
        }
        anyhow::ensure!(
            row.exported || row.skipped || row.new || row.updated,
            "osxphotos recovery report has no successful disposition for UUID {}",
            row.uuid
        );
        let path = fs::canonicalize(&row.filename)
            .with_context(|| format!("resolve osxphotos recovery output {}", row.filename))?;
        anyhow::ensure!(
            path.starts_with(&canonical_destination),
            "osxphotos recovery report escaped destination: {}",
            path.display()
        );
        let matched = recovery_by_uuid[&row.uuid.as_str()];
        let relative = relative_string(&path, &canonical_destination)?;
        if row.sidecar_xmp {
            anyhow::ensure!(
                path.extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("xmp")),
                "osxphotos marked a non-XMP file as sidecar for UUID {}",
                row.uuid
            );
            foundation::metadata::validate_xmp_sidecar(&path)
                .with_context(|| format!("validate exported recovery XMP for UUID {}", row.uuid))?;
            anyhow::ensure!(
                xmp_seen.insert(row.uuid.clone()),
                "osxphotos recovery report contains duplicate XMP output for UUID {}",
                row.uuid
            );
            records.push(RecoveryManifestRecord {
                identity: matched.source.uuid.clone(),
                source_jxl_blake3: matched.source.source_blake3.clone(),
                output_relative_path: relative,
                output_blake3: file_hash(&path)?,
                sidecar: true,
            });
            continue;
        }
        let format = detect_true_format(&path)?;
        if !is_jpeg_original(format) {
            anyhow::bail!(
                "osxphotos recovery report returned non-JPEG media for UUID {}: {}",
                row.uuid,
                path.display()
            );
        }
        let original = original_by_uuid[&row.uuid.as_str()];
        let expected_backup_hash = recovery_by_uuid[&row.uuid.as_str()].backup_blake3.as_str();
        anyhow::ensure!(
            file_hash(&path)? == expected_backup_hash,
            "exported Photos original differs from backup bytes for UUID {}",
            row.uuid
        );
        anyhow::ensure!(
            file_hash(&original.original_path)? == expected_backup_hash,
            "backup Photos original changed during export for UUID {}",
            row.uuid
        );
        anyhow::ensure!(
            media_seen.insert(row.uuid.clone()),
            "osxphotos recovery report contains duplicate media output for UUID {}",
            row.uuid
        );
        records.push(RecoveryManifestRecord {
            identity: matched.source.uuid.clone(),
            source_jxl_blake3: matched.source.source_blake3.clone(),
            output_relative_path: relative,
            output_blake3: file_hash(&path)?,
            sidecar: false,
        });
    }
    anyhow::ensure!(
        media_seen
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            == expected,
        "Photos recovery export produced originals for {} of {} UUIDs",
        media_seen.len(),
        expected.len()
    );
    anyhow::ensure!(
        xmp_seen.iter().map(String::as_str).collect::<BTreeSet<_>>() == expected,
        "Photos recovery export produced XMP sidecars for {} of {} UUIDs",
        xmp_seen.len(),
        expected.len()
    );
    for matched in matches {
        anyhow::ensure!(
            file_hash(&matched.source.source_path)? == matched.source.source_blake3,
            "live Photos JXL {} changed during recovery export; rerun audit before consuming the manifest",
            matched.source.uuid
        );
        anyhow::ensure!(
            file_hash(&matched.backup.original_path)? == matched.backup_blake3,
            "backup Photos original {} changed during recovery export",
            matched.backup.uuid
        );
    }
    Ok(records)
}

fn collect_photos_recovery(
    source: &Path,
    backup: &Path,
    destination: &Path,
    dry_run: bool,
) -> Result<RecoveryCollectionSummary> {
    let recovery = list_photos_jxl_recovery_records(source)?;
    if recovery.is_empty() {
        return Ok(RecoveryCollectionSummary::default());
    }
    let matches = resolve_photos_recovery_matches(backup, &recovery)?;
    if dry_run {
        for matched in &matches {
            println!(
                "[DRY-RUN] recover Photos {} ({}) <- {} ({})",
                matched.source.uuid,
                matched.source.original_filename,
                matched.backup.uuid,
                matched.backup.original_filename
            );
        }
        return Ok(RecoveryCollectionSummary {
            selected: recovery.len(),
            ..RecoveryCollectionSummary::default()
        });
    }
    let _incomplete_manifest = write_manifest(
        destination,
        source,
        backup,
        "photos",
        false,
        Vec::new(),
        Vec::new(),
    )?;
    let records = export_photos_originals(backup, destination, &matches)?;
    let copied = records.len();
    let manifest = write_manifest(
        destination,
        source,
        backup,
        "photos",
        true,
        records,
        Vec::new(),
    )?;
    Ok(RecoveryCollectionSummary {
        selected: recovery.len(),
        copied,
        manifest: Some(manifest),
        ..RecoveryCollectionSummary::default()
    })
}

fn comparison_destination(destination: &Path, source: &Path, backup: &Path) -> Result<PathBuf> {
    let resolved = if destination.exists() {
        checked_real_directory(destination, "comparison destination")?
    } else {
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent = checked_real_directory(parent, "comparison destination parent")?;
        let name = destination
            .file_name()
            .context("comparison destination has no final component")?;
        anyhow::ensure!(
            Path::new(name)
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "comparison destination has an unsafe final component"
        );
        parent.join(name)
    };
    anyhow::ensure!(
        !resolved.starts_with(source) && !resolved.starts_with(backup),
        "comparison destination must not be inside either compared input"
    );
    Ok(resolved)
}

fn write_comparison_report(
    destination: &Path,
    report: &RecoveryComparisonReport,
) -> Result<PathBuf> {
    fs::create_dir_all(destination)?;
    let path = destination.join(COMPARISON_REPORT);
    let bytes = serde_json::to_vec_pretty(report)?;
    let mut staged = foundation::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "backup_comparison_report",
        destination,
        "mfb-comparison-",
        ".json",
    )?;
    staged.as_file_mut().write_all(&bytes)?;
    staged.as_file_mut().write_all(b"\n")?;
    staged.as_file_mut().flush()?;
    staged.as_file().sync_all()?;
    staged.persist(&path).map_err(|error| error.error)?;
    foundation::io_utils::sync_committed_file_and_parent(&path)?;
    Ok(path)
}

fn compare_photos_libraries(
    source: &Path,
    backup: &Path,
    destination: &Path,
) -> Result<RecoveryComparisonSummary> {
    let osxphotos = foundation::common_utils::resolve_tool_path("osxphotos")
        .context("osxphotos is required to compare Photos libraries")?;
    let mut command = Command::new(osxphotos);
    command.arg("compare").arg("--json").arg(source).arg(backup);
    let output = foundation::process_runner::run_command_with_liveness_timeout(
        &mut command,
        Duration::from_mins(5),
        Duration::from_hours(12),
        "compare Photos libraries",
    )?;
    anyhow::ensure!(
        output.status.success(),
        "osxphotos Photos comparison failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let photos: PhotosComparisonReport = serde_json::from_slice(&output.stdout)
        .context("parse osxphotos Photos comparison report")?;
    anyhow::ensure!(
        fs::canonicalize(&photos.library_a)? == source
            && fs::canonicalize(&photos.library_b)? == backup,
        "osxphotos comparison report identifies different libraries than requested"
    );
    let matched = photos.in_a_and_b_same.len();
    let source_only = photos.in_a_not_b.len();
    let backup_only = photos.in_b_not_a.len();
    let different = photos.in_a_and_b_different.len();
    let report = RecoveryComparisonReport {
        schema: "MFB_PHOTOS_LIBRARY_COMPARISON_V1",
        complete: true,
        source_kind: "photos_library",
        source_identity: path_identity(source)?,
        backup_identity: path_identity(backup)?,
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock precedes Unix epoch")?
            .as_secs(),
        matched: photos.in_a_and_b_same,
        source_only: photos.in_a_not_b,
        backup_only: photos.in_b_not_a,
        different: photos.in_a_and_b_different,
        needs_review: Vec::new(),
    };
    Ok(RecoveryComparisonSummary {
        matched,
        source_only,
        backup_only,
        different,
        needs_review: 0,
        report: Some(write_comparison_report(destination, &report)?),
    })
}

/// Compare two Photos libraries without changing either library or its assets.
///
/// # Errors
/// Rejects non-Photos inputs, unsafe destinations, or an incomplete upstream
/// Photos comparison. File/folder comparison belongs to a dedicated external
/// deduplication tool and is deliberately not implemented here.
pub fn run_recovery_comparison(
    source: &Path,
    backup: &Path,
    destination: &Path,
) -> Result<RecoveryComparisonSummary> {
    let source = checked_real_input(source, "comparison source")?;
    let backup = checked_real_input(backup, "comparison backup")?;
    let destination = comparison_destination(destination, &source, &backup)?;
    anyhow::ensure!(
        source.is_dir() && is_photos_library(&source),
        "comparison is only supported for two Photos libraries; use an external file deduplicator for folders/files"
    );
    anyhow::ensure!(
        backup.is_dir() && is_photos_library(&backup),
        "comparison is only supported for two Photos libraries; use an external file deduplicator for folders/files"
    );
    compare_photos_libraries(&source, &backup, &destination)
}

/// Collect originals for only the live, non-reconstructible JXL set.
///
/// # Errors
/// Rejects mixed folder/Photos inputs, unsafe path relationships, ambiguous
/// backup matches, byte changes, incomplete Photos exports, or missing XMP proof.
pub fn run_recovery_collection(
    source: &Path,
    backup: &Path,
    destination: &Path,
    dry_run: bool,
) -> Result<RecoveryCollectionSummary> {
    let source = checked_real_input(source, "audited source")?;
    let backup = checked_real_input(backup, "backup source")?;
    anyhow::ensure!(source != backup, "audited source and backup must differ");
    let destination = checked_destination(destination, &source, &backup)?;
    let source_is_photos = source.is_dir() && is_photos_library(&source);
    let backup_is_photos = backup.is_dir() && is_photos_library(&backup);
    anyhow::ensure!(
        source_is_photos == backup_is_photos,
        "audited source and backup must both be filesystem items or both be Photos libraries"
    );
    if source_is_photos {
        println!("[RECOVERY] exact Photos identity collection");
        collect_photos_recovery(&source, &backup, &destination, dry_run)
    } else {
        anyhow::ensure!(
            source.is_file() || backup.is_dir(),
            "an audited folder requires a backup folder; a backup file is valid only for one audited JXL"
        );
        println!("[RECOVERY] exact filesystem collection");
        collect_folder_recovery(&source, &backup, &destination, dry_run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photos_backup_candidate(
        uuid: &str,
        name: &str,
        album: &[&str],
    ) -> PhotosBackupOriginalRecord {
        PhotosBackupOriginalRecord {
            uuid: uuid.to_string(),
            original_filename: name.to_string(),
            original_path: PathBuf::from("/tmp").join(name),
            original_uti: "public.jpeg".to_string(),
            capture_date: None,
            album_paths: vec![album.iter().map(|value| (*value).to_string()).collect()],
        }
    }

    #[test]
    fn exact_relative_backup_match_rejects_ambiguity_and_spoofed_extensions() -> Result<()> {
        let backup = tempfile::tempdir()?;
        let dir = backup.path().join("album");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("photo.jpg"), [0xff, 0xd8, 0xff, 0xd9])?;
        fs::write(dir.join("photo.png"), b"not a png")?;
        let found = find_folder_backup_original(backup.path(), Path::new("album/photo.jxl"))?;
        assert_eq!(
            found.file_name().and_then(|name| name.to_str()),
            Some("photo.jpg")
        );
        let single = find_folder_backup_original(&dir.join("photo.jpg"), Path::new("photo.jxl"))?;
        assert_eq!(single, dir.join("photo.jpg"));

        fs::write(
            dir.join("photo.jpeg"),
            [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0xff, 0xd9],
        )?;
        assert!(find_folder_backup_original(backup.path(), Path::new("album/photo.jxl")).is_err());

        let png_backup = tempfile::tempdir()?;
        fs::write(png_backup.path().join("photo.png"), b"\x89PNG\r\n\x1a\n")?;
        assert!(
            find_folder_backup_original(png_backup.path(), Path::new("photo.jxl")).is_err(),
            "recovery collection must never substitute a non-JPEG static original"
        );
        Ok(())
    }

    #[test]
    fn photos_backup_match_uses_exact_identity_without_date_guessing() -> Result<()> {
        let source = PhotosJxlRecoveryRecord {
            uuid: "source-uuid".to_string(),
            original_filename: "IMG_0001.JXL".to_string(),
            source_path: PathBuf::from("fixture/IMG_0001.JXL"),
            source_blake3: "source-hash".to_string(),
            album_paths: vec![vec!["Family".to_string(), "2025".to_string()]],
        };
        let exact_uuid =
            photos_backup_candidate("source-uuid", "IMG_0001.JPG", &["Different", "Album"]);
        let same_album =
            photos_backup_candidate("backup-uuid", "IMG_0001.jpeg", &["Family", "2025"]);
        assert_eq!(
            select_photos_backup_match(&source, &[exact_uuid.clone(), same_album.clone()])?.uuid,
            exact_uuid.uuid
        );

        let source_with_new_uuid = PhotosJxlRecoveryRecord {
            uuid: "new-jxl-uuid".to_string(),
            ..source.clone()
        };
        assert_eq!(
            select_photos_backup_match(
                &source_with_new_uuid,
                &[exact_uuid.clone(), same_album.clone()],
            )?
            .uuid,
            same_album.uuid
        );

        let ambiguous =
            photos_backup_candidate("another-backup-uuid", "IMG_0001.jpg", &["Family", "2025"]);
        assert!(
            select_photos_backup_match(&source_with_new_uuid, &[same_album, ambiguous],).is_err(),
            "duplicate filename and album identity must remain explicit instead of choosing the earliest date"
        );
        Ok(())
    }

    #[test]
    fn backup_comparison_rejects_filesystem_folder_inputs() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let backup = root.path().join("backup");
        let report_dir = root.path().join("report");
        fs::create_dir_all(&source)?;
        fs::create_dir_all(&backup)?;
        let error = run_recovery_comparison(&source, &backup, &report_dir)
            .expect_err("filesystem folder comparison must be rejected");
        assert!(
            error
                .to_string()
                .contains("only supported for two Photos libraries")
        );
        Ok(())
    }
}
