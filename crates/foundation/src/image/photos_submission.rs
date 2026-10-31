//! Durable references to import requests, not proof that Photos committed them.
//! The native journal and fresh original-resource verification remain authoritative.

use super::{BTreeMap, PhotosImportPendingEntry, WorkingCopyMarker, write_marker_atomic};
use crate::pipeline::verification::{
    PhotosSubmissionBackend, PhotosSubmissionIntent, PhotosSubmissionKind,
};
use crate::unified_error::{ImgQualityError, Result};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn checkpoint_with(
    marker: &mut WorkingCopyMarker,
    intents: &[PhotosSubmissionIntent],
    persist: impl FnOnce(&WorkingCopyMarker) -> std::io::Result<()>,
) -> Result<()> {
    // A failed write must leave both the old checkpoint and the caller's
    // in-memory authority unchanged; no import may be sent on that failure.
    let mut next = marker.clone();
    next.record_photos_submissions(intents)
        .map_err(ImgQualityError::AnalysisError)?;
    persist(&next).map_err(|error| {
        let context = intents.first().map_or_else(String::new, |intent| {
            format!(
                " (source={} output={})",
                intent.source_rel, intent.output_rel
            )
        });
        ImgQualityError::AnalysisError(format!(
            "Photos submission identity could not be persisted{context}; this media import was not sent: {error}"
        ))
    })?;
    *marker = next;
    Ok(())
}

fn intents_for_entries(
    marker: &WorkingCopyMarker,
    entries: &[PhotosImportPendingEntry],
    kind: PhotosSubmissionKind,
    backend: &PhotosSubmissionBackend,
    enriched: Option<&BTreeMap<String, (String, String, String)>>,
) -> Result<Vec<PhotosSubmissionIntent>> {
    entries
        .iter()
        .map(|entry| {
            let source_blake3 = if let Some((source, delivered, _)) =
                enriched.and_then(|hashes| hashes.get(&entry.source_rel))
            {
                if delivered != &entry.blake3_entry.out {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "Photos submission XMP delivery identity differs for {}",
                        entry.source_rel
                    )));
                }
                source.clone()
            } else {
                entry.blake3_entry.src.clone()
            };
            Ok(PhotosSubmissionIntent {
                kind,
                source_rel: entry.source_rel.clone(),
                source_blake3,
                output_rel: entry.rel_path.clone(),
                output_blake3: entry.blake3_entry.out.clone(),
                album_name: entry.album_name.clone(),
                photos_library_path: marker.photos_library_path.clone(),
                backend: backend.clone(),
            })
        })
        .collect()
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn record_native(
    marker: &mut WorkingCopyMarker,
    entries: &[PhotosImportPendingEntry],
    request: &serde_json::Value,
    kind: PhotosSubmissionKind,
    enriched: Option<&BTreeMap<String, (String, String, String)>>,
) -> Result<()> {
    let build = || -> anyhow::Result<Vec<PhotosSubmissionIntent>> {
        anyhow::ensure!(
            request["version"] == 1 && request["operation"] == "import",
            "unsupported native Photos submission request"
        );
        let assets = request["assets"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("native submission is missing assets"))?;
        anyhow::ensure!(
            !entries.is_empty() && assets.len() == entries.len(),
            "native submission inventory differs from its checkpoint entries"
        );
        let mut remaining = entries
            .iter()
            .map(|entry| (entry.rel_path.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        anyhow::ensure!(
            remaining.len() == entries.len(),
            "duplicate submission entry"
        );
        for asset in assets {
            let id = asset["entryID"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("native submission is missing entryID"))?;
            let entry = remaining
                .remove(id)
                .ok_or_else(|| anyhow::anyhow!("unexpected native submission entry {id}"))?;
            let resources = asset["resources"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("native submission is missing resources"))?;
            anyhow::ensure!(resources.len() == 1, "unsupported submission resource set");
            let resource = &resources[0];
            anyhow::ensure!(
                resource["path"] == serde_json::json!(entry.path)
                    && resource["blake3"] == entry.blake3_entry.out
                    && resource["originalFilename"].as_str()
                        == entry.path.file_name().and_then(|name| name.to_str()),
                "native submission resource identity differs for {id}"
            );
        }
        let journal_path = serde_json::from_value(request["journalPath"].clone())?;
        let backend = PhotosSubmissionBackend::Native {
            journal_path,
            request_blake3: blake3::hash(&serde_json::to_vec(request)?)
                .to_hex()
                .to_string(),
        };
        intents_for_entries(marker, entries, kind, &backend, enriched).map_err(Into::into)
    };
    let intents = build().map_err(|error| {
        ImgQualityError::AnalysisError(format!(
            "native Photos submission identity is invalid; this media import was not sent: {error:#}"
        ))
    })?;
    checkpoint_with(marker, &intents, write_marker_atomic)
}

/// A one-use reservation for a possible media import. It is not a delivery
/// receipt, and unused reservations remain possible-submission references.
#[derive(Debug)]
pub(super) struct AppleScriptSubmissionPermit {
    source_dir: PathBuf,
    working_copy: PathBuf,
    started_at: String,
    photos_library_path: Option<PathBuf>,
    history_index: usize,
    intent: PhotosSubmissionIntent,
    entry: PhotosImportPendingEntry,
}

impl AppleScriptSubmissionPermit {
    pub(super) fn authorize(
        self,
        marker: &WorkingCopyMarker,
        entry: &PhotosImportPendingEntry,
    ) -> Result<(PathBuf, String)> {
        let owner_matches = marker.src_dir == self.source_dir
            && marker.working_copy == self.working_copy
            && marker.started_at == self.started_at
            && marker.photos_library_path == self.photos_library_path;
        let entry_matches = entry.source_rel == self.entry.source_rel
            && entry.rel_path == self.entry.rel_path
            && entry.path == self.entry.path
            && entry.album_name == self.entry.album_name
            && entry.blake3_entry == self.entry.blake3_entry;
        let history_matches = marker
            .photos_submission_history()
            .and_then(|history| history.intents.get(self.history_index))
            == Some(&self.intent);
        if !owner_matches || !entry_matches || !history_matches {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos submission reservation no longer matches its task, media, or saved identity (source={} output={}); this media import was not sent",
                self.intent.source_rel, self.intent.output_rel
            )));
        }
        Ok((self.entry.path, self.entry.album_name))
    }
}

pub(super) fn prepare_applescript(
    marker: &mut WorkingCopyMarker,
    entries: &[PhotosImportPendingEntry],
    kind: PhotosSubmissionKind,
    enriched: Option<&BTreeMap<String, (String, String, String)>>,
) -> Result<Vec<AppleScriptSubmissionPermit>> {
    prepare_applescript_with(marker, entries, kind, enriched, write_marker_atomic)
}

fn prepare_applescript_with(
    marker: &mut WorkingCopyMarker,
    entries: &[PhotosImportPendingEntry],
    kind: PhotosSubmissionKind,
    enriched: Option<&BTreeMap<String, (String, String, String)>>,
    persist: impl FnOnce(&WorkingCopyMarker) -> std::io::Result<()>,
) -> Result<Vec<AppleScriptSubmissionPermit>> {
    if entries.is_empty() {
        return Err(ImgQualityError::AnalysisError(
            "empty AppleScript media submission refused".into(),
        ));
    }
    let first_index = marker
        .photos_submission_history()
        .map_or(0, |history| history.intents.len());
    let mut sources = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut intents = Vec::with_capacity(entries.len());
    for (offset, entry) in entries.iter().enumerate() {
        if !sources.insert(entry.source_rel.as_str())
            || !outputs.insert(entry.rel_path.as_str())
            || !paths.insert(entry.path.as_path())
        {
            return Err(ImgQualityError::AnalysisError(format!(
                "duplicate Photos submission reservation (source={} output={}); this media import was not sent",
                entry.source_rel, entry.rel_path
            )));
        }
        let sequence = first_index.checked_add(offset).ok_or_else(|| {
            ImgQualityError::AnalysisError("Photos submission reservation index overflow".into())
        })?;
        // Every one-file request gets an identity before the group's single
        // durable write. The task lease serializes this append sequence.
        let bytes = serde_json::to_vec(&(
            &marker.src_dir,
            &marker.working_copy,
            &marker.started_at,
            &marker.photos_library_path,
            kind,
            sequence,
            &entry.source_rel,
            &entry.rel_path,
            &entry.path,
            &entry.album_name,
            &entry.blake3_entry.src,
            &entry.blake3_entry.out,
        ))
        .map_err(|error| {
            ImgQualityError::AnalysisError(format!("Photos request identity: {error}"))
        })?;
        let backend = PhotosSubmissionBackend::Applescript {
            request_id: blake3::hash(&bytes).to_hex().to_string(),
        };
        intents.extend(intents_for_entries(
            marker,
            std::slice::from_ref(entry),
            kind,
            &backend,
            enriched,
        )?);
    }
    checkpoint_with(marker, &intents, persist)?;
    // Opaque permits are constructed only after the complete reservation group
    // is durable. Consuming one does not require another marker write or read.
    Ok(entries
        .iter()
        .cloned()
        .zip(intents)
        .enumerate()
        .map(|(offset, (entry, intent))| AppleScriptSubmissionPermit {
            source_dir: marker.src_dir.clone(),
            working_copy: marker.working_copy.clone(),
            started_at: marker.started_at.clone(),
            photos_library_path: marker.photos_library_path.clone(),
            history_index: first_index + offset,
            intent,
            entry,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_entry(root: &std::path::Path, index: usize) -> PhotosImportPendingEntry {
        PhotosImportPendingEntry {
            source_rel: format!("source-{index}.jpg"),
            rel_path: format!("output-{index}.JXL"),
            path: root.join(format!("output-{index}.JXL")),
            album_name: "Synthetic archive".into(),
            blake3_entry: super::super::Blake3Entry {
                src: blake3::hash(format!("source-{index}").as_bytes())
                    .to_hex()
                    .to_string(),
                out: blake3::hash(format!("output-{index}").as_bytes())
                    .to_hex()
                    .to_string(),
                out_rel: Some(format!("output-{index}.JXL")),
                library_asset: None,
            },
        }
    }

    #[test]
    fn applescript_reservations_persist_ten_requests_once_before_authorization()
    -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let mut marker =
            WorkingCopyMarker::new(temp.path().join("source"), temp.path().join("out"), 10);
        let entries = (0..10)
            .map(|index| synthetic_entry(temp.path(), index))
            .collect::<Vec<_>>();
        let writes = std::cell::Cell::new(0);
        let permits = prepare_applescript_with(
            &mut marker,
            &entries,
            PhotosSubmissionKind::Encoded,
            None,
            |next| {
                writes.set(writes.get() + 1);
                assert_eq!(next.photos_submission_history().unwrap().intents.len(), 10);
                Ok(())
            },
        )?;
        assert_eq!(writes.get(), 1);
        let history = marker.photos_submission_history().unwrap();
        let identities = history
            .intents
            .iter()
            .map(|intent| match &intent.backend {
                PhotosSubmissionBackend::Applescript { request_id } => request_id,
                PhotosSubmissionBackend::Native { .. } => panic!("unexpected native reservation"),
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(identities.len(), 10);
        assert!(marker.photos_imported_assets.is_empty());
        // Later outcome/progress writes and appended requests do not invalidate
        // the earlier group's exact durable reservation positions.
        marker.stage = crate::pipeline::verification::FastImgStageName::Importing;
        let additional = synthetic_entry(temp.path(), 10);
        prepare_applescript_with(
            &mut marker,
            &[additional],
            PhotosSubmissionKind::Encoded,
            None,
            |_| Ok(()),
        )?;
        for (permit, entry) in permits.into_iter().zip(&entries) {
            let manifest = permit.authorize(&marker, entry)?;
            assert_eq!(manifest, (entry.path.clone(), entry.album_name.clone()));
        }
        assert_eq!(writes.get(), 1, "authorization performs no persistence");
        Ok(())
    }

    #[test]
    fn applescript_reservation_write_failure_returns_no_permits_or_memory_update()
    -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let mut marker =
            WorkingCopyMarker::new(temp.path().join("source"), temp.path().join("out"), 10);
        let before = marker.clone();
        let entries = (0..10)
            .map(|index| synthetic_entry(temp.path(), index))
            .collect::<Vec<_>>();
        let writes = std::cell::Cell::new(0);
        let error = prepare_applescript_with(
            &mut marker,
            &entries,
            PhotosSubmissionKind::Encoded,
            None,
            |_| {
                writes.set(writes.get() + 1);
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "synthetic reservation failure",
                ))
            },
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("synthetic reservation failure"));
        assert!(message.contains("source=source-0.jpg output=output-0.JXL"));
        assert!(message.contains("this media import was not sent"));
        assert_eq!(writes.get(), 1);
        assert_eq!(marker, before);
        Ok(())
    }

    #[test]
    fn applescript_reservations_reject_changed_owner_entry_and_saved_position() -> anyhow::Result<()>
    {
        let temp = tempfile::tempdir()?;
        let base = WorkingCopyMarker::new(temp.path().join("source"), temp.path().join("out"), 1);
        let entry = synthetic_entry(temp.path(), 0);
        for mutate in [
            |marker: &mut WorkingCopyMarker| marker.src_dir.push("changed"),
            |marker: &mut WorkingCopyMarker| marker.working_copy.push("changed"),
            |marker: &mut WorkingCopyMarker| marker.started_at.push_str("changed"),
            |marker: &mut WorkingCopyMarker| {
                marker.photos_library_path = Some(marker.src_dir.join("Other.photoslibrary"));
            },
        ] {
            let mut marker = base.clone();
            let permit = prepare_applescript_with(
                &mut marker,
                std::slice::from_ref(&entry),
                PhotosSubmissionKind::Encoded,
                None,
                |_| Ok(()),
            )?
            .pop()
            .unwrap();
            mutate(&mut marker);
            assert!(permit.authorize(&marker, &entry).is_err());
        }
        for mutate in [
            |entry: &mut PhotosImportPendingEntry| entry.source_rel.push_str("changed"),
            |entry: &mut PhotosImportPendingEntry| entry.rel_path.push_str("changed"),
            |entry: &mut PhotosImportPendingEntry| entry.path.push("changed"),
            |entry: &mut PhotosImportPendingEntry| entry.album_name.push_str("changed"),
            |entry: &mut PhotosImportPendingEntry| entry.blake3_entry.src = "a".repeat(64),
            |entry: &mut PhotosImportPendingEntry| entry.blake3_entry.out = "b".repeat(64),
        ] {
            let mut marker = base.clone();
            let permit = prepare_applescript_with(
                &mut marker,
                std::slice::from_ref(&entry),
                PhotosSubmissionKind::Encoded,
                None,
                |_| Ok(()),
            )?
            .pop()
            .unwrap();
            let mut changed = entry.clone();
            mutate(&mut changed);
            assert!(permit.authorize(&marker, &changed).is_err());
        }
        for replacement in [
            serde_json::Value::Null,
            serde_json::json!({
                "schema": 1, "complete_from_creation": true, "intents": []
            }),
        ] {
            let mut marker = base.clone();
            let permit = prepare_applescript_with(
                &mut marker,
                std::slice::from_ref(&entry),
                PhotosSubmissionKind::Encoded,
                None,
                |_| Ok(()),
            )?
            .pop()
            .unwrap();
            let mut value = serde_json::to_value(&marker)?;
            value["photos_submission_history"] = replacement;
            let stale = serde_json::from_value(value)?;
            assert!(permit.authorize(&stale, &entry).is_err());
        }
        let mut marker = base;
        let permit = prepare_applescript_with(
            &mut marker,
            std::slice::from_ref(&entry),
            PhotosSubmissionKind::Encoded,
            None,
            |_| Ok(()),
        )?
        .pop()
        .unwrap();
        let mut value = serde_json::to_value(&marker)?;
        value["photos_submission_history"]["intents"][0]["output_blake3"] =
            serde_json::json!("f".repeat(64));
        let stale = serde_json::from_value(value)?;
        assert!(permit.authorize(&stale, &entry).is_err());
        Ok(())
    }

    #[test]
    fn applescript_reservations_reject_duplicates_and_preserve_unknown_coverage()
    -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let marker = WorkingCopyMarker::new(temp.path().join("source"), temp.path().join("out"), 1);
        let mut value = serde_json::to_value(marker)?;
        value
            .as_object_mut()
            .unwrap()
            .remove("photos_submission_history");
        let mut legacy: WorkingCopyMarker = serde_json::from_value(value)?;
        let before = legacy.clone();
        let entry = synthetic_entry(temp.path(), 0);
        let duplicate = [entry.clone(), entry.clone()];
        let writes = std::cell::Cell::new(0);
        assert!(
            prepare_applescript_with(
                &mut legacy,
                &duplicate,
                PhotosSubmissionKind::Encoded,
                None,
                |_| {
                    writes.set(writes.get() + 1);
                    Ok(())
                },
            )
            .is_err()
        );
        assert_eq!(legacy, before);
        assert_eq!(writes.get(), 0);
        let permit = prepare_applescript_with(
            &mut legacy,
            std::slice::from_ref(&entry),
            PhotosSubmissionKind::Encoded,
            None,
            |_| {
                writes.set(writes.get() + 1);
                Ok(())
            },
        )?
        .pop()
        .unwrap();
        assert!(
            !legacy
                .photos_submission_history()
                .unwrap()
                .complete_from_creation
        );
        permit.authorize(&legacy, &entry)?;
        assert_eq!(writes.get(), 1);
        Ok(())
    }

    #[test]
    fn original_submission_keeps_source_and_xmp_enriched_delivery_identities_separate()
    -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let marker = WorkingCopyMarker::new(temp.path().join("source"), temp.path().join("out"), 1);
        let source_hash = blake3::hash(b"original source").to_hex().to_string();
        let output_hash = blake3::hash(b"source with XMP").to_hex().to_string();
        let xmp_hash = blake3::hash(b"sidecar").to_hex().to_string();
        let entry = PhotosImportPendingEntry {
            source_rel: "image.heic".into(),
            rel_path: "image.heic".into(),
            path: temp.path().join("staged/image.heic"),
            album_name: "Test".into(),
            blake3_entry: super::super::Blake3Entry {
                src: output_hash.clone(),
                out: output_hash.clone(),
                ..Default::default()
            },
        };
        let backend = PhotosSubmissionBackend::Applescript {
            request_id: blake3::hash(b"request").to_hex().to_string(),
        };
        let mut enriched = BTreeMap::from([(
            "image.heic".into(),
            (source_hash.clone(), output_hash.clone(), xmp_hash),
        )]);
        let intents = intents_for_entries(
            &marker,
            std::slice::from_ref(&entry),
            PhotosSubmissionKind::Original,
            &backend,
            Some(&enriched),
        )?;
        assert_eq!(intents[0].source_blake3, source_hash);
        assert_eq!(intents[0].output_blake3, output_hash);
        enriched.get_mut("image.heic").unwrap().1 = source_hash;
        assert!(
            intents_for_entries(
                &marker,
                &[entry],
                PhotosSubmissionKind::Original,
                &backend,
                Some(&enriched)
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn submission_write_failure_keeps_in_memory_authority_unchanged() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let mut marker =
            WorkingCopyMarker::new(temp.path().join("source"), temp.path().join("out"), 1);
        let original = marker.clone();
        let intent = PhotosSubmissionIntent {
            kind: PhotosSubmissionKind::Encoded,
            source_rel: "image.jpg".into(),
            source_blake3: blake3::hash(b"source").to_hex().to_string(),
            output_rel: "image.JXL".into(),
            output_blake3: blake3::hash(b"output").to_hex().to_string(),
            album_name: "Test".into(),
            photos_library_path: Some(temp.path().join("Test.photoslibrary")),
            backend: PhotosSubmissionBackend::Applescript {
                request_id: blake3::hash(b"request").to_hex().to_string(),
            },
        };
        let result = checkpoint_with(&mut marker, &[intent], |next| {
            assert_eq!(next.photos_submission_history().unwrap().intents.len(), 1);
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "synthetic write failure",
            ))
        });
        assert!(result.is_err());
        assert_eq!(marker, original);
        Ok(())
    }

    #[test]
    fn native_submission_rejects_request_entry_drift_before_checkpointing() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let mut marker =
            WorkingCopyMarker::new(temp.path().join("source"), temp.path().join("out"), 1);
        let entry = PhotosImportPendingEntry {
            source_rel: "image.jpg".into(),
            rel_path: "image.JXL".into(),
            path: temp.path().join("out/image.JXL"),
            album_name: "Test".into(),
            blake3_entry: super::super::Blake3Entry {
                src: blake3::hash(b"source").to_hex().to_string(),
                out: blake3::hash(b"output").to_hex().to_string(),
                ..Default::default()
            },
        };
        let original = marker.clone();
        let request = serde_json::json!({
            "version": 1, "operation": "import",
            "journalPath": temp.path().join("native/batch-0.json"),
            "assets": [{"entryID": "image.JXL", "resources": [{
                "path": entry.path, "originalFilename": "image.JXL",
                "blake3": blake3::hash(b"different bytes").to_hex().to_string()
            }]}]
        });
        let error = record_native(
            &mut marker,
            &[entry],
            &request,
            PhotosSubmissionKind::Encoded,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("resource identity differs"));
        assert_eq!(marker, original);
        Ok(())
    }
}
