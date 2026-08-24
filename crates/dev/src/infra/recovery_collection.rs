//! Collect only originals proven necessary by the JXL recovery audit.
//!
//! Folder backups are matched by the audited relative directory and basename.
//! Photos backups are matched by the Photos UUID and exported read-only through
//! `osxphotos`. Neither path guesses when identity is ambiguous.

use anyhow::{Context, Result};
use foundation::image::format_detect::{FormatKind, detect_true_format};
use foundation::image::jxl_utils::JpegReconstructionEligibility;
use foundation::image::photos_jxl_audit::{
    PhotosBackupOriginalRecord, PhotosJxlRecoveryRecord, list_photos_jxl_recovery_records,
    photos_backup_originals_by_uuid,
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

fn is_photos_library(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".photoslibrary") || lower.ends_with(".photolibrary")
        })
}

fn is_static_original(format: FormatKind) -> bool {
    !matches!(
        format,
        FormatKind::Jxl
            | FormatKind::Mp4
            | FormatKind::Mov
            | FormatKind::Mkv
            | FormatKind::Webm
            | FormatKind::Unknown
    )
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
            sidecars.push(entry.path());
        }
    }
    sidecars.sort();
    sidecars.dedup();
    Ok(sidecars)
}

fn find_folder_backup_original(backup: &Path, relative_jxl: &Path) -> Result<PathBuf> {
    let relative_parent = relative_jxl.parent().unwrap_or_else(|| Path::new(""));
    let directory = backup.join(relative_parent);
    let source_stem = relative_jxl
        .file_stem()
        .and_then(|value| value.to_str())
        .context("audited JXL filename is not UTF-8")?;
    let mut matches = Vec::new();
    if directory.exists() {
        let metadata = fs::symlink_metadata(&directory)?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "backup relative directory is not a real directory: {}",
            directory.display()
        );
        anyhow::ensure!(
            fs::canonicalize(&directory)?.starts_with(backup),
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
            if same_stem && is_static_original(detect_true_format(&path)?) {
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
    let mut selected = Vec::new();
    let mut needs_review = 0_usize;
    let mut failures = Vec::new();
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry.with_context(|| format!("scan audited folder {}", source.display()))?;
        if entry.path_is_symlink() || !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if detect_true_format(path)? != FormatKind::Jxl {
            continue;
        }
        match foundation::image::jxl_utils::probe_jpeg_reconstruction_eligibility(path) {
            Ok(JpegReconstructionEligibility::Exact) => {}
            Ok(
                JpegReconstructionEligibility::PixelOnly
                | JpegReconstructionEligibility::AdvertisedButRejected { .. },
            ) => selected.push(path.to_path_buf()),
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
            "folder",
            false,
            Vec::new(),
            summary.failed.clone(),
        )?);
    }
    for source_jxl in selected {
        let relative = source_jxl.strip_prefix(source)?;
        let source_hash = file_hash(&source_jxl)?;
        let original = match find_folder_backup_original(backup, relative) {
            Ok(path) => path,
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
                "[DRY-RUN] recover {} <- {}",
                relative.display(),
                original.display()
            );
            continue;
        }
        match copy_exact(destination, &original, &output) {
            Ok((hash, copied)) => {
                summary.copied += usize::from(copied);
                summary.skipped += usize::from(!copied);
                records.push(RecoveryManifestRecord {
                    identity: relative_string(&source_jxl, source)?,
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
        for sidecar in xmp_sidecars(&original)? {
            let sidecar_output = output
                .parent()
                .context("recovery output has no parent")?
                .join(sidecar.file_name().context("XMP sidecar has no filename")?);
            match copy_exact(destination, &sidecar, &sidecar_output) {
                Ok((hash, copied)) => {
                    summary.copied += usize::from(copied);
                    summary.skipped += usize::from(!copied);
                    records.push(RecoveryManifestRecord {
                        identity: relative_string(&source_jxl, source)?,
                        source_jxl_blake3: source_hash.clone(),
                        output_relative_path: relative_string(&sidecar_output, destination)?,
                        output_blake3: hash,
                        sidecar: true,
                    });
                }
                Err(error) => summary.failed.push(error.to_string()),
            }
        }
    }
    if !dry_run {
        summary.manifest = Some(write_manifest(
            destination,
            source,
            backup,
            "folder",
            summary.failed.is_empty(),
            records,
            summary.failed.clone(),
        )?);
    }
    Ok(summary)
}

fn validate_photos_originals(originals: &[PhotosBackupOriginalRecord]) -> Result<()> {
    for original in originals {
        let format = detect_true_format(&original.original_path)?;
        anyhow::ensure!(
            is_static_original(format),
            "backup Photos UUID {} resolves to {:?}, not an original static image suitable for JXL recovery",
            original.uuid,
            format
        );
    }
    Ok(())
}

fn export_photos_originals(
    backup: &Path,
    destination: &Path,
    recovery: &[PhotosJxlRecoveryRecord],
) -> Result<Vec<RecoveryManifestRecord>> {
    let uuids = recovery
        .iter()
        .map(|record| record.uuid.clone())
        .collect::<Vec<_>>();
    let originals = photos_backup_originals_by_uuid(backup, &uuids)?;
    validate_photos_originals(&originals)?;
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
    let recovery_by_uuid = recovery
        .iter()
        .map(|record| (record.uuid.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let original_by_uuid = originals
        .iter()
        .map(|record| (record.uuid.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let expected = uuids.iter().map(String::as_str).collect::<BTreeSet<_>>();
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
        let source = recovery_by_uuid[&row.uuid.as_str()];
        let relative = relative_string(&path, &canonical_destination)?;
        if row.sidecar_xmp {
            anyhow::ensure!(
                path.extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("xmp")),
                "osxphotos marked a non-XMP file as sidecar for UUID {}",
                row.uuid
            );
            xmp_seen.insert(row.uuid.clone());
            records.push(RecoveryManifestRecord {
                identity: row.uuid,
                source_jxl_blake3: source.source_blake3.clone(),
                output_relative_path: relative,
                output_blake3: file_hash(&path)?,
                sidecar: true,
            });
            continue;
        }
        let format = detect_true_format(&path)?;
        if !is_static_original(format) {
            continue;
        }
        let original = original_by_uuid[&row.uuid.as_str()];
        anyhow::ensure!(
            file_hash(&path)? == file_hash(&original.original_path)?,
            "exported Photos original differs from backup bytes for UUID {}",
            row.uuid
        );
        media_seen.insert(row.uuid.clone());
        records.push(RecoveryManifestRecord {
            identity: row.uuid,
            source_jxl_blake3: source.source_blake3.clone(),
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
    if dry_run {
        let uuids = recovery
            .iter()
            .map(|record| record.uuid.clone())
            .collect::<Vec<_>>();
        validate_photos_originals(&photos_backup_originals_by_uuid(backup, &uuids)?)?;
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
    let records = export_photos_originals(backup, destination, &recovery)?;
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

/// Collect originals for only the live, non-reconstructible JXL set.
///
/// # Errors
/// Rejects mixed folder/Photos inputs, unsafe path relationships, ambiguous
/// backup matches, byte changes, incomplete UUID exports, or missing XMP proof.
pub fn run_recovery_collection(
    source: &Path,
    backup: &Path,
    destination: &Path,
    dry_run: bool,
) -> Result<RecoveryCollectionSummary> {
    let source = checked_real_directory(source, "audited source")?;
    let backup = checked_real_directory(backup, "backup source")?;
    anyhow::ensure!(source != backup, "audited source and backup must differ");
    let destination = checked_destination(destination, &source, &backup)?;
    let source_is_photos = is_photos_library(&source);
    anyhow::ensure!(
        source_is_photos == is_photos_library(&backup),
        "audited source and backup must both be folders or both be Photos libraries"
    );
    if source_is_photos {
        println!("[RECOVERY] exact Photos UUID collection");
        collect_photos_recovery(&source, &backup, &destination, dry_run)
    } else {
        println!("[RECOVERY] exact relative-path folder collection");
        collect_folder_recovery(&source, &backup, &destination, dry_run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        fs::write(
            dir.join("photo.jpeg"),
            [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0xff, 0xd9],
        )?;
        assert!(find_folder_backup_original(backup.path(), Path::new("album/photo.jxl")).is_err());
        Ok(())
    }
}
