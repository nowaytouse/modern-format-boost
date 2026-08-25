//! JXL classification for Photos libraries plus idempotent audit-album marking.
//!
//! The Photos package is never written directly. Existing assets are added to
//! native `Photos` albums through `AppleScript`, then queried again by UUID for proof.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AUDIT_ROOT: &str = "MFB JXL Audit";
const AUDIT_ROOT_PREFIX: &str = "MFB JXL Audit/";
const CHECKPOINT_SCHEMA: &str = "MFB_PHOTOS_JXL_AUDIT_V1";
const ALBUM_MUTATION_BATCH_SIZE: usize = 50;

#[derive(Debug, Clone)]
pub struct PhotosJxlAuditScope {
    pub library: PathBuf,
    pub selected_asset_path: Option<PathBuf>,
    pub selected_container: Option<PhotosAuditContainerSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PhotosAuditContainerKind {
    Folder,
    Album,
}

impl PhotosAuditContainerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Album => "album",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotosAuditContainerSelection {
    pub kind: PhotosAuditContainerKind,
    pub id: String,
}

impl PhotosAuditContainerSelection {
    /// Build an exact Photos container selection from its native UUID.
    ///
    /// # Errors
    /// Rejects malformed identifiers before any Photos query or mutation.
    pub fn new(kind: PhotosAuditContainerKind, id: &str) -> Result<Self> {
        let id = normalize_photos_object_id(id)
            .context("Photos album/folder selection is not a valid UUID")?;
        Ok(Self { kind, id })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhotosAuditContainer {
    pub kind: PhotosAuditContainerKind,
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub path: Vec<String>,
}

#[derive(Debug)]
pub struct PhotosJxlAuditSummary {
    pub library: PathBuf,
    pub checkpoint: PathBuf,
    pub audited: usize,
    pub exact: usize,
    pub recovery_needed: usize,
    pub needs_review: usize,
    pub album_links_verified: usize,
}

/// One live, re-verified Photos asset that still needs its original recovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotosJxlRecoveryRecord {
    pub uuid: String,
    pub original_filename: String,
    pub source_blake3: String,
    pub album_paths: Vec<Vec<String>>,
}

/// The original media row resolved from a backup Photos library by UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotosBackupOriginalRecord {
    pub uuid: String,
    pub original_filename: String,
    pub original_path: PathBuf,
    pub original_uti: String,
    pub capture_date: Option<String>,
    pub album_paths: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditStatus {
    Exact,
    RecoveryNeeded,
    NeedsReview,
}

impl AuditStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::RecoveryNeeded => "recovery-needed",
            Self::NeedsReview => "needs-review",
        }
    }

    const fn album_component(self) -> Option<&'static str> {
        match self {
            Self::Exact => None,
            Self::RecoveryNeeded => Some("Recovery Needed"),
            Self::NeedsReview => Some("Needs Review"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PhotosAlbumInfo {
    #[serde(default)]
    uuid: String,
    title: String,
    #[serde(default)]
    folder_names: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PhotosQueryRecord {
    uuid: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    original_filename: String,
    #[serde(default)]
    uti: String,
    #[serde(default)]
    uti_original: String,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    date_original: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    path_derivatives: Vec<PathBuf>,
    #[serde(default)]
    ismissing: bool,
    #[serde(default)]
    album_info: Vec<PhotosAlbumInfo>,
}

#[derive(Debug, Deserialize)]
struct PhotosLibraryList {
    last_library: Option<PathBuf>,
}

#[derive(Debug)]
struct ClassifiedAsset {
    record: PhotosQueryRecord,
    status: AuditStatus,
    source_blake3: Option<String>,
    reason: String,
    target_albums: Vec<Vec<String>>,
}

/// Detect whether an input is a Photos package or one file inside it.
///
/// # Errors
/// Returns an error when the selected path cannot be resolved safely.
pub fn detect_photos_audit_scope(input: &Path) -> Result<Option<PhotosJxlAuditScope>> {
    let canonical = fs::canonicalize(input)
        .with_context(|| format!("could not resolve restore-jpeg input {}", input.display()))?;
    let Some(library) = canonical.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                let lower = name.to_ascii_lowercase();
                lower.ends_with(".photoslibrary") || lower.ends_with(".photolibrary")
            })
    }) else {
        return Ok(None);
    };
    let library = library.to_path_buf();
    anyhow::ensure!(
        library.is_dir(),
        "selected Photos library is not a directory: {}",
        library.display()
    );
    let selected_asset_path = (canonical != library).then_some(canonical);
    if let Some(selected) = &selected_asset_path {
        anyhow::ensure!(
            selected.is_file(),
            "select the Photos library package or one concrete asset file inside it: {}",
            selected.display()
        );
    }
    Ok(Some(PhotosJxlAuditScope {
        library,
        selected_asset_path,
        selected_container: None,
    }))
}

fn resolve_osxphotos() -> Result<PathBuf> {
    crate::common_utils::resolve_tool_path("osxphotos").ok_or_else(|| {
        anyhow::anyhow!(
            "osxphotos is required for Photos-library JXL audit; install it or expose it in PATH"
        )
    })
}

fn run_osxphotos(osxphotos: &Path, arguments: &[String], context: &str) -> Result<String> {
    let mut command = Command::new(osxphotos);
    command.args(arguments);
    let output = crate::process_runner::run_command_with_liveness_timeout(
        &mut command,
        Duration::from_mins(3),
        Duration::from_mins(20),
        context,
    )
    .with_context(|| format!("{context} did not complete"))?;
    anyhow::ensure!(
        output.status.success(),
        "{context} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).with_context(|| format!("{context} returned invalid UTF-8"))
}

fn photos_last_library(osxphotos: &Path) -> Result<Option<PathBuf>> {
    let output = run_osxphotos(
        osxphotos,
        &["list".to_string(), "--json".to_string()],
        "Photos library discovery",
    )?;
    Ok(serde_json::from_str::<PhotosLibraryList>(&output)
        .context("could not parse Photos library discovery JSON")?
        .last_library)
}

fn ensure_active_library(osxphotos: &Path, library: &Path) -> Result<()> {
    let canonical = fs::canonicalize(library)?;
    let active_matches = photos_last_library(osxphotos)?
        .and_then(|path| fs::canonicalize(path).ok())
        .is_some_and(|path| path == canonical);
    if active_matches {
        return Ok(());
    }

    let mut open = Command::new("/usr/bin/open");
    open.arg("-a").arg("Photos").arg(&canonical);
    let output = crate::process_runner::run_command_with_liveness_timeout(
        &mut open,
        Duration::from_secs(15),
        Duration::from_secs(30),
        "open selected Photos library",
    )?;
    anyhow::ensure!(
        output.status.success(),
        "could not open selected Photos library: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    for _ in 0..30 {
        if photos_last_library(osxphotos)?
            .and_then(|path| fs::canonicalize(path).ok())
            .is_some_and(|path| path == canonical)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    anyhow::bail!(
        "Photos did not activate the selected library {}; open it in Photos and retry",
        canonical.display()
    )
}

fn looks_like_photos_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn normalize_photos_object_id(value: &str) -> Option<String> {
    let id = value.split('/').next()?;
    looks_like_photos_uuid(id).then(|| id.to_ascii_uppercase())
}

#[derive(Debug)]
struct RawPhotosContainer {
    kind: PhotosAuditContainerKind,
    id: String,
    name: String,
    parent_id: Option<String>,
}

fn parse_photos_containers(output: &str) -> Result<Vec<PhotosAuditContainer>> {
    let mut raw = BTreeMap::<String, RawPhotosContainer>::new();
    for record in output.split('\u{1e}') {
        let record = record.trim_matches(['\r', '\n']);
        if record.is_empty() {
            continue;
        }
        let fields = record.split('\u{1f}').collect::<Vec<_>>();
        anyhow::ensure!(
            fields.len() == 4,
            "native Photos container listing returned a malformed record"
        );
        let kind = match fields[0] {
            "folder" => PhotosAuditContainerKind::Folder,
            "album" => PhotosAuditContainerKind::Album,
            _ => anyhow::bail!("native Photos container listing returned an unknown kind"),
        };
        let id = normalize_photos_object_id(fields[1])
            .context("native Photos container listing returned an invalid UUID")?;
        let name = fields[2].to_string();
        anyhow::ensure!(
            !name.is_empty() && !name.chars().any(char::is_control),
            "native Photos container listing returned an invalid name"
        );
        let parent_id = if fields[3].is_empty() {
            None
        } else {
            Some(
                normalize_photos_object_id(fields[3])
                    .context("native Photos container listing returned an invalid parent UUID")?,
            )
        };
        anyhow::ensure!(
            raw.insert(
                id.clone(),
                RawPhotosContainer {
                    kind,
                    id,
                    name,
                    parent_id,
                },
            )
            .is_none(),
            "native Photos container listing returned a duplicate UUID"
        );
    }

    let mut containers = Vec::with_capacity(raw.len());
    for container in raw.values() {
        let mut path = Vec::new();
        let mut cursor = Some(container.id.as_str());
        let mut seen = BTreeSet::new();
        while let Some(id) = cursor {
            anyhow::ensure!(
                seen.insert(id.to_string()) && seen.len() <= 64,
                "native Photos container hierarchy contains a cycle or is too deep"
            );
            let node = raw
                .get(id)
                .context("native Photos container hierarchy has a missing parent")?;
            path.push(node.name.clone());
            cursor = node.parent_id.as_deref();
        }
        path.reverse();
        if path
            .first()
            .is_some_and(|component| component == AUDIT_ROOT)
        {
            continue;
        }
        containers.push(PhotosAuditContainer {
            kind: container.kind,
            id: container.id.clone(),
            name: container.name.clone(),
            parent_id: container.parent_id.clone(),
            path,
        });
    }
    containers.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(containers)
}

fn list_photos_audit_containers_with(
    osxphotos: &Path,
    library: &Path,
) -> Result<Vec<PhotosAuditContainer>> {
    const SCRIPT: &str = r#"
property fieldSeparator : ASCII character 31
property recordSeparator : ASCII character 30

on normalizedID(rawID)
    set oldDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to "/"
    set normalized to text item 1 of (rawID as text)
    set AppleScript's text item delimiters to oldDelimiters
    return normalized
end normalizedID

on recordFor(kindName, objectID, objectName, parentID)
    return kindName & fieldSeparator & objectID & fieldSeparator & objectName & fieldSeparator & parentID & recordSeparator
end recordFor

on walkFolder(folderRef, parentID, depth)
    if depth > 64 then error "Photos folder hierarchy is too deep"
    tell application "Photos"
        set folderID to my normalizedID(id of folderRef)
        set resultText to my recordFor("folder", folderID, name of folderRef as text, parentID)
        repeat with childFolder in every folder of folderRef
            set resultText to resultText & my walkFolder(contents of childFolder, folderID, depth + 1)
        end repeat
        repeat with childAlbum in every album of folderRef
            set albumRef to contents of childAlbum
            set albumID to my normalizedID(id of albumRef)
            set resultText to resultText & my recordFor("album", albumID, name of albumRef as text, folderID)
        end repeat
    end tell
    return resultText
end walkFolder

on run
    set resultText to ""
    tell application "Photos"
        repeat with topFolder in every folder
            set resultText to resultText & my walkFolder(contents of topFolder, "", 1)
        end repeat
        repeat with topAlbum in every album
            set albumRef to contents of topAlbum
            set albumID to my normalizedID(id of albumRef)
            set resultText to resultText & my recordFor("album", albumID, name of albumRef as text, "")
        end repeat
    end tell
    return resultText
end run
"#;

    ensure_active_library(osxphotos, library)?;
    let mut command = Command::new("/usr/bin/osascript");
    command.arg("-e").arg(SCRIPT);
    let output = crate::process_runner::run_command_with_liveness_timeout(
        &mut command,
        Duration::from_mins(1),
        Duration::from_mins(3),
        "list native Photos folders and albums",
    )?;
    anyhow::ensure!(
        output.status.success(),
        "native Photos container listing failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    parse_photos_containers(
        &String::from_utf8(output.stdout)
            .context("native Photos container listing returned invalid UTF-8")?,
    )
}

/// List selectable native Photos folders and albums by stable UUID.
///
/// # Errors
/// Fails if the library is not an exact Photos package, cannot be activated, or
/// the native hierarchy is ambiguous or malformed.
pub fn list_photos_audit_containers(library: &Path) -> Result<Vec<PhotosAuditContainer>> {
    let scope = detect_photos_audit_scope(library)?
        .context("select a Photos library package to list folders and albums")?;
    anyhow::ensure!(
        scope.selected_asset_path.is_none(),
        "select the Photos library package, not an asset inside it"
    );
    let osxphotos = resolve_osxphotos()?;
    list_photos_audit_containers_with(&osxphotos, &scope.library)
}

/// Serialize a validated Photos hierarchy for native GUI/CLI handoff.
///
/// # Errors
/// Returns an error only if JSON serialization fails.
pub fn photos_audit_containers_json(containers: &[PhotosAuditContainer]) -> Result<String> {
    serde_json::to_string(containers).context("could not serialize Photos container hierarchy")
}

fn album_ids_for_selection(
    selection: &PhotosAuditContainerSelection,
    containers: &[PhotosAuditContainer],
) -> Result<BTreeSet<String>> {
    anyhow::ensure!(
        containers
            .iter()
            .any(|container| { container.kind == selection.kind && container.id == selection.id }),
        "selected Photos {} UUID no longer exists in this library",
        selection.kind.as_str()
    );
    if selection.kind == PhotosAuditContainerKind::Album {
        return Ok(std::iter::once(selection.id.clone()).collect());
    }

    let by_id = containers
        .iter()
        .map(|container| (container.id.as_str(), container))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeSet::new();
    for album in containers
        .iter()
        .filter(|container| container.kind == PhotosAuditContainerKind::Album)
    {
        let mut parent = album.parent_id.as_deref();
        let mut seen = BTreeSet::new();
        while let Some(parent_id) = parent {
            anyhow::ensure!(
                seen.insert(parent_id) && seen.len() <= 64,
                "native Photos container hierarchy contains a cycle or is too deep"
            );
            if parent_id == selection.id {
                selected.insert(album.id.clone());
                break;
            }
            parent = by_id
                .get(parent_id)
                .context("native Photos container hierarchy has a missing parent")?
                .parent_id
                .as_deref();
        }
    }
    Ok(selected)
}

fn validate_query_records(
    records: &[PhotosQueryRecord],
    expected: Option<&BTreeSet<String>>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for record in records {
        anyhow::ensure!(
            looks_like_photos_uuid(&record.uuid),
            "Photos query returned an invalid asset UUID"
        );
        if let Some(expected) = expected {
            anyhow::ensure!(
                expected.contains(&record.uuid),
                "Photos query returned an unexpected asset UUID"
            );
        }
        anyhow::ensure!(
            seen.insert(record.uuid.clone()),
            "Photos query returned a duplicate asset UUID"
        );
    }
    if let Some(expected) = expected {
        anyhow::ensure!(
            &seen == expected,
            "Photos query returned {} of {} requested UUIDs",
            seen.len(),
            expected.len()
        );
    }
    Ok(())
}

fn album_is_selected(album: &PhotosAlbumInfo, selected_album_ids: &BTreeSet<String>) -> bool {
    normalize_photos_object_id(&album.uuid).is_some_and(|id| selected_album_ids.contains(&id))
}

fn query_records(
    osxphotos: &Path,
    scope: &PhotosJxlAuditScope,
    selected_album_ids: Option<&BTreeSet<String>>,
) -> Result<Vec<PhotosQueryRecord>> {
    let mut arguments = vec![
        "query".to_string(),
        "--db".to_string(),
        scope.library.to_string_lossy().to_string(),
    ];
    if let Some(selected) = &scope.selected_asset_path {
        let stem = selected
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if looks_like_photos_uuid(stem) {
            arguments.extend(["--uuid".to_string(), stem.to_string()]);
        } else {
            arguments.extend(["--uti".to_string(), "public.jpeg-xl".to_string()]);
        }
    } else {
        arguments.extend(["--uti".to_string(), "public.jpeg-xl".to_string()]);
    }
    arguments.extend(["--json".to_string(), "--mute".to_string()]);
    let output = run_osxphotos(osxphotos, &arguments, "query Photos JXL candidates")?;
    let mut records: Vec<PhotosQueryRecord> =
        serde_json::from_str(&output).context("could not parse Photos JXL query JSON")?;
    validate_query_records(&records, None)?;
    if let Some(selected) = &scope.selected_asset_path {
        let canonical_selected = fs::canonicalize(selected)?;
        records.retain(|record| {
            record
                .path
                .iter()
                .chain(&record.path_derivatives)
                .filter_map(|path| fs::canonicalize(path).ok())
                .any(|path| path == canonical_selected)
        });
        anyhow::ensure!(
            records.len() == 1,
            "selected Photos asset resolved to {} database rows; expected exactly one",
            records.len()
        );
    }
    if let Some(selected_album_ids) = selected_album_ids {
        for record in &mut records {
            record
                .album_info
                .retain(|album| album_is_selected(album, selected_album_ids));
        }
        records.retain(|record| !record.album_info.is_empty());
    }
    records.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    Ok(records)
}

fn query_records_by_uuid(
    osxphotos: &Path,
    library: &Path,
    uuids: &[String],
) -> Result<Vec<PhotosQueryRecord>> {
    if uuids.is_empty() {
        return Ok(Vec::new());
    }
    let scratch = crate::process_lock::get_mfb_tmp_dir()?;
    let mut uuid_file = tempfile::NamedTempFile::new_in(scratch)?;
    for uuid in uuids {
        writeln!(uuid_file, "{uuid}")?;
    }
    uuid_file.flush()?;
    let output = run_osxphotos(
        osxphotos,
        &[
            "query".to_string(),
            "--db".to_string(),
            library.to_string_lossy().to_string(),
            "--uuid-from-file".to_string(),
            uuid_file.path().to_string_lossy().to_string(),
            "--json".to_string(),
            "--mute".to_string(),
        ],
        "re-query Photos audit UUIDs",
    )?;
    let records: Vec<PhotosQueryRecord> =
        serde_json::from_str(&output).context("could not parse Photos audit verification JSON")?;
    let expected = uuids.iter().cloned().collect::<BTreeSet<_>>();
    anyhow::ensure!(
        expected.len() == uuids.len(),
        "Photos audit UUID request contains duplicates"
    );
    validate_query_records(&records, Some(&expected))?;
    Ok(records)
}

fn query_all_records(osxphotos: &Path, library: &Path) -> Result<Vec<PhotosQueryRecord>> {
    let output = run_osxphotos(
        osxphotos,
        &[
            "query".to_string(),
            "--db".to_string(),
            library.to_string_lossy().to_string(),
            "--json".to_string(),
            "--mute".to_string(),
        ],
        "query all Photos backup assets",
    )?;
    let mut records: Vec<PhotosQueryRecord> = serde_json::from_str(&output)
        .context("could not parse complete Photos backup query JSON")?;
    validate_query_records(&records, None)?;
    records.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    Ok(records)
}

fn safe_original_filename(record: &PhotosQueryRecord) -> Result<String> {
    let original_filename = if record.original_filename.trim().is_empty() {
        record.filename.clone()
    } else {
        record.original_filename.clone()
    };
    anyhow::ensure!(
        !original_filename.trim().is_empty()
            && Path::new(&original_filename)
                .file_name()
                .is_some_and(|name| name == original_filename.as_str()),
        "Photos asset {} returned an unsafe original filename",
        record.uuid
    );
    Ok(original_filename)
}

fn source_album_paths(record: &PhotosQueryRecord) -> Vec<Vec<String>> {
    let mut paths = record
        .album_info
        .iter()
        .filter(|album| {
            album
                .folder_names
                .first()
                .is_none_or(|name| name != AUDIT_ROOT)
                && album.title != AUDIT_ROOT
                && !album.title.starts_with(AUDIT_ROOT_PREFIX)
        })
        .map(|album| {
            let mut components = album.folder_names.clone();
            components.push(album.title.clone());
            components
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        paths.push(vec!["Unfiled".to_string()]);
    }
    paths
}

fn current_album_paths(record: &PhotosQueryRecord) -> BTreeSet<Vec<String>> {
    record
        .album_info
        .iter()
        .map(|album| {
            let mut components = album.folder_names.clone();
            components.push(album.title.clone());
            components
        })
        .collect()
}

fn target_album_paths(record: &PhotosQueryRecord, status: AuditStatus) -> Vec<Vec<String>> {
    let Some(classification) = status.album_component() else {
        return Vec::new();
    };
    source_album_paths(record)
        .into_iter()
        .map(|source| {
            let mut target = vec![AUDIT_ROOT.to_string(), classification.to_string()];
            target.extend(source);
            target
        })
        .collect()
}

fn classify_asset(scope: &PhotosJxlAuditScope, record: PhotosQueryRecord) -> ClassifiedAsset {
    let review = |record: PhotosQueryRecord, reason: String| ClassifiedAsset {
        target_albums: target_album_paths(&record, AuditStatus::NeedsReview),
        record,
        status: AuditStatus::NeedsReview,
        source_blake3: None,
        reason,
    };
    if record.ismissing {
        return review(
            record,
            "Photos reports the original asset as missing".to_string(),
        );
    }
    let Some(path) = record.path.clone() else {
        return review(
            record,
            "Photos has no locally readable original path".to_string(),
        );
    };
    let Ok(canonical) = fs::canonicalize(&path) else {
        return review(
            record,
            "Photos original path is not locally readable".to_string(),
        );
    };
    if !canonical.starts_with(&scope.library) {
        return review(
            record,
            "Photos original path escaped the selected library".to_string(),
        );
    }
    match crate::image::format_detect::detect_true_format(&canonical) {
        Ok(crate::image::format_detect::FormatKind::Jxl) => {}
        Ok(format) => {
            return review(
                record,
                format!("JXL-named Photos asset has {format:?} payload"),
            );
        }
        Err(error) => return review(record, format!("JXL payload detection failed: {error}")),
    }
    let source_blake3 = match crate::common_utils::calculate_blake3_hash(&canonical) {
        Ok(hash) => hash,
        Err(error) => return review(record, format!("JXL BLAKE3 failed: {error}")),
    };
    let (status, reason) = match crate::jxl_utils::probe_jpeg_reconstruction_eligibility(&canonical)
    {
        Ok(crate::jxl_utils::JpegReconstructionEligibility::Exact) => (
            AuditStatus::Exact,
            "official djxl exactly reconstructed the original JPEG".to_string(),
        ),
        Ok(crate::jxl_utils::JpegReconstructionEligibility::PixelOnly) => (
            AuditStatus::RecoveryNeeded,
            "healthy JXL has no exact JPEG reconstruction payload".to_string(),
        ),
        Ok(crate::jxl_utils::JpegReconstructionEligibility::AdvertisedButRejected {
            diagnostic,
        }) => (
            AuditStatus::RecoveryNeeded,
            format!("official djxl rejected advertised JPEG reconstruction: {diagnostic}"),
        ),
        Err(error) => (
            AuditStatus::NeedsReview,
            format!("JXL probe failed: {error}"),
        ),
    };
    ClassifiedAsset {
        target_albums: target_album_paths(&record, status),
        record,
        status,
        source_blake3: Some(source_blake3),
        reason,
    }
}

fn exact_photos_library_scope(library: &Path) -> Result<PhotosJxlAuditScope> {
    let scope = detect_photos_audit_scope(library)?
        .context("selected recovery source is not a Photos library package")?;
    anyhow::ensure!(
        scope.selected_asset_path.is_none(),
        "select the Photos library package itself, not a file inside it"
    );
    Ok(scope)
}

/// Re-read the audited Photos library and return only assets whose current JXL
/// bytes still prove that exact JPEG reconstruction is unavailable.
///
/// # Errors
/// Fails closed when the audit album is stale, a source changed during the
/// probe, or the Photos query cannot be proven complete.
pub fn list_photos_jxl_recovery_records(library: &Path) -> Result<Vec<PhotosJxlRecoveryRecord>> {
    let scope = exact_photos_library_scope(library)?;
    let _library_lock = crate::process_lock::acquire_dir_lock(&scope.library)?;
    let osxphotos = resolve_osxphotos()?;
    let records = query_records(&osxphotos, &scope, None)?;
    let mut recovery = Vec::new();

    for record in records {
        let is_marked = current_album_paths(&record).iter().any(|path| {
            path.first().is_some_and(|value| value == AUDIT_ROOT)
                && path.get(1).is_some_and(|value| value == "Recovery Needed")
        });
        if !is_marked {
            continue;
        }
        let album_paths = source_album_paths(&record);
        let classified = classify_asset(&scope, record);
        anyhow::ensure!(
            classified.status == AuditStatus::RecoveryNeeded,
            "Photos recovery album contains UUID {} whose live status is {}; rerun restore-jpeg audit before collecting from backup",
            classified.record.uuid,
            classified.status.as_str()
        );
        let source_blake3 = classified
            .source_blake3
            .context("live Photos recovery candidate has no source hash")?;
        let original_filename = safe_original_filename(&classified.record)?;
        recovery.push(PhotosJxlRecoveryRecord {
            uuid: classified.record.uuid,
            original_filename,
            source_blake3,
            album_paths,
        });
    }
    recovery.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    Ok(recovery)
}

/// Resolve the original rows for audited UUIDs in a read-only backup library.
///
/// # Errors
/// Fails when any UUID is absent, duplicated, missing on disk, or no longer has
/// an unambiguous original filename/path.
pub fn photos_backup_originals_by_uuid(
    library: &Path,
    uuids: &[String],
) -> Result<Vec<PhotosBackupOriginalRecord>> {
    let scope = exact_photos_library_scope(library)?;
    let osxphotos = resolve_osxphotos()?;
    let records = query_records_by_uuid(&osxphotos, &scope.library, uuids)?;
    let mut originals = records
        .into_iter()
        .map(photos_backup_original_from_record)
        .collect::<Result<Vec<_>>>()?;
    originals.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    Ok(originals)
}

fn photos_backup_original_from_record(
    record: PhotosQueryRecord,
) -> Result<PhotosBackupOriginalRecord> {
    anyhow::ensure!(
        !record.ismissing,
        "backup Photos asset {} is marked missing",
        record.uuid
    );
    let original_path = record
        .path
        .clone()
        .with_context(|| format!("backup Photos asset {} has no original path", record.uuid))?;
    let metadata = fs::symlink_metadata(&original_path).with_context(|| {
        format!(
            "backup Photos original for UUID {} is unavailable at {}",
            record.uuid,
            original_path.display()
        )
    })?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0,
        "backup Photos original for UUID {} is not a non-empty regular file",
        record.uuid
    );
    let original_filename = safe_original_filename(&record)?;
    let original_uti = if record.uti_original.trim().is_empty() {
        record.uti.clone()
    } else {
        record.uti_original.clone()
    };
    let capture_date = record.date_original.clone().or(record.date.clone());
    let album_paths = source_album_paths(&record);
    Ok(PhotosBackupOriginalRecord {
        uuid: record.uuid,
        original_filename,
        original_path,
        original_uti,
        capture_date,
        album_paths,
    })
}

fn folded_filename_stem(name: &str) -> Result<String> {
    let safe = Path::new(name);
    anyhow::ensure!(
        safe.file_name().is_some_and(|component| component == name),
        "Photos original filename is unsafe"
    );
    let stem = safe
        .file_stem()
        .and_then(|value| value.to_str())
        .context("Photos original filename has no UTF-8 stem")?;
    anyhow::ensure!(!stem.is_empty(), "Photos original filename has an empty stem");
    Ok(stem.to_lowercase())
}

/// Resolve all JPEG candidates in a read-only backup Photos library whose
/// original filename stem exactly matches one of the requested audited names.
///
/// # Errors
/// Fails closed if a matching database row is missing, unsafe, unreadable, or
/// cannot be proven to contain a real JPEG payload.
pub fn photos_backup_original_candidates(
    library: &Path,
    requested_names: &[String],
) -> Result<Vec<PhotosBackupOriginalRecord>> {
    let requested = requested_names
        .iter()
        .map(|name| folded_filename_stem(name))
        .collect::<Result<BTreeSet<_>>>()?;
    let scope = exact_photos_library_scope(library)?;
    let osxphotos = resolve_osxphotos()?;
    let mut candidates = Vec::new();
    for record in query_all_records(&osxphotos, &scope.library)? {
        let Ok(original_filename) = safe_original_filename(&record) else {
            continue;
        };
        let Ok(stem) = folded_filename_stem(&original_filename) else {
            continue;
        };
        if !requested.contains(&stem) {
            continue;
        }
        let candidate = photos_backup_original_from_record(record)?;
        if crate::image::format_detect::detect_true_format(&candidate.original_path)?
            == crate::image::format_detect::FormatKind::Jpeg
        {
            candidates.push(candidate);
        }
    }
    candidates.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    Ok(candidates)
}

fn add_uuids_to_album(album: &[String], uuids: &[String]) -> Result<()> {
    const SCRIPT: &str = r#"
on run argv
    set componentCount to item 1 of argv as integer
    set folderNames to items 2 thru componentCount of argv
    set albumName to item (componentCount + 1) of argv
    set assetUUIDs to items (componentCount + 2) thru -1 of argv
    tell application "Photos"
        set parentFolder to missing value
        repeat with folderName in folderNames
            set folderName to contents of folderName
            if parentFolder is missing value then
                set matchingFolders to every folder whose name is folderName
                if (count of matchingFolders) is 0 then
                    set parentFolder to make new folder named folderName
                else if (count of matchingFolders) is 1 then
                    set parentFolder to item 1 of matchingFolders
                else
                    error "ambiguous Photos audit folder name"
                end if
            else
                set matchingFolders to every folder of parentFolder whose name is folderName
                if (count of matchingFolders) is 0 then
                    set parentFolder to make new folder at parentFolder named folderName
                else if (count of matchingFolders) is 1 then
                    set parentFolder to item 1 of matchingFolders
                else
                    error "ambiguous Photos audit subfolder name"
                end if
            end if
        end repeat
        set matchingAlbums to every album of parentFolder whose name is albumName
        if (count of matchingAlbums) is 0 then
            set targetAlbum to make new album at parentFolder named albumName
        else if (count of matchingAlbums) is 1 then
            set targetAlbum to item 1 of matchingAlbums
        else
            error "ambiguous Photos audit album name"
        end if
        set selectedItems to {}
        repeat with assetUUID in assetUUIDs
            set assetUUID to contents of assetUUID
            set matchedItems to every media item whose id contains assetUUID
            if (count of matchedItems) is not 1 then error "expected one Photos asset for UUID " & assetUUID
            set end of selectedItems to item 1 of matchedItems
        end repeat
        add selectedItems to targetAlbum
    end tell
end run
"#;

    anyhow::ensure!(
        album.len() >= 2,
        "Photos audit album path needs a folder and album"
    );
    anyhow::ensure!(!uuids.is_empty(), "Photos audit album UUID list is empty");
    anyhow::ensure!(
        album
            .iter()
            .all(|component| !component.is_empty() && !component.chars().any(char::is_control)),
        "Photos audit album path contains an empty or control-character component"
    );
    let mut command = Command::new("/usr/bin/osascript");
    command.arg("-e").arg(SCRIPT).arg("--");
    command.arg(album.len().to_string());
    command.args(album);
    command.args(uuids);
    let output = crate::process_runner::run_command_with_liveness_timeout(
        &mut command,
        Duration::from_secs(30),
        Duration::from_mins(2),
        "add existing Photos assets to JXL audit album",
    )?;
    anyhow::ensure!(
        output.status.success(),
        "native Photos album update failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::new(), |mut encoded, byte| {
            use std::fmt::Write;
            let _ = write!(encoded, "{byte:02x}");
            encoded
        })
}

fn checkpoint_path(scope: &PhotosJxlAuditScope) -> Result<PathBuf> {
    let directory = crate::process_lock::get_mfb_root()?.join("state/photos_jxl_audit");
    fs::create_dir_all(&directory)?;
    let library_hash = crate::process_lock::hash_path_to_hex(&scope.library)?;
    let selection = if let Some(path) = scope.selected_asset_path.as_deref() {
        Some(crate::process_lock::hash_path_to_hex(path)?)
    } else {
        scope.selected_container.as_ref().map(|selection| {
            blake3::hash(format!("{}:{}", selection.kind.as_str(), selection.id).as_bytes())
                .to_hex()
                .to_string()
        })
    };
    Ok(directory.join(match selection {
        Some(selection) => format!("{library_hash}-{selection}.tsv"),
        None => format!("{library_hash}.tsv"),
    }))
}

fn write_checkpoint(
    path: &Path,
    assets: &[ClassifiedAsset],
    verified: &BTreeMap<String, BTreeSet<Vec<String>>>,
) -> Result<()> {
    use std::fmt::Write as _;

    let updated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_secs();
    let mut text = format!(
        "# {CHECKPOINT_SCHEMA}\nuuid\tstatus\tsource_blake3\ttarget_album_hex\tverified\treason_hex\tupdated_unix_seconds\n"
    );
    for asset in assets {
        if asset.target_albums.is_empty() {
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t\ttrue\t{}\t{updated}",
                asset.record.uuid,
                asset.status.as_str(),
                asset.source_blake3.as_deref().unwrap_or(""),
                hex_encode(&asset.reason),
            );
            continue;
        }
        for album in &asset.target_albums {
            let is_verified = verified
                .get(&asset.record.uuid)
                .is_some_and(|albums| albums.contains(album));
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}\t{}\t{}\t{updated}",
                asset.record.uuid,
                asset.status.as_str(),
                asset.source_blake3.as_deref().unwrap_or(""),
                hex_encode(&album.join("/")),
                is_verified,
                hex_encode(&asset.reason),
            );
        }
    }
    let parent = path
        .parent()
        .context("Photos audit checkpoint has no parent")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(text.as_bytes())?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    crate::io_utils::sync_committed_file_and_parent(path)?;
    Ok(())
}

/// Audit historical JXL assets in Photos and mark only non-exact assets.
///
/// # Errors
/// Fails closed if library identity, classification, album mutation, live UUID
/// reconciliation, or checkpoint persistence cannot be proven.
pub fn run_photos_jxl_audit(scope: &PhotosJxlAuditScope) -> Result<PhotosJxlAuditSummary> {
    let _library_lock = crate::process_lock::acquire_dir_lock(&scope.library)?;
    let osxphotos = resolve_osxphotos()?;
    ensure_active_library(&osxphotos, &scope.library)?;
    anyhow::ensure!(
        scope.selected_asset_path.is_none() || scope.selected_container.is_none(),
        "select either one Photos asset or one Photos album/folder, not both"
    );
    let selected_album_ids = if let Some(selection) = &scope.selected_container {
        let containers = list_photos_audit_containers_with(&osxphotos, &scope.library)?;
        Some(album_ids_for_selection(selection, &containers)?)
    } else {
        None
    };
    let records = query_records(&osxphotos, scope, selected_album_ids.as_ref())?;
    let assets = records
        .into_iter()
        .map(|record| classify_asset(scope, record))
        .collect::<Vec<_>>();
    let checkpoint = checkpoint_path(scope)?;

    let mut verified = BTreeMap::<String, BTreeSet<Vec<String>>>::new();
    let mut pending = BTreeMap::<Vec<String>, Vec<String>>::new();
    for asset in &assets {
        let current = current_album_paths(&asset.record);
        for target in &asset.target_albums {
            if current.contains(target) {
                verified
                    .entry(asset.record.uuid.clone())
                    .or_default()
                    .insert(target.clone());
            } else {
                pending
                    .entry(target.clone())
                    .or_default()
                    .push(asset.record.uuid.clone());
            }
        }
    }
    write_checkpoint(&checkpoint, &assets, &verified)?;

    for (album, uuids) in pending {
        for batch in uuids.chunks(ALBUM_MUTATION_BATCH_SIZE) {
            ensure_active_library(&osxphotos, &scope.library)?;
            let add_result = add_uuids_to_album(&album, batch);
            let refreshed = query_records_by_uuid(&osxphotos, &scope.library, batch)?;
            let mut missing = Vec::new();
            for uuid in batch {
                let present = refreshed
                    .iter()
                    .find(|record| &record.uuid == uuid)
                    .is_some_and(|record| current_album_paths(record).contains(&album));
                if present {
                    verified
                        .entry(uuid.clone())
                        .or_default()
                        .insert(album.clone());
                } else {
                    missing.push(uuid.clone());
                }
            }
            write_checkpoint(&checkpoint, &assets, &verified)?;
            if !missing.is_empty() {
                let add_detail = add_result.err().map_or_else(
                    || "Photos returned success".to_string(),
                    |error| error.to_string(),
                );
                anyhow::bail!(
                    "Photos audit album verification failed for {} UUID(s) in {} ({add_detail})",
                    missing.len(),
                    album.join("/")
                );
            }
        }
    }

    let exact = assets
        .iter()
        .filter(|asset| asset.status == AuditStatus::Exact)
        .count();
    let recovery_needed = assets
        .iter()
        .filter(|asset| asset.status == AuditStatus::RecoveryNeeded)
        .count();
    let needs_review = assets
        .iter()
        .filter(|asset| asset.status == AuditStatus::NeedsReview)
        .count();
    Ok(PhotosJxlAuditSummary {
        library: scope.library.clone(),
        checkpoint,
        audited: assets.len(),
        exact,
        recovery_needed,
        needs_review,
        album_links_verified: verified.values().map(BTreeSet::len).sum(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_record(uuid: &str) -> PhotosQueryRecord {
        PhotosQueryRecord {
            uuid: uuid.to_string(),
            filename: String::new(),
            original_filename: String::new(),
            uti: String::new(),
            uti_original: String::new(),
            date: None,
            date_original: None,
            path: None,
            path_derivatives: Vec::new(),
            ismissing: false,
            album_info: Vec::new(),
        }
    }

    #[test]
    fn query_record_validation_is_exact_and_duplicate_safe() {
        let first = "00000000-0000-0000-0000-000000000000";
        let second = "11111111-1111-1111-1111-111111111111";
        let expected = std::iter::once(first.to_string()).collect::<BTreeSet<_>>();

        assert!(validate_query_records(&[query_record(first)], Some(&expected)).is_ok());
        assert!(validate_query_records(&[query_record(first), query_record(first)], None).is_err());
        assert!(validate_query_records(&[query_record(second)], Some(&expected)).is_err());
        assert!(validate_query_records(&[], Some(&expected)).is_err());
        assert!(validate_query_records(&[query_record("not-a-uuid")], None).is_err());
    }

    #[test]
    fn audit_album_path_preserves_source_hierarchy() {
        let record = PhotosQueryRecord {
            uuid: "00000000-0000-0000-0000-000000000000".to_string(),
            filename: String::new(),
            original_filename: String::new(),
            uti: String::new(),
            uti_original: String::new(),
            date: None,
            date_original: None,
            path: None,
            path_derivatives: Vec::new(),
            ismissing: false,
            album_info: vec![PhotosAlbumInfo {
                uuid: "11111111-1111-1111-1111-111111111111".to_string(),
                title: "Trip/Day".to_string(),
                folder_names: vec!["Family".to_string()],
            }],
        };
        let target = target_album_paths(&record, AuditStatus::RecoveryNeeded);
        assert_eq!(
            target,
            vec![vec![
                "MFB JXL Audit".to_string(),
                "Recovery Needed".to_string(),
                "Family".to_string(),
                "Trip/Day".to_string(),
            ]]
        );
    }
}
