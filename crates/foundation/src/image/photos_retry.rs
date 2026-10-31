//! Cross-attempt Photos recovery consumes prior identities before any new import.
//! A possible submission is never negative evidence, even after a successful
//! filename lookup returns no candidates.

use super::{
    BTreeMap, BTreeSet, FastImgLibraryAssetProbe, ImgQualityError, LibraryAssetRecord, Path,
    PhotosImportPendingEntry, Result, WorkingCopyMarker,
};
use crate::pipeline::verification::{
    PhotosSubmissionBackend, PhotosSubmissionIntent, PhotosSubmissionKind,
};
use serde_json::Value;
use std::io::Read;

fn unresolved(detail: impl std::fmt::Display) -> ImgQualityError {
    ImgQualityError::AnalysisError(format!(
        "Prior Photos submission remains unresolved: {detail}; no new import was sent, sources and recovery state retained"
    ))
}

/// Full native transactions are checked before selecting any retry entries.
/// Neither a request reference nor this identity mapping proves custody.
pub(crate) fn native_committed_pairs(
    request: &Value,
    record: &Value,
) -> anyhow::Result<Vec<(String, String)>> {
    anyhow::ensure!(
        record["version"] == 1
            && record["batchID"] == request["batchID"]
            && record["request"] == *request,
        "PhotoKit journal belongs to another request"
    );
    anyhow::ensure!(
        matches!(
            record["state"].as_str(),
            Some("committed" | "identifier-known")
        ),
        "PhotoKit commit is uncertain without complete identifiers; original files retained"
    );
    let assets = request["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing requested assets"))?;
    let expected = assets
        .iter()
        .map(|asset| {
            asset["entryID"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("missing entry ID"))
        })
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    anyhow::ensure!(expected.len() == assets.len(), "duplicate task entry IDs");
    let mut pairs = BTreeMap::new();
    let mut seen_ids = BTreeSet::new();
    for identity in record["identities"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing durable asset identifiers"))?
    {
        let entry = identity["entryID"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing entry ID"))?;
        let id = identity["localIdentifier"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing asset identifier"))?;
        anyhow::ensure!(
            expected.contains(entry)
                && !id.is_empty()
                && seen_ids.insert(id.to_owned())
                && pairs.insert(entry.to_owned(), id.to_owned()).is_none(),
            "ambiguous PhotoKit identity mapping"
        );
    }
    anyhow::ensure!(
        pairs.len() == expected.len(),
        "partial PhotoKit identity mapping; do not reimport"
    );
    Ok(pairs.into_iter().collect())
}

fn native_identifier(
    intent: &PhotosSubmissionIntent,
    record: &Value,
    entry: &PhotosImportPendingEntry,
) -> Result<String> {
    let PhotosSubmissionBackend::Native {
        journal_path,
        request_blake3,
    } = &intent.backend
    else {
        return Err(unresolved("native recovery received another backend"));
    };
    let request = &record["request"];
    let bytes = serde_json::to_vec(request).map_err(unresolved)?;
    if request["version"] != 1
        || request["operation"] != "import"
        || request["journalPath"] != serde_json::json!(journal_path)
        || blake3::hash(&bytes).to_hex().as_str() != request_blake3
    {
        return Err(unresolved(
            "native journal request identity differs from the saved parent intent",
        ));
    }
    // Validate the entire parent request, including entries outside the child.
    // Never truncate it or publish the parent's other entries as child results.
    let pairs = native_committed_pairs(request, record).map_err(unresolved)?;
    let asset = request["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|asset| asset["entryID"] == intent.output_rel)
        })
        .ok_or_else(|| unresolved("native journal is missing the selected failed entry"))?;
    let resources = asset["resources"]
        .as_array()
        .ok_or_else(|| unresolved("native journal is missing original resources"))?;
    let resource = resources
        .first()
        .ok_or_else(|| unresolved("native original resource set is empty"))?;
    if resources.len() != 1
        || resource["kind"] != "photo"
        || resource["blake3"] != intent.output_blake3
        || resource["originalFilename"].as_str()
            != entry.path.file_name().and_then(|name| name.to_str())
        || resource["path"]
            .as_str()
            .is_none_or(|path| !Path::new(path).is_absolute())
        || asset["albumIdentifier"].as_str().is_none_or(str::is_empty)
    {
        return Err(unresolved(
            "native original resource or album binding changed",
        ));
    }
    pairs
        .into_iter()
        .find(|(rel, _)| rel == &intent.output_rel)
        .map(|(_, id)| id)
        .ok_or_else(|| unresolved("native journal has no unique selected identifier"))
}

fn prior_intents<'a>(
    ancestors: &'a [WorkingCopyMarker],
    admitted: &BTreeMap<String, String>,
    entries: &[PhotosImportPendingEntry],
    kind: PhotosSubmissionKind,
    enriched: &BTreeMap<String, (String, String, String)>,
    library: Option<&Path>,
) -> Result<Vec<Vec<&'a PhotosSubmissionIntent>>> {
    let mut selected = Vec::with_capacity(entries.len());
    for entry in entries {
        let source_hash = enriched
            .get(&entry.source_rel)
            .map_or(&entry.blake3_entry.src, |(source, _, _)| source);
        if admitted.get(&entry.source_rel) != Some(source_hash) {
            return Err(unresolved(format!(
                "current source was not admitted to this failed-only attempt: {}",
                entry.source_rel
            )));
        }
        let mut intents = Vec::new();
        for ancestor in ancestors {
            let history = ancestor
                .photos_submission_history()
                .filter(|history| history.complete_from_creation)
                .ok_or_else(|| {
                    unresolved(format!(
                        "historical submission coverage is unknown for {}",
                        entry.source_rel
                    ))
                })?;
            for intent in history.intents.iter().filter(|intent| {
                intent.source_rel == entry.source_rel && &intent.source_blake3 == source_hash
            }) {
                if intent.kind != kind
                    || intent.output_rel != entry.rel_path
                    || intent.output_blake3 != entry.blake3_entry.out
                    || intent.album_name != entry.album_name
                    || intent.photos_library_path.as_deref() != library
                {
                    return Err(unresolved(format!(
                        "prior submission differs from current output, route, album, or library for {}",
                        entry.source_rel
                    )));
                }
                if !intents.contains(&intent) {
                    intents.push(intent);
                }
            }
        }
        selected.push(intents);
    }
    Ok(selected)
}

fn read_journal(path: &Path) -> Result<Value> {
    let root = crate::process_lock::inspect_mfb_root_path()
        .map_err(unresolved)?
        .join("photos-native");
    if !path.starts_with(&root) {
        return Err(unresolved(
            "native journal is outside the formal Photos state root",
        ));
    }
    let root_metadata = std::fs::symlink_metadata(&root).map_err(unresolved)?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(unresolved(
            "native journal root is not a real state directory",
        ));
    }
    let mut parent = root.clone();
    for part in path.strip_prefix(&root).map_err(unresolved)?.components() {
        let std::path::Component::Normal(name) = part else {
            return Err(unresolved("native journal path is not normalized"));
        };
        parent.push(name);
        let metadata = std::fs::symlink_metadata(&parent).map_err(unresolved)?;
        if metadata.file_type().is_symlink() {
            return Err(unresolved("native journal path contains a symlink"));
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(unresolved)?;
    let metadata = file.metadata().map_err(unresolved)?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        return Err(unresolved("native journal is not a bounded regular file"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(unresolved)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(unresolved("native journal grew beyond the allowed size"));
    }
    decode_journal(&bytes)
}

fn decode_journal(bytes: &[u8]) -> Result<Value> {
    crate::common_utils::validate_unique_json_members(bytes).map_err(unresolved)?;
    serde_json::from_slice(bytes).map_err(unresolved)
}

// A negative content match is a typed outcome reached only after a successful
// UUID query and an actual original-resource hash. Diagnostic text never grants
// permission to ignore a query, permission, export or I/O error.
fn probe_candidate<Q, P>(
    entry: &PhotosImportPendingEntry,
    identifier: &str,
    query: &mut Q,
    quarantined: &mut P,
) -> Result<Option<LibraryAssetRecord>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    let targets =
        super::fast_img_import_probe_targets(&[(entry.rel_path.clone(), identifier.to_owned())])?;
    let mut probes = super::query_uploaded_asset_probes_batch_with_retry(
        &targets,
        super::fast_img_icloud_upload_verify_attempts(),
        super::fast_img_icloud_upload_verify_batch_size(),
        super::fast_img_icloud_upload_verify_delay(),
        super::fast_img_require_icloud_upload_proof(),
        query,
        std::thread::sleep,
    )?;
    if probes.len() != 1 {
        return Err(unresolved(
            "prior UUID query returned an incomplete or ambiguous original-resource mapping",
        ));
    }
    let probe = probes
        .remove(&entry.rel_path)
        .ok_or_else(|| unresolved("prior UUID query is missing the selected original resource"))?;
    super::verify_import_probe_uuid_binding(&targets[0], &probe)?;
    let hash = crate::common_utils::calculate_blake3_hash(&probe.path).map_err(unresolved)?;
    if hash != entry.blake3_entry.out {
        return Ok(None);
    }
    Ok(Some(LibraryAssetRecord {
        rel_path: entry.rel_path.clone(),
        blake3: hash,
        sync_status: super::photos_sync_status(&probe).to_owned(),
        quarantined: quarantined(&entry.path)?,
        photos_uuid: Some(probe.uuid),
        library_blake3: None,
        xmp_sidecar_blake3: None,
    }))
}

fn reconcile_with<Q, P, D, J>(
    ancestors: &[WorkingCopyMarker],
    admitted: &BTreeMap<String, String>,
    entries: &[PhotosImportPendingEntry],
    kind: PhotosSubmissionKind,
    enriched: &BTreeMap<String, (String, String, String)>,
    library: Option<&Path>,
    query: &mut Q,
    quarantined: &mut P,
    discover: &mut D,
    journal: &mut J,
) -> Result<Vec<LibraryAssetRecord>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
    D: FnMut(&PhotosImportPendingEntry) -> Result<Vec<String>>,
    J: FnMut(&Path) -> Result<Value>,
{
    let projected = prior_intents(ancestors, admitted, entries, kind, enriched, library)?;
    let mut verified = Vec::new();
    for (entry, intents) in entries.iter().zip(projected) {
        if intents.is_empty() {
            continue;
        } // Complete coverage proves no old side effect for this identity.
        let discovered = discover(entry)?;
        if discovered.is_empty() {
            return Err(unresolved(format!(
                "previously reserved media was not found in its album: {}",
                entry.source_rel
            )));
        }
        let mut proven = BTreeMap::<String, LibraryAssetRecord>::new();
        for intent in intents {
            let ids = match &intent.backend {
                PhotosSubmissionBackend::Native { journal_path, .. } => {
                    let id = native_identifier(intent, &journal(journal_path)?, entry)?;
                    let uuid = super::osxphotos_uuid_from_photos_import_identifier(&id)?;
                    if !discovered.iter().any(|candidate| {
                        super::osxphotos_uuid_from_photos_import_identifier(candidate)
                            .is_ok_and(|candidate| candidate == uuid)
                    }) {
                        return Err(unresolved(
                            "native UUID is not present in the recorded destination album",
                        ));
                    }
                    vec![id]
                }
                PhotosSubmissionBackend::Applescript { .. } => discovered.clone(),
            };
            let mut matched = false;
            for id in ids {
                match probe_candidate(entry, &id, query, quarantined)? {
                    Some(asset) => {
                        let uuid = asset
                            .photos_uuid
                            .clone()
                            .ok_or_else(|| unresolved("fresh original proof has no UUID"))?;
                        proven.insert(uuid, asset);
                        matched = true;
                    }
                    None if matches!(
                        intent.backend,
                        PhotosSubmissionBackend::Applescript { .. }
                    ) => {}
                    None => {
                        return Err(unresolved(
                            "native submitted original bytes differ from the expected output",
                        ));
                    }
                }
            }
            if !matched {
                return Err(unresolved(format!(
                    "no exact original-resource match for {}",
                    entry.source_rel
                )));
            }
        }
        if proven.len() != 1 {
            return Err(unresolved(format!(
                "multiple exact Photos assets match prior submissions for {}",
                entry.source_rel
            )));
        }
        verified.extend(proven.into_values());
    }
    Ok(verified)
}

/// Only a failed-only child has cross-attempt authority. Ordinary attempts
/// retain their existing same-attempt import flow.
pub(super) fn reconcile<Q, P>(
    marker: &mut WorkingCopyMarker,
    entries: &[PhotosImportPendingEntry],
    kind: PhotosSubmissionKind,
    enriched: &BTreeMap<String, (String, String, String)>,
    library: Option<&Path>,
    query: &mut Q,
    quarantined: &mut P,
) -> Result<()>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    if entries.is_empty() || marker.failed_retry_manifest_blake3.is_none() {
        return Ok(());
    }
    let context = crate::db::fast_img_store::failed_retry_context(&marker.working_copy)
        .map_err(unresolved)?
        .ok_or_else(|| unresolved("failed-only child has no durable parent relation"))?;
    let mut admitted = BTreeMap::new();
    for entry in entries {
        let admission = context.admissions().get(&entry.source_rel).ok_or_else(|| {
            unresolved("pending media is outside the failed-only admission manifest")
        })?;
        crate::db::fast_img_store::verify_failed_retry_admission(&marker.src_dir, admission)
            .map_err(unresolved)?;
        if crate::common_utils::calculate_blake3_hash(&entry.path).map_err(unresolved)?
            != entry.blake3_entry.out
        {
            return Err(unresolved(
                "current delivery output changed before prior-submission recovery",
            ));
        }
        admitted.insert(
            entry.source_rel.clone(),
            admission.admitted_source_blake3.clone(),
        );
    }
    let mut ancestors = context.ancestors().to_vec();
    // A resumed child can itself have an unacknowledged request. Its own
    // append-only history is subject to the same gate as the parent's history.
    ancestors.push(marker.clone());
    let assets = reconcile_with(
        &ancestors,
        &admitted,
        entries,
        kind,
        enriched,
        library,
        query,
        quarantined,
        &mut |entry| {
            let candidate = super::photos_import_candidate_from_pending(entry);
            let candidates = std::slice::from_ref(&candidate);
            let output = super::run_photos_import_applescript_session_mode_in_library(
                "failed-only prior submission reconciliation",
                &super::photos_import_candidate_manifest_entries(candidates),
                "reconcile_all",
                library,
            )?;
            super::photos_reconciled_candidate_ids(candidates, &output)?
                .pop()
                .ok_or_else(|| unresolved("prior reconciliation returned no result row"))
        },
        &mut read_journal,
    )?;
    if assets.is_empty() {
        return Ok(());
    }
    // A Photos query can wait on the library. Recheck the admitted source,
    // sidecars and exact current output before publishing any recovered receipt.
    for entry in entries {
        let admission = context
            .admissions()
            .get(&entry.source_rel)
            .ok_or_else(|| unresolved("failed-only admission disappeared during recovery"))?;
        crate::db::fast_img_store::verify_failed_retry_admission(&marker.src_dir, admission)
            .map_err(unresolved)?;
        if crate::common_utils::calculate_blake3_hash(&entry.path).map_err(unresolved)?
            != entry.blake3_entry.out
        {
            return Err(unresolved(
                "delivery output changed during prior-submission recovery",
            ));
        }
    }
    match kind {
        PhotosSubmissionKind::Encoded => {
            let paths = assets
                .iter()
                .map(|asset| asset.rel_path.as_str())
                .collect::<BTreeSet<_>>();
            let recovered = entries
                .iter()
                .filter(|entry| paths.contains(entry.rel_path.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            super::checkpoint_photos_import_window(marker, &recovered, &assets)
        }
        PhotosSubmissionKind::Original => super::checkpoint_tier2_library_assets(
            marker,
            &super::LibraryHandle {
                imported_assets: assets,
                import_error_count: 0,
                photos_library_path: library.map(Path::to_path_buf),
            },
            enriched,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::verification::Blake3Entry;

    struct Fixture {
        _root: tempfile::TempDir,
        parent: WorkingCopyMarker,
        entry: PhotosImportPendingEntry,
        admitted: BTreeMap<String, String>,
        record: Value,
        probe: FastImgLibraryAssetProbe,
    }

    fn fixture() -> anyhow::Result<Fixture> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let old_output = root.path().join("old-output");
        let child_output = root.path().join("retry-output");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir(&child_output)?;
        std::fs::write(source.join("b.jpg"), b"admitted failed source")?;
        let path = child_output.join("b.JXL");
        std::fs::write(&path, b"exact output and Photos original")?;
        let original = root.path().join("photos-original.JXL");
        std::fs::copy(&path, &original)?;
        let source_hash = crate::common_utils::calculate_blake3_hash(&source.join("b.jpg"))?;
        let output_hash = crate::common_utils::calculate_blake3_hash(&path)?;
        let journal = root.path().join("native-parent/batch-0.json");
        let request = serde_json::json!({"version":1,"operation":"import","batchID":"batch-0",
            "journalPath":journal,"witnessIdentifiers":["LIBRARY-WITNESS"],"assets":[
                {"entryID":"a.JXL","albumIdentifier":"ALBUM","resources":[{"path":old_output.join("a.JXL"),"kind":"photo","blake3":blake3::hash(b"successful A").to_hex().to_string(),"originalFilename":"a.JXL"}]},
                {"entryID":"b.JXL","albumIdentifier":"ALBUM","resources":[{"path":old_output.join("b.JXL"),"kind":"photo","blake3":output_hash,"originalFilename":"b.JXL"}]}]});
        let mut parent = WorkingCopyMarker::new(source, old_output, 2);
        parent
            .record_photos_submissions(&[PhotosSubmissionIntent {
                kind: PhotosSubmissionKind::Encoded,
                source_rel: "b.jpg".into(),
                source_blake3: source_hash.clone(),
                output_rel: "b.JXL".into(),
                output_blake3: output_hash.clone(),
                album_name: "Archive".into(),
                photos_library_path: None,
                backend: PhotosSubmissionBackend::Native {
                    journal_path: journal,
                    request_blake3: blake3::hash(&serde_json::to_vec(&request)?)
                        .to_hex()
                        .to_string(),
                },
            }])
            .map_err(anyhow::Error::msg)?;
        let entry = PhotosImportPendingEntry {
            source_rel: "b.jpg".into(),
            rel_path: "b.JXL".into(),
            path,
            album_name: "Archive".into(),
            blake3_entry: Blake3Entry {
                src: source_hash.clone(),
                out: output_hash,
                out_rel: Some("b.JXL".into()),
                library_asset: None,
            },
        };
        Ok(Fixture {
            _root: root,
            parent,
            entry,
            admitted: BTreeMap::from([("b.jpg".into(), source_hash)]),
            record: serde_json::json!({"version":1,"batchID":"batch-0","state":"committed","request":request,
                "identities":[{"entryID":"a.JXL","localIdentifier":"UUID-A/L0/001"},{"entryID":"b.JXL","localIdentifier":"UUID-B/L0/001"}]}),
            probe: FastImgLibraryAssetProbe {
                uuid: "UUID-B".into(),
                path: original,
                iscloudasset: false,
                incloud: Some(false),
                ismissing: false,
            },
        })
    }

    #[test]
    fn native_parent_full_request_recovers_only_failed_child_after_old_outputs_are_gone()
    -> anyhow::Result<()> {
        let fixture = fixture()?;
        let before = fixture.parent.clone();
        assert!(!fixture.parent.working_copy.exists());
        let queried = std::cell::Cell::new(0);
        let assets = reconcile_with(
            std::slice::from_ref(&fixture.parent),
            &fixture.admitted,
            std::slice::from_ref(&fixture.entry),
            PhotosSubmissionKind::Encoded,
            &BTreeMap::new(),
            None,
            &mut |ids| {
                assert_eq!(ids, &["UUID-B"]);
                queried.set(queried.get() + 1);
                Ok(vec![fixture.probe.clone()])
            },
            &mut |_| Ok(false),
            &mut |_| Ok(vec!["UUID-B/L0/001".into()]),
            &mut |_| Ok(fixture.record.clone()),
        )?;
        assert_eq!(queried.get(), 1);
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].rel_path, "b.JXL");
        assert_eq!(assets[0].photos_uuid.as_deref(), Some("UUID-B"));
        assert_eq!(
            fixture.parent, before,
            "parent results and complete journal must remain unchanged"
        );
        assert_eq!(
            fixture.record["request"]["assets"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        Ok(())
    }

    #[test]
    fn missing_partial_or_changed_native_evidence_never_admits_a_new_import() -> anyhow::Result<()>
    {
        let fixture = fixture()?;
        let mut records = Vec::new();
        for state in ["submitted", "uncertain"] {
            let mut record = fixture.record.clone();
            record["state"] = serde_json::json!(state);
            records.push(record);
        }
        let mut partial = fixture.record.clone();
        partial["identities"].as_array_mut().unwrap().pop();
        records.push(partial);
        let mut changed = fixture.record.clone();
        changed["request"]["assets"][0]["resources"][0]["blake3"] =
            serde_json::json!(blake3::hash(b"changed").to_hex().to_string());
        records.push(changed);
        let sends = std::cell::Cell::new(0);
        for record in records {
            let result = reconcile_with(
                std::slice::from_ref(&fixture.parent),
                &fixture.admitted,
                std::slice::from_ref(&fixture.entry),
                PhotosSubmissionKind::Encoded,
                &BTreeMap::new(),
                None,
                &mut |_| panic!("incomplete native reference must not become a custody proof"),
                &mut |_| Ok(false),
                &mut |_| Ok(vec!["UUID-B/L0/001".into()]),
                &mut |_| Ok(record.clone()),
            );
            if result.is_ok() {
                sends.set(sends.get() + 1);
            }
            assert!(result.is_err());
        }
        let result = reconcile_with(
            std::slice::from_ref(&fixture.parent),
            &fixture.admitted,
            std::slice::from_ref(&fixture.entry),
            PhotosSubmissionKind::Encoded,
            &BTreeMap::new(),
            None,
            &mut |_| panic!("missing journal cannot authorize a new import"),
            &mut |_| Ok(false),
            &mut |_| Ok(vec!["UUID-B/L0/001".into()]),
            &mut |_| Err(unresolved("journal missing")),
        );
        assert!(result.is_err());
        assert_eq!(sends.get(), 0);
        Ok(())
    }

    #[test]
    fn unknown_coverage_changed_outputs_and_failed_discovery_remain_unresolved()
    -> anyhow::Result<()> {
        let fixture = fixture()?;
        let mut unknown = serde_json::to_value(&fixture.parent)?;
        unknown
            .as_object_mut()
            .unwrap()
            .remove("photos_submission_history");
        let unknown: WorkingCopyMarker = serde_json::from_value(unknown)?;
        assert!(
            prior_intents(
                &[unknown],
                &fixture.admitted,
                std::slice::from_ref(&fixture.entry),
                PhotosSubmissionKind::Encoded,
                &BTreeMap::new(),
                None
            )
            .is_err()
        );
        let mut changed = fixture.entry.clone();
        changed.blake3_entry.out = blake3::hash(b"new output").to_hex().to_string();
        assert!(
            prior_intents(
                std::slice::from_ref(&fixture.parent),
                &fixture.admitted,
                &[changed],
                PhotosSubmissionKind::Encoded,
                &BTreeMap::new(),
                None
            )
            .is_err()
        );
        for discovery in [Ok(Vec::new()), Err(unresolved("permission denied (-1743)"))] {
            let mut discovery = Some(discovery);
            assert!(
                reconcile_with(
                    std::slice::from_ref(&fixture.parent),
                    &fixture.admitted,
                    std::slice::from_ref(&fixture.entry),
                    PhotosSubmissionKind::Encoded,
                    &BTreeMap::new(),
                    None,
                    &mut |_| panic!("failed/empty discovery cannot establish custody"),
                    &mut |_| Ok(false),
                    &mut |_| discovery.take().unwrap(),
                    &mut |_| panic!("failed discovery is not an absent import")
                )
                .is_err()
            );
        }
        // A genuinely complete empty history permits this new strong identity.
        let fresh = WorkingCopyMarker::new(
            fixture.parent.src_dir.clone(),
            fixture.parent.working_copy.clone(),
            1,
        );
        assert!(
            prior_intents(
                &[fresh],
                &fixture.admitted,
                std::slice::from_ref(&fixture.entry),
                PhotosSubmissionKind::Encoded,
                &BTreeMap::new(),
                None
            )?[0]
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn appleevent_possible_submission_requires_a_unique_fresh_original_match() -> anyhow::Result<()>
    {
        let mut fixture = fixture()?;
        let mut value = serde_json::to_value(&fixture.parent)?;
        value["photos_submission_history"]["intents"][0]["backend"] =
            serde_json::json!({"backend":"applescript","request_id":"prior-appleevent"});
        fixture.parent = serde_json::from_value(value)?;
        let query_error = reconcile_with(
            std::slice::from_ref(&fixture.parent),
            &fixture.admitted,
            std::slice::from_ref(&fixture.entry),
            PhotosSubmissionKind::Encoded,
            &BTreeMap::new(),
            None,
            &mut |_| {
                Err(unresolved(
                    "query transport failed with text: Photos verifier BLAKE3 mismatch",
                ))
            },
            &mut |_| Ok(false),
            &mut |_| Ok(vec!["UUID-B/L0/001".into()]),
            &mut |_| panic!("AppleScript uses its recorded possible-effect reference"),
        );
        assert!(query_error.is_err());
        let assets = reconcile_with(
            std::slice::from_ref(&fixture.parent),
            &fixture.admitted,
            std::slice::from_ref(&fixture.entry),
            PhotosSubmissionKind::Encoded,
            &BTreeMap::new(),
            None,
            &mut |_| Ok(vec![fixture.probe.clone()]),
            &mut |_| Ok(false),
            &mut |_| Ok(vec!["UUID-B/L0/001".into()]),
            &mut |_| panic!("AppleScript has no native journal"),
        )?;
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].photos_uuid.as_deref(), Some("UUID-B"));
        Ok(())
    }

    #[test]
    fn journal_rejects_duplicate_members_before_value_can_discard_them() -> anyhow::Result<()> {
        for raw in [
            r#"{"state":"committed","state":"submitted"}"#,
            r#"{"request":{"version":1,"version":2}}"#,
            r#"{"state":"committed","st\u0061te":"uncertain"}"#,
            r#"{"identities":[{"entryID":"b","entryID":"a"}]}"#,
        ] {
            assert!(decode_journal(raw.as_bytes()).is_err(), "{raw}");
        }
        let fixture = fixture()?;
        assert_eq!(
            decode_journal(&serde_json::to_vec(&fixture.record)?)?,
            fixture.record
        );
        Ok(())
    }
}
