//! One read-only view of a source's committed delivery and cleanup evidence.

use super::*;
use crate::pipeline::verification::{
    FastImgProcessingContract, FastImgStageName, validate_marker_relative_path,
};

#[derive(Debug, Serialize)]
pub struct SourceEvidence {
    pub schema_version: u8,
    pub scope: &'static str,
    pub source_relative: String,
    pub coverage: &'static str,
    pub task_key: String,
    pub task_revision: i64,
    pub parent_task_key: Option<String>,
    pub stage: FastImgStageName,
    pub admission: Option<FastImgSourceAdmission>,
    pub retry_parent_source_blake3: Option<String>,
    pub retry_admitted_source_blake3: Option<String>,
    pub processing_contract: Option<FastImgProcessingContract>,
    pub tool_snapshot_identity: Option<String>,
    pub outcome: Option<SourceOutcome>,
    pub primary_result: Option<PrimaryResultEvidence>,
    pub photos_library_path: Option<PathBuf>,
    pub photos_receipt: Option<LibraryAssetRecord>,
    pub photos_submission_history_complete: Option<bool>,
    pub photos_submissions: Vec<SubmissionEvidence>,
    pub cleanup_journal: Option<&'static str>,
    pub cleanup: Vec<CleanupEvidence>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceOutcome {
    Converted { receipt: Blake3Entry },
    Skipped { result: SkippedSourceEntry },
    Failed { result: SkippedSourceEntry },
    OriginalRejected { result: SkippedSourceEntry },
    OriginalDelivery,
}

#[derive(Debug, Serialize)]
pub struct SubmissionEvidence {
    kind: PhotosSubmissionKind,
    request_identity: String,
    backend: &'static str,
    source_blake3: String,
    output_relative: String,
    output_blake3: String,
    applescript_uncertainty_recorded: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct CleanupEvidence {
    kind: String,
    relative_path: String,
    blake3: String,
    unlink_state: UnlinkState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum UnlinkState {
    /// Only an intent was recorded. The unlink outcome is unknown.
    Planned,
    Removed,
}

// checked_marker already validated these rows and their hashes in this transaction.
fn cleanup_rows(
    conn: &Connection,
    key: &str,
    source: &str,
    journal: Option<&str>,
) -> Result<Vec<CleanupEvidence>> {
    let table = match journal {
        Some("primary") => "fast_img_primary_cleanup",
        Some("original") => "fast_img_tier2_cleanup",
        None => return Ok(Vec::new()),
        Some(_) => bail!("Unknown Fast IMG cleanup journal"),
    };
    let mut statement = conn.prepare(&format!(
        "SELECT kind,entry_rel,entry_blake3,state FROM {table}
         WHERE task_key=?1 AND source_rel=?2 ORDER BY entry_rel"
    ))?;
    let rows = statement.query_map(params![key, source], |row| {
        Ok(CleanupEvidence {
            kind: row.get(0)?,
            relative_path: row.get(1)?,
            blake3: row.get(2)?,
            unlink_state: match row.get_ref(3)?.as_str()? {
                "planned" => UnlinkState::Planned,
                "removed" => UnlinkState::Removed,
                _ => return Err(rusqlite::Error::InvalidQuery),
            },
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Read exact source evidence from one validated SQLite snapshot. This reports
/// historical records without rechecking media, contacting Photos, or authorizing cleanup.
///
/// # Errors
/// Rejects noncanonical source keys, missing or corrupt state, and ambiguous receipt ownership.
pub fn read_source_evidence(working_copy: &Path, source: &str) -> io::Result<SourceEvidence> {
    (|| -> Result<SourceEvidence> {
        validate_marker_relative_path("source evidence query", source)
            .map_err(anyhow::Error::msg)?;
        migration::reject_unmigrated_for_working_copy(working_copy)?;
        let wc = canonical_text(working_copy)?;
        let key = key_for_canonical(&wc);
        let conn = open_readonly()?.context("Fast IMG source evidence store is unavailable")?;
        in_snapshot(&conn, |conn| {
            let marker =
                checked_marker(conn, &wc)?.context("No committed Fast IMG task for this output")?;
            let revision = conn.query_row(
                "SELECT revision FROM fast_img_tasks WHERE task_key=?1",
                [&key],
                |row| row.get(0),
            )?;
            let retry = if marker.failed_retry_manifest_blake3.is_some() {
                retry::context_for(conn, &key, &marker)?
            } else {
                None
            };
            let plan = admission::read_validated(conn, &key, &marker)?;
            let retry_admission = retry
                .as_ref()
                .and_then(|retry| retry.admissions().get(source));
            let admission = plan.as_ref().and_then(|plan| plan.get(source)).cloned();
            let primary = marker.blake3_log.get(source);
            let original = marker
                .tier2_imported_assets
                .iter()
                .find(|asset| asset.rel_path == source);
            ensure!(
                original.is_none()
                    || (primary.is_none()
                        && !marker.skipped_sources.contains_key(source)
                        && !marker.failed_sources.contains_key(source)),
                "Source has conflicting Photos delivery routes"
            );
            let outcome = primary
                .map(|receipt| SourceOutcome::Converted {
                    receipt: receipt.clone(),
                })
                .or_else(|| {
                    marker
                        .skipped_sources
                        .get(source)
                        .map(|result| SourceOutcome::Skipped {
                            result: result.clone(),
                        })
                })
                .or_else(|| {
                    marker
                        .failed_sources
                        .get(source)
                        .map(|result| SourceOutcome::Failed {
                            result: result.clone(),
                        })
                })
                .or_else(|| {
                    marker.tier2_failed_sources.get(source).map(|result| {
                        SourceOutcome::OriginalRejected {
                            result: result.clone(),
                        }
                    })
                })
                .or_else(|| original.map(|_| SourceOutcome::OriginalDelivery));
            let primary_asset = if let Some((record, output)) = primary
                .and_then(|receipt| receipt.out_rel.as_deref().map(|output| (receipt, output)))
            {
                let asset = marker
                    .photos_imported_assets
                    .iter()
                    .find(|asset| asset.rel_path == output);
                if let Some(asset) = asset {
                    ensure!(
                        marker
                            .blake3_log
                            .values()
                            .filter(|entry| entry.out_rel.as_deref() == Some(output))
                            .count()
                            == 1
                            && asset.blake3 == record.out,
                        "Photos receipt is not uniquely bound to this source's recorded output",
                    );
                }
                asset
            } else {
                None
            };
            let history = marker.photos_submission_history();
            let has_uncertainty = inspect_store_schema_version(conn)?
                .is_some_and(|version| version >= FAST_IMG_PHOTOS_UNCERTAINTY_SCHEMA_VERSION);
            let uncertain = if has_uncertainty {
                photos_uncertainty_evidence(conn, &key, &marker)?
            } else {
                Vec::new()
            };
            let photos_submissions = history
                .into_iter()
                .flat_map(|history| &history.intents)
                .filter(|intent| intent.source_rel == source)
                .map(|intent| {
                    Ok(SubmissionEvidence {
                        kind: intent.kind,
                        request_identity: request_identity(&intent.backend)?,
                        backend: match intent.backend {
                            PhotosSubmissionBackend::Native { .. } => "native",
                            PhotosSubmissionBackend::Applescript { .. } => "applescript",
                        },
                        source_blake3: intent.source_blake3.clone(),
                        output_relative: intent.output_rel.clone(),
                        output_blake3: intent.output_blake3.clone(),
                        applescript_uncertainty_recorded: (has_uncertainty
                            && matches!(
                                intent.backend,
                                PhotosSubmissionBackend::Applescript { .. }
                            ))
                        .then(|| uncertain.contains(intent)),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let cleanup_journal = if primary.is_some() && marker.primary_cleanup_journal {
                Some("primary")
            } else if original.is_some() && marker.tier2_cleanup_journal {
                Some("original")
            } else {
                None
            };
            let cleanup = cleanup_rows(conn, &key, source, cleanup_journal)?;
            let coverage = if admission.is_some()
                || retry_admission.is_some()
                || outcome.is_some()
                || !photos_submissions.is_empty()
            {
                "recorded"
            } else if plan.is_some() || retry.is_some() {
                "not_admitted"
            } else {
                "unknown"
            };
            let primary_result = primary_result::current(conn, &key, source, revision, &marker)?;
            Ok(SourceEvidence {
                schema_version: 1,
                scope: "historical_fast_img_source",
                source_relative: source.to_owned(),
                coverage,
                task_key: key,
                task_revision: revision,
                parent_task_key: retry
                    .as_ref()
                    .map(|retry| retry.parent_task_key().to_owned()),
                stage: marker.stage.clone(),
                admission,
                retry_parent_source_blake3: retry_admission
                    .map(|entry| entry.parent_source_blake3.clone()),
                retry_admitted_source_blake3: retry_admission
                    .map(|entry| entry.admitted_source_blake3.clone()),
                processing_contract: marker.processing_contract.clone(),
                tool_snapshot_identity: marker.tool_snapshot_identity.clone(),
                outcome,
                primary_result,
                photos_library_path: marker.photos_library_path.clone(),
                photos_receipt: primary_asset.or(original).cloned(),
                photos_submission_history_complete: history
                    .map(|history| history.complete_from_creation),
                photos_submissions,
                cleanup_journal,
                cleanup,
            })
        })
    })()
    .map_err(as_io)
}
