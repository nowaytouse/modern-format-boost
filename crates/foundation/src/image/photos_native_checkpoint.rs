//! `PhotoKit` transactions reuse the existing UUID/original-byte custody gates.
use super::{
    BTreeMap, Duration, FastImgLibraryAssetProbe, ImgQualityError, LibraryHandle, Path, PathBuf,
    PhotosImportCheckpointPlan, PhotosImportPendingEntry, Result, WorkingCopyMarker,
    checkpoint_photos_import_window, library_records_from_pending_import,
    osxphotos_uuid_from_photos_import_identifier, require_active_photos_library,
    resolve_osascript_command, run_fast_img_command_with_timeout,
    run_photos_import_applescript_session_mode_in_library,
};
use crate::image::photos_native::{Client, committed_pairs};
use std::os::unix::fs::PermissionsExt;

fn resource(entry: &PhotosImportPendingEntry) -> anyhow::Result<serde_json::Value> {
    Ok(
        serde_json::json!({"path":entry.path,"kind":"photo","blake3":entry.blake3_entry.out,
        "originalFilename":entry.path.file_name().and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("PhotoKit original filename is not UTF-8"))?}),
    )
}

fn helper_app() -> Option<PathBuf> {
    std::env::var_os("MFB_PHOTOS_NATIVE_HELPER_APP")
        .map(PathBuf::from)
        .or_else(|| {
            let binary = std::env::current_exe().ok()?;
            let contents = binary.parent()?.parent()?;
            let app = contents.join("Helpers/MFB Photos Import.app");
            app.is_dir().then_some(app)
        })
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
    try_import_inner(marker, plan, query, quarantined, library).map_err(|error| {
        ImgQualityError::AnalysisError(format!(
            "PhotoKit import: {error:#}; sources and journals retained"
        ))
    })
}

fn try_import_inner<Q, P>(
    marker: &mut WorkingCopyMarker,
    plan: &PhotosImportCheckpointPlan,
    query: &mut Q,
    quarantined: &mut P,
    library: Option<&Path>,
) -> anyhow::Result<Option<LibraryHandle>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
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
    let task = blake3::hash(&serde_json::to_vec(&(
        &marker.src_dir,
        &marker.working_copy,
        library,
    ))?)
    .to_hex()
    .to_string();
    let journals = state.join(task);
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
    let Some(app) = helper_app() else {
        anyhow::ensure!(
            backend == "auto" && journal_files.is_empty(),
            "native helper app required for import/reconciliation"
        );
        return Ok(None);
    };
    require_active_photos_library(library)?;
    let startup = (|| -> anyhow::Result<(Client, Vec<String>)> {
        let witness = run_fast_img_command_with_timeout(
            std::process::Command::new(resolve_osascript_command()).args([
                "-e",
                "tell application \"Photos\" to return id of first media item",
            ]),
            Duration::from_secs(30),
            "selected Photos library witness",
        )?;
        anyhow::ensure!(
            witness.status.success(),
            "selected library has no visible witness asset; native target cannot be proven"
        );
        let witnesses = vec![String::from_utf8(witness.stdout)?.trim().to_owned()];
        let client = Client::start(&app, &state, &witnesses)?;
        require_active_photos_library(library)?;
        Ok((client, witnesses))
    })();
    let (mut client, witnesses) = match startup {
        Ok(ready) => ready,
        Err(error) if backend == "auto" && journal_files.is_empty() => {
            tracing::warn!(target:"photos_import", %error, "PhotoKit unavailable before any import intent; using AppleScript compatibility backend");
            return Ok(None);
        }
        Err(error) => return Err(error),
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
                    asset["resources"] == serde_json::json!([resource(entry)?]),
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
            checkpoint_photos_import_window(marker, &recovered_entries, &proofs)?;
            for entry in recovered_entries {
                pending.remove(&entry.rel_path);
            }
            imported.append(&mut proofs);
        }
    }
    let pending = pending.into_values().collect::<Vec<_>>();
    if pending.is_empty() {
        anyhow::ensure!(
            imported.len() == marker.expected_output_count(),
            "native verified asset count mismatch"
        );
        imported.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
        return Ok(Some(LibraryHandle {
            imported_assets: imported,
            import_error_count: 0,
            photos_library_path: library.map(Path::to_path_buf),
        }));
    }
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
    let size = if let Some(config) = crate::infra::runtime_config::active() {
        config.photos.native_batch_size
    } else {
        std::env::var("MFB_PHOTOS_NATIVE_BATCH_SIZE")
            .map_or(Ok(100), |value| value.parse::<usize>())?
    };
    anyhow::ensure!(
        (1..=1000).contains(&size),
        "native batch size must be 1...1000"
    );
    for (index, entries) in pending.chunks(size).enumerate() {
        let _transaction = crate::batch_control::begin_transaction()?;
        require_active_photos_library(library)?;
        for entry in entries {
            anyhow::ensure!(
                crate::common_utils::calculate_blake3_hash(&entry.path)? == entry.blake3_entry.out,
                "original changed before native import: {}",
                entry.rel_path
            );
        }
        let batch_id = format!("batch-{}", journal_files.len() + index);
        let assets = entries
            .iter()
            .map(|entry| {
                Ok(serde_json::json!({"entryID":entry.rel_path,
            "resources":[resource(entry)?],"albumIdentifier":albums[&entry.album_name]}))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let request = serde_json::json!({"version":1,"operation":"import","batchID":batch_id,
            "witnessIdentifiers":witnesses,"assets":assets,"journalPath":journals.join(format!("{batch_id}.json"))});
        let _timing = super::super::photos_import_metrics::timer("native_import_and_verification");
        let reply = client.request(&request)?;
        let pairs = committed_pairs(&request, &reply)?;
        require_active_photos_library(library)?;
        let mut proofs = library_records_from_pending_import(entries, &pairs, query, quarantined)?;
        checkpoint_photos_import_window(marker, entries, &proofs)?;
        imported.append(&mut proofs);
    }
    anyhow::ensure!(
        imported.len() == marker.expected_output_count(),
        "native verified asset count mismatch"
    );
    imported.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(Some(LibraryHandle {
        imported_assets: imported,
        import_error_count: 0,
        photos_library_path: library.map(Path::to_path_buf),
    }))
}
