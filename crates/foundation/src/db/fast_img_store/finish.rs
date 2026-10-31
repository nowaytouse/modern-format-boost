//! Compare-and-finalize for a failed-only attempt whose source root is absent.
//! The sealed media proof is produced separately; this module touches only Core SQL.

use super::{
    FailedRetryContext, FailedRetryDeliveryIntent, FailedRetryFinishEvidence,
    FailedRetrySourceRootObservation, as_io, current_family, in_snapshot, key_for_canonical,
    matching_markers_in_snapshot, open_readonly, open_store_connection, read_snapshot,
    read_snapshot_base, retry, snapshot_digest, text, validate_locator,
    write_snapshot_at_with_finish,
};
use crate::image::fast_img::VerifiedFailedRetryFinish;
use crate::pipeline::verification::{FastImgStageName, WorkingCopyMarker};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, TransactionBehavior};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct FailedRetryFinishSnapshot {
    marker: WorkingCopyMarker,
    context: FailedRetryContext,
    task_key: String,
    snapshot: String,
    revision: i64,
    request: String,
    manifest: String,
}

impl FailedRetryFinishSnapshot {
    #[must_use]
    pub const fn marker(&self) -> &WorkingCopyMarker {
        &self.marker
    }
    #[must_use]
    pub const fn context(&self) -> &FailedRetryContext {
        &self.context
    }
    #[must_use]
    pub fn task_key(&self) -> &str {
        &self.task_key
    }
    #[must_use]
    pub fn snapshot_blake3(&self) -> &str {
        &self.snapshot
    }
    #[must_use]
    pub const fn revision(&self) -> i64 {
        self.revision
    }
    #[must_use]
    pub fn request_key(&self) -> &str {
        &self.request
    }
    #[must_use]
    pub fn manifest_blake3(&self) -> &str {
        &self.manifest
    }
}

fn require_gate1(marker: &WorkingCopyMarker) -> Result<()> {
    let checks = &marker.gate1_checks;
    ensure!(
        checks.count.0
            && checks.blake3.0
            && checks.metadata.0
            && checks.size.0
            && checks.orient.0
            && checks.decode.0,
        "Finish-only recovery requires every applicable local Gate 1 check"
    );
    Ok(())
}

/// Select only one exact saved child; no media path or timestamp decides ownership.
///
/// # Errors
/// Rejects invalid locators, damaged state and multiple children, including completed ones.
pub fn resolve_failed_retry_child_for_source(
    source: &Path,
    strategy: &str,
) -> io::Result<Option<WorkingCopyMarker>> {
    (|| -> Result<_> {
        let source = text(source)?;
        validate_locator(source)?;
        ensure!(
            matches!(strategy, "jxl" | "avif"),
            "Failed-only child selection requires a supported exact strategy"
        );
        let Some(conn) = open_readonly()? else {
            return Ok(None);
        };
        in_snapshot(&conn, |conn| {
            if !current_family(conn)? {
                return Ok(None);
            }
            let markers = matching_markers_in_snapshot(conn, source, strategy, |conn, path| {
                read_snapshot(conn, &key_for_canonical(path))
            })?;
            let mut selected = None;
            for marker in markers {
                // read_snapshot has already verified the complete retry context,
                // including refusal of a missing relation behind this anchor.
                if marker.failed_retry_manifest_blake3.is_none() {
                    continue;
                }
                ensure!(
                    selected.is_none(),
                    "MFB_RETRY_SELECTION_AMBIGUOUS: multiple saved failed-only children match this exact source and strategy; select one explicitly with --output"
                );
                selected = Some(marker);
            }
            Ok(selected)
        })
    })()
    .map_err(as_io)
}

fn require_eligible(
    marker: &WorkingCopyMarker,
    delivery: &FailedRetryDeliveryIntent,
) -> Result<()> {
    ensure!(
        !marker.is_terminal()
            && marker.failed_retry_finish_evidence().is_none()
            && marker.failed_retry_manifest_blake3.is_some(),
        "Finish-only recovery requires an unfinished manifest-bound child"
    );
    ensure!(
        marker.failed_sources.is_empty() && marker.skipped_sources.is_empty(),
        "Finish-only recovery cannot confirm missing failed or retained sources"
    );
    let converted = marker.blake3_log.keys().collect::<BTreeSet<_>>();
    let originals = marker
        .tier2_imported_assets
        .iter()
        .map(|asset| &asset.rel_path)
        .collect::<BTreeSet<_>>();
    ensure!(
        converted.is_disjoint(&originals)
            && converted.len().checked_add(originals.len()) == Some(marker.src_jpeg_count)
            && marker.src_jpeg_count > 0,
        "Finish-only recovery requires an exact, nonempty successful source set"
    );
    match delivery {
        FailedRetryDeliveryIntent::Local => {
            ensure!(
                marker.stage == FastImgStageName::Gate1Passed
                    && !marker.tier2_in_progress
                    && marker.tier2_imported_assets.is_empty()
                    && marker.photos_imported_assets.is_empty(),
                "Local finish-only recovery requires the existing local Gate 1 boundary"
            );
            require_gate1(marker)?;
        }
        FailedRetryDeliveryIntent::Photos { .. } => {
            ensure!(
                marker.stage == FastImgStageName::Gate3Passed
                    || (marker.stage == FastImgStageName::CleanupComplete
                        && marker.tier2_in_progress),
                "Photos finish-only recovery requires the existing final delivery boundary"
            );
            ensure!(
                marker.photos_library_path.is_some()
                    && marker.photos_imported_assets.len() == marker.blake3_log.len(),
                "Photos finish-only recovery requires exact bound primary receipts"
            );
            if !marker.blake3_log.is_empty() {
                require_gate1(marker)?;
                let gate2 = &marker.gate2_checks;
                let gate3 = &marker.gate3_checks;
                ensure!(
                    gate2.count.0
                        && gate2.blake3_sample.0
                        && gate2.no_error.0
                        && gate3.count_x3.0
                        && gate3.sync.0
                        && gate3.quarantine.0
                        && gate3.chain.0,
                    "Photos finish-only recovery cannot replace missing Gate 2/3 evidence"
                );
            }
        }
    }
    Ok(())
}

fn require_admission_coverage(
    marker: &WorkingCopyMarker,
    context: &FailedRetryContext,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (rel, entry) in &marker.blake3_log {
        let admission = context
            .admissions()
            .get(rel)
            .context("Finish output is outside the immutable failed-only set")?;
        ensure!(
            entry.src == admission.admitted_source_blake3 && seen.insert(rel),
            "Finish output has a conflicting source identity"
        );
    }
    for asset in &marker.tier2_imported_assets {
        let admission = context
            .admissions()
            .get(&asset.rel_path)
            .context("Finish original receipt is outside the immutable failed-only set")?;
        ensure!(
            asset.blake3 == admission.admitted_source_blake3 && seen.insert(&asset.rel_path),
            "Finish original receipt has a conflicting source identity"
        );
    }
    ensure!(
        seen.len() == context.admissions().len(),
        "Finish-only recovery cannot omit uncompleted admitted media"
    );
    Ok(())
}

fn require_counts(marker: &WorkingCopyMarker, evidence: &FailedRetryFinishEvidence) -> Result<()> {
    let outputs = u64::try_from(marker.blake3_log.len())?;
    let primary = u64::try_from(marker.photos_imported_assets.len())?;
    let original = u64::try_from(marker.tier2_imported_assets.len())?;
    ensure!(
        evidence.verified_primary_photos_count == primary
            && evidence.verified_original_photos_count == original
            && evidence.verified_output_count == outputs,
        "Finish-only observation counts differ from the exact verified media set"
    );
    Ok(())
}

impl FailedRetryFinishSnapshot {
    /// Check this immutable snapshot's Gate boundary and exact successful admission set.
    pub(crate) fn validate_eligibility(&self) -> Result<()> {
        require_eligible(self.marker(), self.context().requested_delivery())?;
        require_admission_coverage(self.marker(), self.context())
    }
}

/// Inspect a prospective finish boundary without accessing media paths or changing storage.
///
/// # Errors
/// Rejects damaged state and broken lineage. Media verification determines finish eligibility.
pub fn inspect_failed_retry_finish(
    child_wc: &Path,
) -> io::Result<Option<FailedRetryFinishSnapshot>> {
    (|| -> Result<_> {
        validate_locator(text(child_wc)?)?;
        let Some(conn) = open_readonly()? else {
            return Ok(None);
        };
        in_snapshot(&conn, |conn| {
            if !current_family(conn)? {
                return Ok(None);
            }
            let key = key_for_canonical(text(child_wc)?);
            let Some(marker) = read_snapshot_base(conn, &key)? else {
                return Ok(None);
            };
            let context = retry::context_for(conn, &key, &marker)?;
            let Some(context) = context else {
                return Ok(None);
            };
            if marker.is_terminal() {
                return Ok(None);
            }
            let (revision, bytes, request): (i64, Vec<u8>, String) = conn.query_row(
                "SELECT t.revision,t.snapshot_blake3,r.request_key FROM fast_img_tasks t
                 JOIN fast_img_retry_tasks r ON r.task_key=t.task_key WHERE t.task_key=?1",
                [&key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let digest =
                snapshot_digest(text(&marker.working_copy)?, text(&marker.src_dir)?, &marker)?;
            ensure!(
                revision >= 0 && digest.as_bytes().as_slice() == bytes,
                "Finish-only snapshot changed within its read transaction"
            );
            Ok(Some(FailedRetryFinishSnapshot {
                manifest: marker
                    .failed_retry_manifest_blake3
                    .clone()
                    .context("Finish snapshot lost its retry manifest")?,
                marker,
                context,
                task_key: key,
                snapshot: digest.to_hex().to_string(),
                revision,
                request,
            }))
        })
    })()
    .map_err(as_io)
}

pub(super) fn validate_write(
    old: &WorkingCopyMarker,
    next: &WorkingCopyMarker,
    revision: i64,
    finalization: Option<(&FailedRetryFinishSnapshot, &FailedRetryFinishEvidence)>,
) -> Result<()> {
    let Some((expected, evidence)) = finalization else {
        ensure!(
            old.failed_retry_finish_evidence() == next.failed_retry_finish_evidence(),
            "Only verified compare-and-finalize may establish or change finish-only evidence"
        );
        ensure!(
            old.failed_retry_finish_evidence().is_none() || old == next,
            "A finalized failed-only observation is immutable"
        );
        return Ok(());
    };
    ensure!(
        old == expected.marker()
            && revision == expected.revision()
            && snapshot_digest(text(&old.working_copy)?, text(&old.src_dir)?, old)?
                .to_hex()
                .as_str()
                == expected.snapshot_blake3(),
        "MFB_FINISH_STATE_CHANGED: saved task changed after verification; no finalization committed"
    );
    require_eligible(old, expected.context().requested_delivery())?;
    require_admission_coverage(old, expected.context())?;
    ensure!(
        evidence.schema == 1
            && evidence.task_key == expected.task_key()
            && evidence.request_key == expected.request_key()
            && evidence.manifest_blake3 == expected.manifest_blake3()
            && evidence.verified_snapshot_blake3 == expected.snapshot_blake3()
            && evidence.verified_revision == expected.revision()
            && evidence.verified_stage == old.stage
            && evidence.verified_tier2_in_progress == old.tier2_in_progress
            && evidence.verified_error == old.error
            && evidence.source_root_observation == FailedRetrySourceRootObservation::NotFound,
        "Finish-only proof does not bind the exact previously verified task"
    );
    require_counts(old, evidence)?;
    let mut permitted = old.clone();
    permitted.stage = FastImgStageName::CleanupComplete;
    permitted.tier2_in_progress = false;
    permitted.error = None;
    permitted.failed_retry_finish_evidence = Some(evidence.clone());
    ensure!(
        &permitted == next,
        "Finish-only recovery cannot change Gate checks, identities, outcomes or custody receipts"
    );
    Ok(())
}

/// Commit only the sealed, freshly verified observation if the complete old state is unchanged.
/// No source/output path is read, created, imported or removed by this transaction.
///
/// # Errors
/// Rejects stale proofs, missing state, changed parents and every transactional write failure.
pub fn finalize_failed_retry_finish(
    expected: &FailedRetryFinishSnapshot,
    proof: VerifiedFailedRetryFinish,
) -> io::Result<WorkingCopyMarker> {
    (|| -> Result<_> {
        let evidence = proof.into_evidence();
        let mut next = expected.marker().clone();
        next.stage = FastImgStageName::CleanupComplete;
        next.tier2_in_progress = false;
        next.error = None;
        next.failed_retry_finish_evidence = Some(evidence.clone());
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_snapshot_at_with_finish(
            &tx,
            &next,
            text(&next.working_copy)?,
            text(&next.src_dir)?,
            true,
            Some((expected, &evidence)),
        )?;
        ensure!(
            read_snapshot(&tx, expected.task_key())?.as_ref() == Some(&next),
            "Finish-only confirmation readback differs; transaction must roll back"
        );
        tx.commit()
            .context("Finish-only confirmation was not committed; prior task retained")?;
        Ok(next)
    })()
    .map_err(as_io)
}

pub(super) fn validate_persisted(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
    revision: i64,
) -> Result<()> {
    let Some(evidence) = marker.failed_retry_finish_evidence() else {
        return Ok(());
    };
    ensure!(
        marker.is_terminal()
            && marker.error.is_none()
            && evidence.task_key == key
            && evidence.verified_revision.checked_add(1) == Some(revision)
            && marker.failed_retry_manifest_blake3.as_ref() == Some(&evidence.manifest_blake3),
        "Saved finish-only observation has a conflicting task, revision or final state"
    );
    let (request, manifest, delivery): (String, Vec<u8>, String) = conn.query_row(
        "SELECT request_key,manifest_blake3,requested_delivery FROM fast_img_retry_tasks WHERE task_key=?1",
        [key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    ensure!(
        request == evidence.request_key
            && manifest.as_slice()
                == blake3::Hash::from_hex(&evidence.manifest_blake3)?
                    .as_bytes()
                    .as_slice(),
        "Saved finish-only observation lost its immutable retry relation"
    );
    let delivery: FailedRetryDeliveryIntent = serde_json::from_str(&delivery)?;
    let mut prior = marker.clone();
    prior.stage.clone_from(&evidence.verified_stage);
    prior.tier2_in_progress = evidence.verified_tier2_in_progress;
    prior.error.clone_from(&evidence.verified_error);
    prior.failed_retry_finish_evidence = None;
    require_eligible(&prior, &delivery)?;
    require_counts(&prior, evidence)?;
    ensure!(
        snapshot_digest(text(&prior.working_copy)?, text(&prior.src_dir)?, &prior)?
            .to_hex()
            .as_str()
            == evidence.verified_snapshot_blake3,
        "Saved finish-only observation cannot reconstruct its verified old snapshot"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
