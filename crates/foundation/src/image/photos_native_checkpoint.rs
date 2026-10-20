//! `PhotoKit` transactions reuse the existing UUID/original-byte custody gates.
use super::{
    BTreeMap, Duration, FastImgLibraryAssetProbe, ImgQualityError, LibraryHandle, Path, PathBuf,
    PhotosImportCheckpointPlan, PhotosImportPendingEntry, Result, WorkingCopyMarker,
    checkpoint_photos_import_window, library_records_from_pending_import,
    osxphotos_uuid_from_photos_import_identifier, require_active_photos_library,
    resolve_osascript_command, run_photos_import_applescript_session_mode_in_library,
};
use crate::image::photos_import_metrics;
use crate::image::photos_import_schedule::{Schedule, Step};
use crate::image::photos_native::{Client, committed_pairs};
use crate::progress::{PhotosImportPhase, PhotosImportProgress};
use crate::unified_error::PhotosDeliveryDeferred;
use std::os::unix::fs::PermissionsExt;

const WITNESS_QUERY_CONTEXT: &str = "selected Photos library witness";
const WITNESS_ACTIVATION_CONTEXT: &str = "activate Photos for read-only witness recovery";

fn photos_command_is_transient(error: &std::io::Error, context: &str) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::Interrupted
    ) || error
        .to_string()
        .strip_prefix(context)
        .is_some_and(|detail| detail.starts_with(" timed out at hard timeout"))
}

fn photos_activation_error(error: std::io::Error) -> anyhow::Error {
    if photos_command_is_transient(&error, WITNESS_ACTIVATION_CONTEXT) {
        PhotosDeliveryDeferred {
            detail: format!("safe Photos activation temporarily unavailable: {error}; outputs and checkpoints retained"),
        }
        .into()
    } else {
        error.into()
    }
}

fn native_import_error(error: anyhow::Error, operation: &str) -> ImgQualityError {
    if error.is::<PhotosDeliveryDeferred>() {
        ImgQualityError::Other(error.context(format!("{operation}; sources and journals retained")))
    } else {
        ImgQualityError::AnalysisError(format!(
            "{operation}: {error:#}; sources and journals retained"
        ))
    }
}

fn startup_error(error: anyhow::Error, unresolved_journals: bool) -> anyhow::Error {
    if unresolved_journals && error.is::<PhotosDeliveryDeferred>() {
        anyhow::anyhow!(
            "native target unavailable with unresolved import journals; delivery state requires reconciliation: {error:#}"
        )
    } else {
        error
    }
}

fn checkpoint_with_progress<C>(
    entries: &[PhotosImportPendingEntry],
    assets: &[super::LibraryAssetRecord],
    checkpoint: &mut C,
    progress: &mut PhotosImportProgress,
) -> anyhow::Result<()>
where
    C: FnMut(&[PhotosImportPendingEntry], &[super::LibraryAssetRecord]) -> Result<()>,
{
    checkpoint(entries, assets)?;
    progress.checkpointed(entries.iter().map(|entry| entry.rel_path.as_str()))
}

fn selected_library_witness_with_recovery(
    mut query: impl FnMut(Duration) -> std::io::Result<std::process::Output>,
    mut activate: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<String> {
    // Read-only cold-start retries stop at eight minutes per query.
    let mut budget = Duration::from_secs(120);
    for attempt in 1..=3 {
        anyhow::ensure!(
            !crate::batch_control::is_cancelled()?,
            "Photos witness query cancelled; delivery deferred, checkpoints and outputs retained"
        );
        let output = query(budget);
        anyhow::ensure!(
            !crate::batch_control::is_cancelled()?,
            "Photos witness query cancelled; delivery deferred, checkpoints and outputs retained"
        );
        let error = match output {
            Ok(output) if output.status.success() => {
                let witness = String::from_utf8(output.stdout)?;
                let witness = witness.trim();
                let uuid = super::osxphotos_uuid_from_photos_import_identifier(witness)?;
                anyhow::ensure!(
                    !witness.contains(['\r', '\n'])
                        && uuid.len() == 36
                        && uuid.bytes().enumerate().all(|(index, byte)| match index {
                            8 | 13 | 18 | 23 => byte == b'-',
                            _ => byte.is_ascii_hexdigit(),
                        }),
                    "Photos witness query returned an invalid asset identifier; target cannot be proven"
                );
                if attempt > 1 {
                    tracing::info!(target: "photos_import", attempt, "Photos library witness recovered; resuming target proof before import");
                }
                return Ok(witness.to_owned());
            }
            Ok(output) => {
                let detail = String::from_utf8_lossy(&output.stderr);
                anyhow::ensure!(
                    !detail.contains("(-1743)")
                        && ["(-1712)", "(-609)", "(-600)"]
                            .iter()
                            .any(|code| detail.contains(code)),
                    "Photos witness query rejected: {detail}; target cannot be proven, sources retained"
                );
                detail.into_owned()
            }
            Err(error) if photos_command_is_transient(&error, WITNESS_QUERY_CONTEXT) => {
                error.to_string()
            }
            Err(error) => return Err(error.into()),
        };
        if attempt == 3 {
            return Err(PhotosDeliveryDeferred {
                detail: format!("after {attempt} witness attempts: {error}; verified outputs and checkpoints retained, no new import was submitted during target recovery"),
            }.into());
        }
        anyhow::ensure!(
            !crate::batch_control::is_cancelled()?,
            "Photos witness recovery cancelled; delivery deferred, outputs and checkpoints retained"
        );
        tracing::warn!(target: "photos_import", attempt, budget_secs = budget.as_secs(), %error,
            "Photos witness temporarily unavailable; activating Photos and retrying the read-only query with a larger budget");
        let recovery = activate();
        anyhow::ensure!(
            !crate::batch_control::is_cancelled()?,
            "Photos witness recovery cancelled; delivery deferred, outputs and checkpoints retained"
        );
        recovery.map_err(|recovery| recovery.context(format!(
            "Photos witness recovery failed; preceding query: {error}; verified outputs and checkpoints retained"
        )))?;
        budget *= 2;
    }
    anyhow::bail!("Photos delivery deferred; witness recovery exhausted, outputs retained")
}

fn selected_library_witness(library: Option<&Path>) -> anyhow::Result<String> {
    selected_library_witness_with_recovery(
        |budget| {
            let script = format!(
                "with timeout of {} seconds\ntell application \"Photos\" to return id of first media item\nend timeout",
                budget.as_secs()
            );
            crate::process_runner::run_command_with_liveness_timeout(
                std::process::Command::new(resolve_osascript_command()).args(["-e", &script]),
                Duration::from_secs(30),
                budget + Duration::from_secs(5),
                WITNESS_QUERY_CONTEXT,
            )
        },
        || {
            let mut open = std::process::Command::new("/usr/bin/open");
            open.args(["-a", "Photos"]);
            if let Some(library) = library {
                open.arg(library);
            }
            let output = crate::process_runner::run_command_with_liveness_timeout(
                &mut open,
                Duration::from_secs(15),
                Duration::from_secs(120),
                WITNESS_ACTIVATION_CONTEXT,
            )
            .map_err(photos_activation_error)?;
            anyhow::ensure!(
                output.status.success(),
                "Photos activation failed: {}; delivery deferred, outputs retained",
                String::from_utf8_lossy(&output.stderr)
            );
            require_active_photos_library(library)?;
            Ok(())
        },
    )
}

struct ImportTask {
    identity: String,
    expected: usize,
    relocated_originals: bool,
}

fn import_task(marker: &WorkingCopyMarker, library: Option<&Path>) -> anyhow::Result<ImportTask> {
    Ok(ImportTask {
        identity: blake3::hash(&serde_json::to_vec(&(
            &marker.src_dir,
            &marker.working_copy,
            library,
        ))?)
        .to_hex()
        .to_string(),
        expected: marker.expected_output_count(),
        relocated_originals: false,
    })
}

fn original_plan(
    marker: &WorkingCopyMarker,
    candidates: &[super::PhotosImportCandidate],
) -> Result<PhotosImportCheckpointPlan> {
    super::validate_photos_import_candidates(candidates)?;
    let mut proven_assets = marker.tier2_imported_assets.clone();
    for asset in &mut proven_assets {
        if let Some(delivery_hash) = asset.library_blake3.take() {
            asset.blake3 = delivery_hash;
        }
    }
    let mut pending_entries = Vec::new();
    for candidate in candidates {
        if let Some(proof) = proven_assets
            .iter()
            .find(|a| a.rel_path == candidate.rel_path)
        {
            if proof.blake3 != candidate.blake3 || proof.photos_uuid.is_none() {
                return Err(ImgQualityError::AnalysisError(format!(
                    "tier-2 native checkpoint identity changed for {}; source retained",
                    candidate.rel_path
                )));
            }
            continue;
        }
        pending_entries.push(PhotosImportPendingEntry {
            source_rel: candidate.rel_path.clone(),
            rel_path: candidate.rel_path.clone(),
            path: candidate.path.clone(),
            album_name: candidate.album_name.clone(),
            blake3_entry: super::Blake3Entry {
                out_rel: Some(candidate.rel_path.clone()),
                src: candidate.blake3.clone(),
                out: candidate.blake3.clone(),
                library_asset: None,
            },
        });
    }
    Ok(PhotosImportCheckpointPlan {
        pending_entries,
        proven_assets,
    })
}

pub(super) fn try_import_originals(
    marker: &mut WorkingCopyMarker,
    candidates: &[super::PhotosImportCandidate],
    enriched: &BTreeMap<String, (String, String, String)>,
    library: Option<&Path>,
) -> Result<Option<LibraryHandle>> {
    let profile = photos_import_metrics::Profile::start("native");
    let result = (|| -> anyhow::Result<Option<LibraryHandle>> {
        let plan = original_plan(marker, candidates)?;
        if let Some(proof) = super::library_handle_from_marker_tier2_proof(marker) {
            super::reverify_modern_lossy_static_photos_custody(&proof)?;
        }
        let mut task = import_task(marker, library)?;
        task.identity.push_str("-originals");
        task.expected = plan.proven_assets.len() + plan.pending_entries.len();
        task.relocated_originals = true;
        let result = try_import_inner(
            &task,
            &plan,
            &mut |ids| super::query_selected_photos_asset_probes(ids, library),
            &mut super::path_has_quarantine_xattr,
            library,
            &mut |_entries, assets| {
                let mut handle = LibraryHandle {
                    imported_assets: assets.to_vec(),
                    import_error_count: 0,
                    photos_library_path: library.map(Path::to_path_buf),
                };
                super::apply_tier2_enriched_delivery_proofs(&mut handle, enriched)?;
                super::apply_tier2_library_assets_to_marker(marker, &handle)?;
                super::write_marker_atomic(marker)?;
                photos_import_metrics::verified(assets.len());
                Ok(())
            },
        )?;
        Ok(result.map(|mut handle| {
            handle
                .imported_assets
                .retain(|asset| candidates.iter().any(|c| c.rel_path == asset.rel_path));
            handle
        }))
    })();
    if result
        .as_ref()
        .err()
        .is_some_and(anyhow::Error::is::<PhotosDeliveryDeferred>)
    {
        profile.deferred();
    } else if !matches!(result, Ok(None)) {
        profile.report(result.is_ok());
    }
    result.map_err(|error| native_import_error(error, "PhotoKit original import"))
}

fn resource(entry: &PhotosImportPendingEntry) -> anyhow::Result<serde_json::Value> {
    let kind = if matches!(
        entry
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mp4" | "mov" | "m4v" | "mkv" | "webm")
    ) {
        match crate::image::format_detect::detect_true_format(&entry.path)? {
            crate::image::format_detect::FormatKind::Mp4
            | crate::image::format_detect::FormatKind::Mov
            | crate::image::format_detect::FormatKind::Mkv
            | crate::image::format_detect::FormatKind::Webm => "video",
            _ => anyhow::bail!(
                "video output extension/content mismatch: {}",
                entry.path.display()
            ),
        }
    } else {
        "photo"
    };
    Ok(
        serde_json::json!({"path":entry.path,"kind":kind,"blake3":entry.blake3_entry.out,
        "originalFilename":entry.path.file_name().and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("PhotoKit original filename is not UTF-8"))?}),
    )
}

pub(super) fn candidate_marker(
    source: &Path,
    identity: &impl serde::Serialize,
    library: Option<&Path>,
) -> Result<WorkingCopyMarker> {
    let task = (|| -> anyhow::Result<WorkingCopyMarker> {
        let key = blake3::hash(&serde_json::to_vec(&(source, identity, library))?)
            .to_hex()
            .to_string();
        let directory = crate::process_lock::get_mfb_root()?
            .join("photos-original-checkpoints")
            .join(key);
        std::fs::create_dir_all(&directory)?;
        anyhow::ensure!(
            !std::fs::symlink_metadata(&directory)?
                .file_type()
                .is_symlink(),
            "original checkpoint directory is a symlink"
        );
        let mut marker = match crate::pipeline::verification::read_marker(&directory) {
            Ok(marker) => marker,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                WorkingCopyMarker::new(source.to_path_buf(), directory, 0)
            }
            Err(error) => return Err(error.into()),
        };
        super::bind_photos_library_proof(&mut marker, library)?;
        super::bind_photos_import_naming_policy(&mut marker)?;
        super::write_marker_atomic(&marker)?;
        Ok(marker)
    })();
    task.map_err(|error| ImgQualityError::AnalysisError(format!("original checkpoint: {error:#}")))
}

pub(super) fn try_import_candidates(
    candidates: &[super::PhotosImportCandidate],
    library: Option<&Path>,
) -> Result<Option<LibraryHandle>> {
    super::validate_photos_import_candidates(candidates)?;
    let mut groups: BTreeMap<(PathBuf, String), Vec<super::PhotosImportCandidate>> =
        BTreeMap::new();
    for candidate in candidates {
        let source = candidate
            .path
            .parent()
            .ok_or_else(|| ImgQualityError::AnalysisError("original has no parent".into()))?;
        groups
            .entry((source.to_path_buf(), candidate.album_name.clone()))
            .or_default()
            .push(candidate.clone());
    }
    let mut imported = Vec::new();
    for ((source, album), candidates) in groups {
        let mut marker = candidate_marker(&source, &("media-output", album), library)?;
        let Some(mut handle) =
            try_import_originals(&mut marker, &candidates, &BTreeMap::new(), library)?
        else {
            if !imported.is_empty() {
                return Err(ImgQualityError::AnalysisError("native backend became unavailable after verified imports; receipts retained, no backend switch".into()));
            }
            return Ok(None);
        };
        imported.append(&mut handle.imported_assets);
    }
    Ok(Some(LibraryHandle {
        imported_assets: imported,
        import_error_count: 0,
        photos_library_path: library.map(Path::to_path_buf),
    }))
}

fn resource_matches(
    recorded: &serde_json::Value,
    entry: &PhotosImportPendingEntry,
    allow_relocation: bool,
) -> anyhow::Result<bool> {
    let mut expected = serde_json::json!([resource(entry)?]);
    let mut recorded = recorded.clone();
    if allow_relocation {
        // XMP staging directories are ephemeral. Content, filename and resource
        // kind remain bound to the durable intent when only the staging path moves.
        for value in [&mut expected, &mut recorded] {
            if let Some(resources) = value.as_array_mut() {
                for resource in resources {
                    if let Some(object) = resource.as_object_mut() {
                        object.remove("path");
                    }
                }
            }
        }
    }
    Ok(recorded == expected)
}

fn helper_app() -> anyhow::Result<Option<PathBuf>> {
    if let Some(path) = std::env::var_os("MFB_PHOTOS_NATIVE_HELPER_APP") {
        return Ok(Some(PathBuf::from(path)));
    }
    let binary = std::env::current_exe()?;
    Ok(binary.parent().and_then(Path::parent).and_then(|contents| {
        let app = contents.join("Helpers/MFB Photos Import.app");
        app.is_dir().then_some(app)
    }))
}

fn import_request(
    entries: &[PhotosImportPendingEntry],
    batch_index: usize,
    albums: &BTreeMap<String, &str>,
    witnesses: &[String],
    journals: &Path,
) -> anyhow::Result<serde_json::Value> {
    for entry in entries {
        anyhow::ensure!(
            crate::common_utils::calculate_blake3_hash(&entry.path)? == entry.blake3_entry.out,
            "original changed before native import: {}",
            entry.rel_path
        );
    }
    let batch_id = format!("batch-{batch_index}");
    let assets = entries
        .iter()
        .map(|entry| {
            Ok(serde_json::json!({"entryID":entry.rel_path,
                "resources":[resource(entry)?],"albumIdentifier":albums[&entry.album_name]}))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(
        serde_json::json!({"version":1,"operation":"import","batchID":batch_id,
        "witnessIdentifiers":witnesses,"assets":assets,"journalPath":journals.join(format!("{batch_id}.json"))}),
    )
}

fn finish_import(
    client: &mut Client,
    request: &serde_json::Value,
    identifiers: &mut BTreeMap<String, String>,
    library: Option<&Path>,
) -> anyhow::Result<()> {
    let reply = client.finish_request()?;
    let pairs = committed_pairs(request, &reply)?;
    photos_import_metrics::swift_transaction(&reply)?;
    require_active_photos_library(library)?;
    for (entry, identifier) in pairs {
        anyhow::ensure!(
            identifiers.insert(entry, identifier).is_none(),
            "duplicate queued native entry"
        );
    }
    let count = request["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing native request assets"))?
        .len();
    photos_import_metrics::committed(count, identifiers.len());
    Ok(())
}

pub(super) fn try_import<Q, P>(
    marker: &mut WorkingCopyMarker,
    plan: &PhotosImportCheckpointPlan,
    query: &mut Q,
    quarantined: &mut P,
    library: Option<&Path>,
) -> Result<Option<LibraryHandle>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    let profile = photos_import_metrics::Profile::start("native");
    let result = import_task(marker, library).and_then(|task| {
        try_import_inner(
            &task,
            plan,
            query,
            quarantined,
            library,
            &mut |entries, assets| checkpoint_photos_import_window(marker, entries, assets),
        )
    });
    if result
        .as_ref()
        .err()
        .is_some_and(anyhow::Error::is::<PhotosDeliveryDeferred>)
    {
        profile.deferred();
    } else if !matches!(result, Ok(None)) {
        profile.report(result.is_ok());
    }
    result.map_err(|error| native_import_error(error, "PhotoKit import"))
}

fn try_import_inner<Q, P, C>(
    task: &ImportTask,
    plan: &PhotosImportCheckpointPlan,
    query: &mut Q,
    quarantined: &mut P,
    library: Option<&Path>,
    checkpoint: &mut C,
) -> anyhow::Result<Option<LibraryHandle>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
    C: FnMut(&[PhotosImportPendingEntry], &[super::LibraryAssetRecord]) -> Result<()>,
{
    let backend = if let Some(config) = crate::infra::runtime_config::active() {
        match config.photos.backend {
            crate::infra::runtime_config::PhotosBackend::Auto => "auto".to_string(),
            crate::infra::runtime_config::PhotosBackend::Native => "photokit".to_string(),
            crate::infra::runtime_config::PhotosBackend::Applescript => "applescript".to_string(),
        }
    } else {
        std::env::var("MFB_PHOTOS_IMPORT_BACKEND").unwrap_or_else(|_| "auto".into())
    };
    anyhow::ensure!(
        ["auto", "photokit", "applescript"].contains(&backend.as_str()),
        "invalid MFB_PHOTOS_IMPORT_BACKEND"
    );
    let state = crate::process_lock::get_mfb_root()?.join("photos-native");
    let journals = state.join(&task.identity);
    let mut journal_files = match std::fs::read_dir(&journals) {
        Ok(entries) => entries
            .map(|entry| entry.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    journal_files.sort();
    if backend == "applescript" {
        anyhow::ensure!(
            journal_files.is_empty() || plan.pending_entries.is_empty(),
            "native journals require native reconciliation before switching backend"
        );
        return Ok(None);
    }
    if plan.pending_entries.is_empty() && journal_files.is_empty() {
        return Ok(Some(LibraryHandle {
            imported_assets: plan.proven_assets.clone(),
            import_error_count: 0,
            photos_library_path: library.map(Path::to_path_buf),
        }));
    }
    let Some(app) = helper_app()? else {
        anyhow::ensure!(
            backend == "auto" && journal_files.is_empty(),
            "native helper app required for import/reconciliation"
        );
        return Ok(None);
    };
    let mut progress = PhotosImportProgress::new(
        plan.pending_entries
            .iter()
            .map(|entry| entry.rel_path.as_str()),
    )?;
    require_active_photos_library(library)?;
    let startup = (|| -> anyhow::Result<(Client, Vec<String>)> {
        let witnesses = vec![selected_library_witness(library)?];
        let client = Client::start(&app, &state, &witnesses)?;
        require_active_photos_library(library)?;
        Ok((client, witnesses))
    })();
    anyhow::ensure!(
        !crate::batch_control::is_cancelled()?,
        "Photos startup cancelled; delivery deferred, outputs and checkpoints retained"
    );
    let (mut client, witnesses) = match startup {
        Ok(ready) => ready,
        Err(error) if backend == "auto" && journal_files.is_empty() => {
            tracing::warn!(target:"photos_import", %error, "PhotoKit unavailable before any import intent; using AppleScript compatibility backend");
            return Ok(None);
        }
        Err(error) => return Err(startup_error(error, !journal_files.is_empty())),
    };
    std::fs::create_dir_all(&journals)?;
    anyhow::ensure!(
        !std::fs::symlink_metadata(&journals)?
            .file_type()
            .is_symlink(),
        "journal directory is a symlink"
    );
    std::fs::set_permissions(&journals, std::fs::Permissions::from_mode(0o700))?;
    let mut pending = plan
        .pending_entries
        .iter()
        .map(|entry| (entry.rel_path.clone(), entry.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut imported = plan.proven_assets.clone();
    progress.phase(PhotosImportPhase::Reconciling)?;
    // Reconcile ALL old intents before submitting anything new, including when
    // the batch size changed between runs. Filename/content dedup is never used.
    for file in &journal_files {
        let metadata = std::fs::symlink_metadata(file)?;
        anyhow::ensure!(
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= 8 * 1024 * 1024,
            "invalid native journal file"
        );
        let record: serde_json::Value = serde_json::from_slice(&std::fs::read(file)?)?;
        let request = &record["request"];
        anyhow::ensure!(
            request["journalPath"] == serde_json::json!(file),
            "journal location binding changed"
        );
        let pairs = committed_pairs(request, &record)?;
        let mut recovered_entries = Vec::new();
        let mut recovered_pairs = Vec::new();
        for (entry_id, identifier) in pairs {
            if let Some(entry) = pending.get(&entry_id) {
                let asset = request["assets"]
                    .as_array()
                    .and_then(|assets| assets.iter().find(|asset| asset["entryID"] == entry_id))
                    .ok_or_else(|| {
                        anyhow::anyhow!("journal is missing requested asset {entry_id}")
                    })?;
                anyhow::ensure!(
                    resource_matches(&asset["resources"], entry, task.relocated_originals)?,
                    "journal source/hash binding changed"
                );
                recovered_entries.push(entry.clone());
                recovered_pairs.push((entry_id, identifier));
            } else {
                let uuid = osxphotos_uuid_from_photos_import_identifier(&identifier)?;
                anyhow::ensure!(
                    imported.iter().any(|asset| asset.rel_path == entry_id
                        && asset.photos_uuid.as_deref() == Some(uuid)),
                    "old intent does not belong to a current pending or checkpointed asset"
                );
            }
        }
        if !recovered_entries.is_empty() {
            let _transaction = crate::batch_control::begin_transaction()?;
            let mut proofs = library_records_from_pending_import(
                &recovered_entries,
                &recovered_pairs,
                query,
                quarantined,
            )?;
            checkpoint_with_progress(&recovered_entries, &proofs, checkpoint, &mut progress)?;
            for entry in recovered_entries {
                pending.remove(&entry.rel_path);
            }
            imported.append(&mut proofs);
        }
    }
    let pending = pending.into_values().collect::<Vec<_>>();
    if pending.is_empty() {
        anyhow::ensure!(
            imported.len() == task.expected,
            "native verified asset count mismatch"
        );
        imported.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
        progress.complete()?;
        return Ok(Some(LibraryHandle {
            imported_assets: imported,
            import_error_count: 0,
            photos_library_path: library.map(Path::to_path_buf),
        }));
    }
    progress.phase(PhotosImportPhase::TargetPreparation)?;
    let albums = pending
        .iter()
        .map(|entry| (entry.album_name.clone(), entry.path.clone()))
        .collect::<BTreeMap<_, _>>();
    let album_entries = albums
        .iter()
        .map(|(album, path)| (path.clone(), album.clone()))
        .collect::<Vec<_>>();
    let album_reply = run_photos_import_applescript_session_mode_in_library(
        "native album preparation",
        &album_entries,
        "prepare_albums",
        library,
    )?;
    let album_ids = album_reply.lines().map(str::trim).collect::<Vec<_>>();
    anyhow::ensure!(
        album_ids.len() == albums.len() && album_ids.iter().all(|id| !id.is_empty()),
        "Photos album preparation incomplete"
    );
    let albums = albums
        .into_keys()
        .zip(album_ids)
        .collect::<BTreeMap<_, _>>();
    let mut policy = crate::infra::runtime_config::active()
        .map_or_else(crate::runtime_config::PhotosPolicy::default, |config| {
            config.photos.clone()
        });
    if crate::infra::runtime_config::active().is_none() {
        policy.native_batch_size = std::env::var("MFB_PHOTOS_NATIVE_BATCH_SIZE")
            .map_or(Ok(100), |value| value.parse::<usize>())?;
    }
    let mut schedule = Schedule::new(pending.len(), &policy)?;
    photos_import_metrics::batch_sizes(policy.native_batch_size, policy.verification_batch_size);
    let mut identifiers: BTreeMap<String, String> = BTreeMap::new();
    let mut batch_index = journal_files.len();
    let mut window_started = std::time::Instant::now();
    let mut step = schedule.next();
    while let Some(current) = step {
        let _transaction = crate::batch_control::begin_transaction()?;
        require_active_photos_library(library)?;
        match current {
            Step::Verify(range) => {
                let entries = &pending[range];
                progress.phase(PhotosImportPhase::Verifying)?;
                // A single helper write can run while this window is verified and checkpointed.
                let next = schedule.next();
                let prefetch = if let Some(Step::Import(import_range)) = &next {
                    let request = import_request(
                        &pending[import_range.clone()],
                        batch_index,
                        &albums,
                        &witnesses,
                        &journals,
                    )?;
                    client.begin_request(&request)?;
                    batch_index += 1;
                    Some(request)
                } else {
                    None
                };
                let verification = (|| -> anyhow::Result<()> {
                    let _timing = photos_import_metrics::timer("verification_window");
                    let pairs = entries
                        .iter()
                        .map(|entry| {
                            let identifier = identifiers.get(&entry.rel_path).ok_or_else(|| {
                                anyhow::anyhow!(
                                    "missing committed identifier for {}",
                                    entry.rel_path
                                )
                            })?;
                            Ok((entry.rel_path.clone(), identifier.clone()))
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    let mut proofs =
                        library_records_from_pending_import(entries, &pairs, query, quarantined)?;
                    checkpoint_with_progress(entries, &proofs, checkpoint, &mut progress)?;
                    for entry in entries {
                        identifiers.remove(&entry.rel_path);
                    }
                    imported.append(&mut proofs);
                    if identifiers.is_empty() {
                        let pressure = matches!(
                            crate::system_memory::memory_pressure_level(),
                            Some(crate::system_memory::MemoryPressure::High)
                        );
                        if let Some((previous, next, reason)) =
                            schedule.observe(window_started.elapsed(), pressure)
                        {
                            photos_import_metrics::batch_sizes(
                                next,
                                policy.verification_batch_size,
                            );
                            tracing::info!(target: "photos_import", previous, next, reason, "Native import batch adjusted after verified window");
                        }
                        window_started = std::time::Instant::now();
                    }
                    Ok(())
                })();
                // Drain the submitted request even when verification fails. The helper's
                // durable journal is the recovery authority; never replay this batch here.
                let imported_next = prefetch
                    .as_ref()
                    .map(|request| finish_import(&mut client, request, &mut identifiers, library))
                    .transpose();
                match (verification, imported_next) {
                    (Err(verify), Err(import)) => anyhow::bail!(
                        "verification failed: {verify:#}; prefetched import also failed: {import:#}"
                    ),
                    (Err(verify), _) => return Err(verify),
                    (_, Err(import)) => return Err(import),
                    (Ok(()), Ok(_)) => {}
                }
                step = if prefetch.is_some() {
                    schedule.next()
                } else {
                    next
                };
            }
            Step::Import(range) => {
                progress.phase(PhotosImportPhase::Importing)?;
                let request =
                    import_request(&pending[range], batch_index, &albums, &witnesses, &journals)?;
                client.begin_request(&request)?;
                batch_index += 1;
                finish_import(&mut client, &request, &mut identifiers, library)?;
                step = schedule.next();
            }
        }
    }
    anyhow::ensure!(
        identifiers.is_empty(),
        "unverified native identifiers remain queued"
    );
    anyhow::ensure!(
        imported.len() == task.expected,
        "native verified asset count mismatch"
    );
    imported.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    progress.complete()?;
    Ok(Some(LibraryHandle {
        imported_assets: imported,
        import_error_count: 0,
        photos_library_path: library.map(Path::to_path_buf),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;

    #[test]
    fn photos_progress_advances_only_after_checkpoint_persistence() -> anyhow::Result<()> {
        let entries = [PhotosImportPendingEntry {
            source_rel: "image.jpg".into(),
            rel_path: "image.jxl".into(),
            path: PathBuf::from("/synthetic/image.jxl"),
            album_name: "Synthetic".into(),
            blake3_entry: super::super::Blake3Entry::default(),
        }];
        let mut progress =
            PhotosImportProgress::new(entries.iter().map(|entry| entry.rel_path.as_str()))?;
        assert!(
            checkpoint_with_progress(
                &entries,
                &[],
                &mut |_, _| {
                    Err(ImgQualityError::AnalysisError(
                        "synthetic checkpoint write failure".into(),
                    ))
                },
                &mut progress
            )
            .is_err()
        );
        assert!(progress.complete().is_err());
        checkpoint_with_progress(&entries, &[], &mut |_, _| Ok(()), &mut progress)?;
        progress.complete()?;
        Ok(())
    }
    fn witness_reply(code: i32, text: &str) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: if code == 0 {
                text.as_bytes().to_vec()
            } else {
                Vec::new()
            },
            stderr: if code == 0 {
                Vec::new()
            } else {
                text.as_bytes().to_vec()
            },
        }
    }

    #[test]
    fn witness_recovery_expands_budget_and_requires_a_real_identifier() -> anyhow::Result<()> {
        let mut budgets = Vec::new();
        let mut activations = 0;
        let identifier = "01234567-89AB-CDEF-0123-456789ABCDEF/L0/001";
        let result = selected_library_witness_with_recovery(
            |budget| {
                budgets.push(budget.as_secs());
                match budgets.len() {
                    1 => Err(std::io::Error::other(
                        "selected Photos library witness timed out at hard timeout after 30s",
                    )),
                    2 => Ok(witness_reply(1, "Photos connection invalid (-609)")),
                    _ => Ok(witness_reply(0, identifier)),
                }
            },
            || {
                activations += 1;
                Ok(())
            },
        )?;
        assert_eq!(result, identifier);
        assert_eq!(budgets, [120, 240, 480]);
        assert_eq!(activations, 2);
        Ok(())
    }

    #[test]
    fn typed_deferral_does_not_cover_permanent_errors_or_unresolved_imports() {
        let deferred = || {
            anyhow::Error::new(PhotosDeliveryDeferred {
                detail: "temporary target timeout".into(),
            })
        };
        assert!(startup_error(deferred(), false).is::<PhotosDeliveryDeferred>());
        assert!(!startup_error(deferred(), true).is::<PhotosDeliveryDeferred>());
        let wrapped = anyhow::Error::new(native_import_error(deferred(), "import"));
        assert!(
            wrapped
                .chain()
                .any(<dyn std::error::Error>::is::<PhotosDeliveryDeferred>)
        );
        for permanent in [
            "Not authorized (-1743)",
            "selected library changed",
            "cancelled",
        ] {
            let error = native_import_error(anyhow::anyhow!(permanent.to_owned()), "import");
            assert!(
                !anyhow::Error::new(error)
                    .chain()
                    .any(<dyn std::error::Error>::is::<PhotosDeliveryDeferred>)
            );
        }
    }

    #[test]
    fn witness_recovery_allows_a_live_query_past_the_soft_deadline() -> anyhow::Result<()> {
        let result = selected_library_witness_with_recovery(
            |_| {
                crate::process_runner::run_command_with_liveness_timeout(
                    std::process::Command::new("/bin/sh").args([
                        "-c",
                        "sleep 0.05; printf '01234567-89AB-CDEF-0123-456789ABCDEF/L0/001'",
                    ]),
                    Duration::from_millis(1),
                    Duration::from_secs(2),
                    WITNESS_QUERY_CONTEXT,
                )
            },
            || panic!("a slow successful query must not restart Photos"),
        )?;
        assert_eq!(result, "01234567-89AB-CDEF-0123-456789ABCDEF/L0/001");
        Ok(())
    }

    #[test]
    fn witness_recovery_defers_after_bounded_attempts_or_failed_activation() {
        let mut queries = 0;
        let mut activations = 0;
        let error = selected_library_witness_with_recovery(
            |_| {
                queries += 1;
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "synthetic timeout",
                ))
            },
            || {
                activations += 1;
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!((queries, activations), (3, 2));
        assert!(error.is::<PhotosDeliveryDeferred>());
        assert!(error.to_string().contains("delivery deferred"));
        assert!(error.to_string().contains("checkpoints retained"));
        let mut queries = 0;
        let error = selected_library_witness_with_recovery(
            |_| {
                queries += 1;
                Ok(witness_reply(1, "AppleEvent timed out (-1712)"))
            },
            || anyhow::bail!("selected library changed"),
        )
        .unwrap_err();
        assert_eq!(queries, 1);
        assert!(!error.is::<PhotosDeliveryDeferred>());
        assert!(format!("{error:#}").contains("selected library changed"));
        assert!(format!("{error:#}").contains("(-1712)"));
    }

    #[test]
    fn witness_recovery_does_not_retry_permissions_or_invalid_evidence() {
        for (code, text) in [
            (1, "Not authorized (-1743)"),
            (1, "Not authorized (-1743), previous timeout (-1712)"),
            (1, "No first media item (-1728)"),
            (0, ""),
            (0, "missing value"),
            (0, "first\nsecond"),
        ] {
            let mut queries = 0;
            let result = selected_library_witness_with_recovery(
                |_| {
                    queries += 1;
                    Ok(witness_reply(code, text))
                },
                || panic!("permanent errors must not activate or retry Photos"),
            );
            assert!(result.is_err(), "invalid evidence was accepted: {text}");
            assert_eq!(queries, 1);
        }
    }

    #[test]
    fn witness_recovery_preserves_cancellation() -> anyhow::Result<()> {
        let Some(control) = std::env::var_os("MFB_WITNESS_CANCELLATION_TEST_CHILD") else {
            let root = tempfile::tempdir()?;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
            let control = root.path().join("control");
            std::fs::write(&control, "running\ntest-token")?;
            let output = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "image::fast_img::native_checkpoint::tests::witness_recovery_preserves_cancellation",
                    "--nocapture",
                ])
                .env("MFB_WITNESS_CANCELLATION_TEST_CHILD", &control)
                .env("MFB_BATCH_CONTROL_FILE", &control)
                .env("MFB_HOME_ROOT", root.path())
                .env("MFB_PHOTOS_IMPORT_BACKEND", "auto")
                .env("MFB_PHOTOS_NATIVE_HELPER_APP", root.path().join("unused.app"))
                .output()?;
            anyhow::ensure!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
                "isolated cancellation check failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        };
        let result = selected_library_witness_with_recovery(
            |_| {
                std::fs::write(&control, "cancelled\ntest-token")?;
                Ok(witness_reply(
                    0,
                    "01234567-89AB-CDEF-0123-456789ABCDEF/L0/001",
                ))
            },
            || panic!("cancelled queries must not activate Photos"),
        );
        let error = result.unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error:#}");
        let entry = PhotosImportPendingEntry {
            source_rel: "image.jxl".into(),
            rel_path: "image.jxl".into(),
            path: PathBuf::from("unused.jxl"),
            album_name: "Test".into(),
            blake3_entry: super::super::Blake3Entry::default(),
        };
        let result = try_import_inner(
            &ImportTask {
                identity: "cancelled-test".into(),
                expected: 1,
                relocated_originals: false,
            },
            &PhotosImportCheckpointPlan {
                pending_entries: vec![entry],
                proven_assets: Vec::new(),
            },
            &mut |_| panic!("cancelled startup must not query assets"),
            &mut |_| panic!("cancelled startup must not inspect imports"),
            None,
            &mut |_, _| panic!("cancelled startup must not import"),
        );
        let error = result.unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error:#}");
        Ok(())
    }

    #[test]
    fn originals_reuse_exact_receipts_and_reject_changed_delivery() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("original.avif");
        std::fs::write(&path, b"synthetic-original")?;
        let hash = crate::common_utils::calculate_blake3_hash(&path)?;
        let candidate = super::super::PhotosImportCandidate {
            rel_path: "original.avif".into(),
            path,
            blake3: hash.clone(),
            album_name: "Test".into(),
        };
        let mut marker =
            WorkingCopyMarker::new(root.path().join("source"), root.path().join("output"), 0);
        assert_eq!(
            original_plan(&marker, std::slice::from_ref(&candidate))?
                .pending_entries
                .len(),
            1
        );
        marker
            .tier2_imported_assets
            .push(super::super::LibraryAssetRecord {
                rel_path: candidate.rel_path.clone(),
                blake3: "before-xmp".into(),
                library_blake3: Some(hash.clone()),
                photos_uuid: Some("verified-uuid".into()),
                ..Default::default()
            });
        let plan = original_plan(&marker, std::slice::from_ref(&candidate))?;
        assert!(plan.pending_entries.is_empty());
        assert_eq!(plan.proven_assets[0].blake3, hash);
        marker.tier2_imported_assets[0].library_blake3 = Some("changed".into());
        assert!(original_plan(&marker, &[candidate]).is_err());
        Ok(())
    }

    #[test]
    fn relocated_original_journal_still_binds_bytes_and_filename() -> anyhow::Result<()> {
        let entry = PhotosImportPendingEntry {
            source_rel: "image.avif".into(),
            rel_path: "image.avif".into(),
            path: PathBuf::from("/staging/new/image.avif"),
            album_name: "Test".into(),
            blake3_entry: super::super::Blake3Entry {
                out: "verified-hash".into(),
                ..Default::default()
            },
        };
        let mut recorded = serde_json::json!([resource(&entry)?]);
        recorded[0]["path"] = serde_json::json!("/staging/old/image.avif");
        assert!(resource_matches(&recorded, &entry, true)?);
        assert!(!resource_matches(&recorded, &entry, false)?);
        recorded[0]["blake3"] = serde_json::json!("different-hash");
        assert!(!resource_matches(&recorded, &entry, true)?);
        recorded[0]["blake3"] = serde_json::json!("verified-hash");
        recorded[0]["originalFilename"] = serde_json::json!("different.avif");
        assert!(!resource_matches(&recorded, &entry, true)?);
        Ok(())
    }
}
