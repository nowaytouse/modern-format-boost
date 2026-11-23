//! Keep delivery facts separate from finalization errors and the process exit.
#![expect(
    clippy::redundant_pub_crate,
    reason = "CLI result types remain internal; public visibility conflicts with unreachable_pub"
)]
use foundation::ModernLossyStaticCandidate;
#[cfg(test)]
use foundation::common_utils::calculate_blake3_hash;
use foundation::fast_img::PhotosQuarantineLocations;
use foundation::fast_img_store::{
    FailedRetryAdmission, FailedRetryContext, FailedRetrySourceRoute, verify_failed_retry_admission,
};
use foundation::pipeline::verification::{
    FastImgStageName, GateResult, WorkingCopyMarker, gate1_complete_or_later,
    gate3_complete_or_later, retained_source_blake3, write_marker_atomic,
};
use foundation::unified_error::PhotosDeliveryDeferred;
use std::collections::{BTreeMap, BTreeSet};

/// Current scan evidence only; never an import, cleanup, or retry admission.
#[derive(Debug, Default)]
pub(super) struct ProbeFailures {
    pub(super) total: usize,
    pub(super) hashes: BTreeMap<String, String>,
}

impl ProbeFailures {
    pub(super) const fn new(total: usize) -> Self {
        Self {
            total,
            hashes: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
pub(super) struct FileFailures {
    total: usize,
    originals: usize,
}

impl FileFailures {
    pub(super) const fn primary(total: usize) -> Self {
        Self {
            total,
            originals: 0,
        }
    }
    pub(super) const fn originals(total: usize) -> Self {
        Self {
            total,
            originals: total,
        }
    }
}

impl std::fmt::Display for FileFailures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fast-img {} recorded {} failed source(s); see the result report for source retention",
            if self.originals != 0 {
                "original-media delivery"
            } else {
                "batch"
            },
            self.total
        )
    }
}
impl std::error::Error for FileFailures {}

#[derive(Debug)]
pub(super) struct PhotosQuarantineBlocked {
    pub locations: PhotosQuarantineLocations,
    pub detail: String,
}

impl std::fmt::Display for PhotosQuarantineBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for PhotosQuarantineBlocked {}

pub(super) fn final_quarantine_count(result: &GateResult) -> Option<usize> {
    let required = [
        "count",
        "blake3_sample",
        "no_error",
        "count_x3",
        "sync",
        "quarantine",
        "chain",
    ];
    if result.passed || !result.checks.iter().map(|check| check.name).eq(required) {
        return None;
    }
    let mut failed = result.checks.iter().filter(|check| !check.passed);
    let quarantine = failed.next()?;
    let count = quarantine.affected_files.len();
    (quarantine.name == "quarantine" && count > 0 && failed.next().is_none()).then_some(count)
}

#[derive(Debug, serde::Serialize)]
struct FailedSourceProof {
    invocation_id: String,
    verified_relative_paths: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct OriginalDelivery {
    photos_verified: usize,
    source_cleanup_confirmed: usize,
}

#[derive(Debug, serde::Serialize)]
struct Report {
    schema_version: u8,
    source_directory: std::path::PathBuf,
    output_directory: std::path::PathBuf,
    photos_delivery_deferred: bool,
    photos_quarantine_blocked: Option<usize>,
    photos_quarantine_locations: Option<PhotosQuarantineLocations>,
    primary_verification_complete: bool,
    succeeded: usize,
    skipped: usize,
    ignored: usize,
    failed: usize,
    unprocessed: usize,
    encoded: usize,
    photos_verified: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_delivery: Option<OriginalDelivery>,
    source_retained: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failures_outside_retry_inventory: Option<usize>,
    failed_sources_retained: Option<usize>,
    failed_sources_verified: Option<usize>,
    failed_source_verification_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_source_proof: Option<FailedSourceProof>,
    retention_unknown_reason: Option<String>,
    primary_cleanup_complete: bool,
    retained_only_complete: bool,
    finalization_error: Option<String>,
    #[serde(skip)]
    settled_original_failures: Option<usize>,
}

fn verify_retained_source(root: &std::path::Path, rel: &str, hash: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        retained_source_blake3(root, std::path::Path::new(rel))? == hash,
        "retained source no longer matches its recorded content hash: {rel}"
    );
    Ok(())
}

impl Report {
    #[cfg(test)]
    fn from_marker(
        marker: &WorkingCopyMarker,
        photos: bool,
        originals: &[ModernLossyStaticCandidate],
        probe_failures: usize,
        recorded_failures: Option<&FileFailures>,
    ) -> anyhow::Result<Self> {
        Self::from_execution(
            marker,
            None,
            photos,
            originals,
            &ProbeFailures::new(probe_failures),
            recorded_failures,
            None,
        )
    }

    fn from_execution(
        marker: &WorkingCopyMarker,
        admissions: Option<&BTreeMap<String, FailedRetryAdmission>>,
        photos: bool,
        originals: &[ModernLossyStaticCandidate],
        probe_failures: &ProbeFailures,
        recorded_failures: Option<&FileFailures>,
        invocation_id: Option<&str>,
    ) -> anyhow::Result<Self> {
        marker
            .validate_source_disposition_disjoint()
            .map_err(anyhow::Error::msg)?;
        marker
            .validate_relative_path_contract()
            .map_err(anyhow::Error::msg)?;
        marker
            .validate_library_asset_receipts_unique()
            .map_err(anyhow::Error::msg)?;
        let checked_add = |left: usize, right: usize, count: &str| {
            left.checked_add(right)
                .ok_or_else(|| anyhow::anyhow!("fast-img {count} count overflow"))
        };
        let routed = admissions.filter(|_| marker.failed_retry_primary_source_count.is_some());
        anyhow::ensure!(
            marker.failed_retry_primary_source_count.is_none() || routed.is_some(),
            "fast-img result has no immutable retry inventory"
        );
        if let Some(admissions) = routed {
            anyhow::ensure!(
                admissions.len() == marker.src_jpeg_count,
                "fast-img result lost its immutable retry inventory"
            );
            for (relative, hash) in originals.iter().map(|item| (&item.rel_path, &item.blake3)) {
                anyhow::ensure!(
                    admissions.get(relative).is_some_and(|admission| {
                        admission.route == Some(FailedRetrySourceRoute::Original)
                            && &admission.admitted_source_blake3 == hash
                    }),
                    "fast-img original result candidate falls outside the exact retry admission: {relative}"
                );
            }
        }
        let tier2 = originals
            .iter()
            .map(|c| c.rel_path.as_str())
            .chain(
                marker
                    .tier2_imported_assets
                    .iter()
                    .map(|a| a.rel_path.as_str()),
            )
            .chain(marker.tier2_failed_sources.keys().map(String::as_str))
            .chain(marker.photos_submission_history().into_iter().flat_map(|history| {
                history.intents.iter().filter_map(|intent| {
                    (intent.kind == foundation::pipeline::verification::PhotosSubmissionKind::Original)
                        .then_some(intent.source_rel.as_str())
                })
            }))
            .collect::<BTreeSet<_>>();
        let delivered = if photos {
            gate3_complete_or_later(&marker.stage)
        } else {
            gate1_complete_or_later(&marker.stage)
        };
        let primary_succeeded = if delivered {
            marker.blake3_log.len()
        } else {
            0
        };
        let original_photos_verified = marker.tier2_imported_assets.len();
        anyhow::ensure!(
            photos || original_photos_verified == 0,
            "local delivery has original-format Photos receipts"
        );
        let original_cleanup = if original_photos_verified == 0 {
            Ok(0)
        } else if marker.is_terminal() {
            foundation::fast_img_store::verify_tier2_cleanup_complete(marker)
                .map(|()| original_photos_verified)
        } else {
            foundation::fast_img_store::count_recorded_tier2_unlinks(marker)
        };
        let original_cleanup_confirmed = if let Ok(count) = original_cleanup {
            count
        } else {
            tracing::warn!("could not establish the original-format source cleanup count");
            0
        };
        anyhow::ensure!(
            original_cleanup_confirmed <= original_photos_verified,
            "original-format cleanup exceeds verified Photos receipts"
        );
        let succeeded = checked_add(primary_succeeded, original_cleanup_confirmed, "succeeded")?;
        // Count only explicit file failures; unresolved originals stay unprocessed.
        let anonymous_original_failures = match (routed, recorded_failures) {
            (None, Some(failures)) => failures
                .originals
                .saturating_sub(marker.tier2_failed_sources.len()),
            _ => 0,
        };
        let unbound_failures = if routed.is_some() {
            0
        } else {
            probe_failures.total
        };
        let failed = checked_add(
            checked_add(marker.failed_source_count(), unbound_failures, "failed")?,
            anonymous_original_failures,
            "failed",
        )?;
        let skipped = marker.skipped_sources.len();
        let total = if routed.is_some() {
            marker.src_jpeg_count
        } else {
            checked_add(
                checked_add(marker.src_jpeg_count, tier2.len(), "inventory")?,
                probe_failures.total,
                "inventory",
            )?
        };
        let dispositions = checked_add(
            checked_add(succeeded, failed, "disposition")?,
            skipped,
            "disposition",
        )?;
        let unprocessed = total.checked_sub(dispositions).ok_or_else(|| {
            anyhow::anyhow!("fast-img result dispositions exceed input inventory")
        })?;
        let photos_verified = checked_add(
            marker.photos_imported_assets.len(),
            original_photos_verified,
            "Photos-verified",
        )?;
        let sources = marker
            .blake3_log
            .keys()
            .map(String::as_str)
            .chain(marker.failed_sources.keys().map(String::as_str))
            .chain(marker.skipped_sources.keys().map(String::as_str))
            .chain(tier2)
            .chain(
                routed
                    .into_iter()
                    .flat_map(|admissions| admissions.keys().map(String::as_str)),
            )
            .collect::<BTreeSet<_>>();
        let is_failed = |rel: &str| {
            marker.failed_sources.contains_key(rel) || marker.tier2_failed_sources.contains_key(rel)
        };
        let mut checked_source_parents = BTreeSet::new();
        let retention = sources
            .iter()
            .try_fold((0usize, 0usize), |(count, failures), rel| {
                let path = std::path::Path::new(rel);
                if let Some(parent) = path.parent()
                    && checked_source_parents.insert(parent)
                {
                    foundation::pipeline::verification::validate_no_symlink_components(
                        &marker.src_dir,
                        parent,
                    )
                    .map_err(|_| {
                        std::io::Error::other("an inventory source parent could not be verified")
                    })?;
                }
                match std::fs::symlink_metadata(marker.src_dir.join(path)) {
                    Ok(metadata) if metadata.is_file() => {
                        Ok((count + 1, failures + usize::from(is_failed(rel))))
                    }
                    Ok(_) => Err(std::io::Error::other(
                        "an inventory source path is no longer a regular file",
                    )),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Ok((count, failures))
                    }
                    Err(error) => Err(error),
                }
            });
        let mut failed_sources_retained = match &retention {
            Ok((_, count)) if unbound_failures == 0 && anonymous_original_failures == 0 => {
                Some(*count)
            }
            _ => None,
        };

        let mut verified_relative_paths = Vec::new();
        let failed_verification = (|| -> anyhow::Result<usize> {
            anyhow::ensure!(
                anonymous_original_failures == 0,
                "failed sources are not all bound to recorded source hashes"
            );
            let probe_hashes = (routed.is_none()).then_some(&probe_failures.hashes);
            anyhow::ensure!(
                unbound_failures == probe_hashes.map_or(0, BTreeMap::len),
                "failed sources are not all bound to recorded source hashes"
            );
            if let Some(probes) = probe_hashes {
                for (rel, hash) in probes {
                    anyhow::ensure!(
                        !sources.contains(rel.as_str()),
                        "probe failure overlaps another source disposition: {rel}"
                    );
                    verify_retained_source(&marker.src_dir, rel, hash)?;
                }
            }
            for (rel, entry) in marker.all_failed_sources() {
                if let Some(admissions) = routed {
                    let admission = admissions.get(rel).ok_or_else(|| {
                        anyhow::anyhow!(
                            "fast-img retained failure escaped its retry admission: {rel}"
                        )
                    })?;
                    anyhow::ensure!(
                        admission.admitted_source_blake3 == entry.src,
                        "fast-img retained failure changed its retry identity: {rel}"
                    );
                    verify_failed_retry_admission(&marker.src_dir, admission)?;
                } else {
                    verify_retained_source(&marker.src_dir, rel, &entry.src)?;
                }
                if verified_relative_paths.len() < 8 && rel.len() <= 256 {
                    verified_relative_paths.push(rel.clone());
                }
            }
            // Every counted failure has now passed an independent retained-source check.
            Ok(failed)
        })();
        let (failed_sources_verified, failed_source_verification_error) = match failed_verification
        {
            Ok(count) => (Some(count), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        if failed_sources_verified == Some(failed)
            && anonymous_original_failures == 0
            && retention.is_ok()
        {
            failed_sources_retained = Some(failed);
        }
        // Probe failures live outside the retry checkpoint. Count their originals only
        // after the disjoint paths and their current bytes have all been verified.
        let (mut source_retained, mut retention_unknown_reason) = match retention {
            Err(error) => (
                None,
                Some(format!("source existence inspection failed: {error}")),
            ),
            Ok(_) if probe_failures.total > 0
                && (routed.is_some() || failed_sources_verified != Some(failed)) => (
                None,
                Some("source inventory includes probe failures without complete retention evidence".into()),
            ),
            Ok(_) if anonymous_original_failures > 0
                || (routed.is_none() && !marker.source_disposition_is_complete()) => (
                None,
                Some("source inventory has unclassified or unaccounted entries".into()),
            ),
            Ok((count, _)) => (Some(checked_add(count, unbound_failures, "retained source")?), None),
        };
        let failed_source_proof = if probe_failures.total > 0 || verified_relative_paths.is_empty()
        {
            None
        } else {
            invocation_id
                .filter(|id| {
                    !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_graphic())
                })
                .map(|invocation_id| FailedSourceProof {
                    invocation_id: invocation_id.to_owned(),
                    verified_relative_paths,
                })
        };
        let retained_only_complete =
            recorded_failures.is_some_and(|failures| {
                failures.total == failed
                    && failed > 0
                    && succeeded == 0
                    && unprocessed == 0
                    && marker.blake3_log.is_empty()
                    && photos_verified == 0
                    && !marker.tier2_in_progress
                    && matches!(
                        marker.stage,
                        FastImgStageName::TranscodeComplete | FastImgStageName::Gate1Failed
                    )
                    && failed_sources_verified == Some(failed)
                    && source_retained == Some(dispositions)
            }) && match marker.skipped_sources.iter().try_for_each(|(rel, entry)| {
                verify_retained_source(&marker.src_dir, rel, &entry.src)
            }) {
                Ok(()) => true,
                Err(error) => {
                    source_retained = None;
                    retention_unknown_reason = Some(format!("{error:#}"));
                    false
                }
            };
        Ok(Self {
            schema_version: 1,
            source_directory: std::path::absolute(&marker.src_dir)?,
            output_directory: std::path::absolute(&marker.working_copy)?,
            photos_delivery_deferred: false,
            photos_quarantine_blocked: None,
            photos_quarantine_locations: None,
            primary_verification_complete: gate1_complete_or_later(&marker.stage),
            succeeded,
            skipped,
            ignored: 0,
            failed,
            unprocessed,
            encoded: marker.blake3_log.len(),
            photos_verified,
            original_delivery: (original_photos_verified > 0).then_some(OriginalDelivery {
                photos_verified: original_photos_verified,
                source_cleanup_confirmed: original_cleanup_confirmed,
            }),
            source_retained,
            failures_outside_retry_inventory: (unbound_failures > 0).then_some(unbound_failures),
            failed_sources_retained,
            failed_sources_verified,
            failed_source_verification_error,
            failed_source_proof,
            retention_unknown_reason,
            primary_cleanup_complete: marker.stage == FastImgStageName::CleanupComplete,
            retained_only_complete,
            finalization_error: None,
            settled_original_failures: (marker.is_terminal() && anonymous_original_failures == 0)
                .then_some(marker.tier2_failed_sources.len()),
        })
    }

    fn record_execution(&mut self, execution: &anyhow::Result<()>, photos: bool) {
        self.photos_delivery_deferred = photos
            && self.primary_verification_complete
            && !self.primary_cleanup_complete
            && self.unprocessed > 0
            && execution.as_ref().err().is_some_and(|error| {
                error
                    .chain()
                    .any(<dyn std::error::Error>::is::<PhotosDeliveryDeferred>)
            });
        self.photos_quarantine_locations = execution
            .as_ref()
            .err()
            .filter(|error| error.chain().count() == 1)
            .and_then(|error| error.downcast_ref::<PhotosQuarantineBlocked>())
            .map(|error| error.locations)
            .filter(|locations| {
                photos
                    && self.succeeded == 0
                    && self.failed == 0
                    && locations.affected > 0
                    && locations.affected <= self.unprocessed
                    && self.encoded == self.unprocessed
                    && self.photos_verified == self.unprocessed
                    && !self.primary_cleanup_complete
                    && self.source_retained.is_some()
                    && self.skipped.checked_add(self.unprocessed) == self.source_retained
            });
        self.photos_quarantine_blocked = self
            .photos_quarantine_locations
            .map(|locations| locations.affected);
        // A typed final Gate 2 result includes a fresh local recheck even after the checkpoint rolls back.
        self.primary_verification_complete |= self.photos_quarantine_blocked.is_some();
        self.finalization_error = execution.as_ref().err().and_then(|error| {
            let isolated_file_failures =
                error
                    .downcast_ref::<FileFailures>()
                    .is_some_and(|failures| {
                        error.chain().count() == 1
                            && (failures.total == self.failed
                                || (failures.originals > 0
                                    && self.settled_original_failures == Some(failures.originals)))
                            && self.unprocessed == 0
                            && (self.primary_cleanup_complete || self.retained_only_complete)
                            && self.failed_sources_verified == Some(self.failed)
                            && (!photos || self.photos_verified == self.succeeded)
                    });
            (!isolated_file_failures).then(|| format!("{error:#}"))
        });
        self.retained_only_complete &= self.finalization_error.is_none() && execution.is_err();
    }
}

pub(super) fn finish(
    marker: &WorkingCopyMarker,
    retry: Option<&FailedRetryContext>,
    photos: bool,
    originals: &[ModernLossyStaticCandidate],
    probe_failures: &ProbeFailures,
    ignored: usize,
    execution: anyhow::Result<()>,
) -> anyhow::Result<()> {
    let execution = execution.and_then(|()| {
        anyhow::ensure!(
            !marker.tier2_in_progress,
            "fast-img original-format delivery or source cleanup remains pending"
        );
        Ok(())
    });
    let recorded_failures = execution
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<FileFailures>());
    let invocation_id = std::env::var("MFB_INVOCATION_ID").ok();
    let mut report = Report::from_execution(
        marker,
        retry.map(FailedRetryContext::admissions),
        photos,
        originals,
        probe_failures,
        recorded_failures,
        invocation_id.as_deref(),
    )
    .map_err(|error| match execution.as_ref() {
        Ok(()) => error,
        Err(execution) => error.context(format!("execution also failed: {execution:#}")),
    })?;
    report.record_execution(&execution, photos);
    report.ignored = ignored;
    let mut payload = serde_json::to_string(&report)?;
    if payload.len() > 16_384 && report.failed_source_proof.take().is_some() {
        payload = serde_json::to_string(&report)?;
    }
    println!("MFB_FAST_IMG_RESULT={payload}");
    print_processor_counts(
        report.succeeded,
        report.skipped,
        report.failed,
        report.ignored,
        report.unprocessed,
    );
    println!(
        "[RESULT  ] encoded={} Photos-verified={} unprocessed={} primary-cleanup-complete={}",
        report.encoded, report.photos_verified, report.unprocessed, report.primary_cleanup_complete
    );
    if let Some(original) = &report.original_delivery {
        println!(
            "[RESULT  ] Original-format Photos verified={} source-cleanup-confirmed={} source-cleanup-unconfirmed={}",
            original.photos_verified,
            original.source_cleanup_confirmed,
            original.photos_verified - original.source_cleanup_confirmed
        );
    }
    execution?;
    if report.unprocessed != 0 {
        anyhow::bail!(
            "fast-img left {} input(s) unprocessed; inspect the delivery and source cleanup receipts",
            report.unprocessed
        );
    }
    if report.failed != 0 {
        return Err(FileFailures::primary(report.failed).into());
    }
    Ok(())
}

pub(super) fn print_processor_counts(
    succeeded: usize,
    skipped: usize,
    failed: usize,
    ignored: usize,
    unprocessed: usize,
) {
    println!(
        "Succeeded: {succeeded}\nSkipped: {skipped}\nFailed: {failed}\nIgnored: {ignored}\nUnprocessed: {unprocessed}"
    );
}

pub(super) fn finalize_sources<T, U>(
    marker: &mut WorkingCopyMarker,
    has_tier2: bool,
    primary: impl FnOnce(&WorkingCopyMarker) -> anyhow::Result<T>,
    tier2: impl FnOnce(&mut WorkingCopyMarker) -> anyhow::Result<U>,
) -> anyhow::Result<(T, U)> {
    let primary_result = primary(marker)?;
    // Persist primary completion before attempting a separate import transaction.
    marker.stage = FastImgStageName::CleanupComplete;
    marker.error = None;
    marker.tier2_in_progress |= has_tier2;
    write_marker_atomic(marker)?;
    Ok((primary_result, tier2(marker)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use foundation::pipeline::verification::{Blake3Entry, SkippedSourceEntry};

    fn partial_marker(root: &std::path::Path, successes: usize) -> WorkingCopyMarker {
        let mut marker =
            WorkingCopyMarker::new(root.join("source"), root.join("output"), successes + 1);
        marker.stage = FastImgStageName::Gate3Passed;
        for index in 0..successes {
            marker.blake3_log.insert(
                format!("{index}.jpg"),
                Blake3Entry {
                    out_rel: Some(format!("{index}.jxl")),
                    src: format!("source-{index}"),
                    out: format!("output-{index}"),
                    library_asset: Some(format!("output-{index}")),
                },
            );
        }
        marker.failed_sources.insert(
            "invalid.jpg".into(),
            SkippedSourceEntry {
                src: "invalid-source".into(),
                reason: "JPEG missing EOI".into(),
            },
        );
        marker
    }

    #[test]
    fn partial_success_counts_survive_finalization_errors() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        for successes in [100, 1163] {
            let marker = partial_marker(root.path(), successes);
            let report = Report::from_marker(&marker, true, &[], 0, None)?;
            assert_eq!(
                (
                    report.succeeded,
                    report.skipped,
                    report.failed,
                    report.unprocessed
                ),
                (successes, 0, 1, 0)
            );
            let per_file = finish(
                &marker,
                None,
                true,
                &[],
                &ProbeFailures::default(),
                0,
                Ok(()),
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("file failure must produce an error"))?;
            assert!(per_file.is::<FileFailures>());
            let infrastructure = finish(
                &marker,
                None,
                true,
                &[],
                &ProbeFailures::default(),
                0,
                Err(anyhow::anyhow!("backend unavailable")),
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("infrastructure failure must produce an error"))?;
            assert!(!infrastructure.is::<FileFailures>());
            assert!(infrastructure.to_string().contains("backend unavailable"));
        }
        Ok(())
    }

    #[test]
    fn probe_failure_proof_requires_current_unique_bytes() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut marker =
            WorkingCopyMarker::new(root.path().join("input"), root.path().join("output"), 0);
        std::fs::create_dir_all(&marker.src_dir)?;
        let path = marker.src_dir.join("broken.avif");
        std::fs::write(&path, b"original")?;
        let hashes = BTreeMap::from([("broken.avif".to_owned(), calculate_blake3_hash(&path)?)]);
        let build = |marker: &WorkingCopyMarker, count, hashes: &BTreeMap<String, String>| {
            Report::from_execution(
                marker,
                None,
                false,
                &[],
                &ProbeFailures {
                    total: count,
                    hashes: hashes.clone(),
                },
                None,
                Some("worker-1"),
            )
        };
        let report = build(&marker, 1, &hashes)?;
        assert_eq!(report.failed_sources_verified, Some(1));
        assert_eq!(report.failed_sources_retained, Some(1));
        assert_eq!(report.failures_outside_retry_inventory, Some(1));
        assert!(report.failed_source_proof.is_none());
        assert_eq!(report.source_retained, Some(1));
        assert!(report.retention_unknown_reason.is_none());
        let incomplete = build(&marker, 2, &hashes)?;
        assert!(incomplete.failed_sources_verified.is_none());
        assert!(incomplete.source_retained.is_none());
        std::fs::write(&path, b"changed")?;
        let changed = build(&marker, 1, &hashes)?;
        assert!(changed.failed_sources_verified.is_none());
        assert!(changed.source_retained.is_none());
        std::fs::write(&path, b"original")?;
        marker.src_jpeg_count = 1;
        marker.failed_sources.insert(
            "broken.avif".into(),
            foundation::pipeline::verification::SkippedSourceEntry {
                src: hashes["broken.avif"].clone(),
                reason: "old failure".into(),
            },
        );
        let overlap = build(&marker, 1, &hashes)?;
        assert!(overlap.failed_sources_verified.is_none());
        assert!(overlap.source_retained.is_none());

        let recorded = marker
            .failed_sources
            .remove("broken.avif")
            .context("recorded failure")?;
        std::fs::write(marker.src_dir.join("another.avif"), b"original")?;
        marker
            .failed_sources
            .insert("another.avif".into(), recorded);
        let disjoint = build(&marker, 1, &hashes)?;
        assert_eq!(disjoint.failed_sources_verified, Some(2));
        assert_eq!(disjoint.source_retained, Some(2));
        assert_eq!(disjoint.failures_outside_retry_inventory, Some(1));
        marker.src_jpeg_count += 1;
        assert!(build(&marker, 1, &hashes)?.source_retained.is_none());
        Ok(())
    }

    #[test]
    fn failed_source_retention_is_not_inferred_from_retained_successes() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let marker = partial_marker(root.path(), 1);
        std::fs::create_dir_all(&marker.src_dir)?;
        std::fs::write(marker.src_dir.join("0.jpg"), b"successful source")?;
        let report = Report::from_marker(&marker, true, &[], 0, None)?;
        assert_eq!(report.source_retained, Some(1));
        assert_eq!(report.failed_sources_retained, Some(0));

        std::fs::write(marker.src_dir.join("invalid.jpg"), b"failed source")?;
        let report = Report::from_marker(&marker, true, &[], 0, None)?;
        assert_eq!(report.source_retained, Some(2));
        assert_eq!(report.failed_sources_retained, Some(1));
        assert_eq!(report.failed, 1);
        assert_eq!(serde_json::to_value(&report)?["failed_sources_retained"], 1);
        Ok(())
    }

    #[test]
    fn only_typed_target_deferral_after_verification_is_reported_as_deferred() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 1);
        marker.stage = FastImgStageName::Importing;
        let mut report = Report::from_marker(&marker, true, &[], 0, None)?;
        let deferred = Err(anyhow::Error::new(PhotosDeliveryDeferred {
            detail: "target unavailable".into(),
        }));
        report.record_execution(&deferred, true);
        assert!(report.photos_delivery_deferred);
        assert!(report.primary_verification_complete);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.unprocessed, 1);
        assert!(report.finalization_error.is_some());
        assert_eq!(report.output_directory, marker.working_copy);
        report.record_execution(&Err(anyhow::anyhow!("Photos delivery deferred")), true);
        assert!(!report.photos_delivery_deferred);
        report.record_execution(&deferred, false);
        assert!(!report.photos_delivery_deferred);
        marker.stage = FastImgStageName::OutputPrepared;
        let mut report = Report::from_marker(&marker, true, &[], 0, None)?;
        report.record_execution(&deferred, true);
        assert!(!report.photos_delivery_deferred);
        Ok(())
    }

    #[test]
    fn failed_source_verification_requires_the_recorded_regular_file() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 1);
        std::fs::create_dir_all(&marker.src_dir)?;
        let source = marker.src_dir.join("invalid.jpg");
        std::fs::write(&source, b"original failed source")?;
        marker
            .failed_sources
            .get_mut("invalid.jpg")
            .context("failed source")?
            .src = calculate_blake3_hash(&source)?;
        let report = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!(report.failed_sources_verified, Some(1));
        assert_eq!(report.failed_source_verification_error, None);
        assert_eq!(serde_json::to_value(&report)?["failed_sources_verified"], 1);
        let proof = Report::from_execution(
            &marker,
            None,
            false,
            &[],
            &ProbeFailures::default(),
            None,
            Some("worker-1"),
        )?
        .failed_source_proof
        .context("matching source should have file-scoped proof")?;
        assert_eq!(proof.invocation_id, "worker-1");
        assert_eq!(proof.verified_relative_paths, ["invalid.jpg"]);

        std::fs::write(&source, b"changed failed source")?;
        let report = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!(report.failed_sources_retained, Some(1));
        assert_eq!(report.failed_sources_verified, None);
        assert!(report.failed_source_verification_error.is_some());
        assert!(
            Report::from_execution(
                &marker,
                None,
                false,
                &[],
                &ProbeFailures::default(),
                None,
                Some("worker-1")
            )?
            .failed_source_proof
            .is_none()
        );

        std::fs::remove_file(&source)?;
        std::fs::create_dir(&source)?;
        let report = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!(report.failed_sources_retained, None);
        assert!(report.retention_unknown_reason.is_some());
        assert_eq!(report.failed_sources_verified, None);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn failed_source_verification_rejects_symlinks_even_when_content_matches() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 0);
        std::fs::create_dir_all(&marker.src_dir)?;
        let original = root.path().join("original.jpg");
        std::fs::write(&original, b"original failed source")?;
        marker
            .failed_sources
            .get_mut("invalid.jpg")
            .context("failed source")?
            .src = calculate_blake3_hash(&original)?;
        std::os::unix::fs::symlink(&original, marker.src_dir.join("invalid.jpg"))?;
        let report = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!(report.failed_sources_retained, None);
        assert!(report.retention_unknown_reason.is_some());
        assert_eq!(report.failed_sources_verified, None);

        std::fs::remove_file(marker.src_dir.join("invalid.jpg"))?;
        let entry = marker
            .failed_sources
            .remove("invalid.jpg")
            .context("failed source")?;
        marker
            .failed_sources
            .insert("linked/original.jpg".into(), entry);
        std::os::unix::fs::symlink(root.path(), marker.src_dir.join("linked"))?;
        let report = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!(report.failed_sources_retained, None);
        assert!(report.retention_unknown_reason.is_some());
        assert_eq!(report.failed_sources_verified, None);
        Ok(())
    }

    #[test]
    fn only_verified_completed_file_failures_are_not_finalization_errors() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 1);
        marker.stage = FastImgStageName::CleanupComplete;
        std::fs::create_dir_all(&marker.src_dir)?;
        let source = marker.src_dir.join("invalid.jpg");
        std::fs::write(&source, b"original failed source")?;
        marker
            .failed_sources
            .get_mut("invalid.jpg")
            .context("failed source")?
            .src = calculate_blake3_hash(&source)?;
        let mut report = Report::from_marker(&marker, false, &[], 0, None)?;
        let failure = Err(FileFailures::primary(1).into());
        report.record_execution(&failure, false);
        assert_eq!(report.finalization_error, None);
        assert!(
            finish(
                &marker,
                None,
                false,
                &[],
                &ProbeFailures::default(),
                0,
                failure
            )
            .err()
            .context("file failure exit")?
            .is::<FileFailures>()
        );

        report.record_execution(&Err(anyhow::anyhow!("backend unavailable")), false);
        assert!(
            report
                .finalization_error
                .as_deref()
                .is_some_and(|error| error.contains("backend unavailable"))
        );
        report.record_execution(&Err(FileFailures::primary(2).into()), false);
        assert!(report.finalization_error.is_some());
        report.record_execution(
            &Err(anyhow::Error::new(FileFailures::primary(1)).context("final marker write failed")),
            false,
        );
        assert!(report.finalization_error.is_some());
        report.record_execution(&Err(FileFailures::primary(1).into()), true);
        assert!(report.finalization_error.is_some());
        report.unprocessed = 1;
        report.record_execution(&Err(FileFailures::primary(1).into()), false);
        assert!(report.finalization_error.is_some());
        report.unprocessed = 0;
        report.failed_sources_verified = None;
        report.record_execution(&Err(FileFailures::primary(1).into()), false);
        assert!(report.finalization_error.is_some());
        report.failed_sources_verified = Some(1);
        report.primary_cleanup_complete = false;
        report.record_execution(&Err(FileFailures::primary(1).into()), false);
        assert!(report.finalization_error.is_some());
        Ok(())
    }

    #[test]
    fn final_quarantine_requires_every_other_gate_check_to_pass() {
        use foundation::pipeline::verification::{CheckDetail, CheckFailureScope};

        let mut gate = GateResult {
            passed: false,
            checks: [
                "count",
                "blake3_sample",
                "no_error",
                "count_x3",
                "sync",
                "quarantine",
                "chain",
            ]
            .into_iter()
            .map(|name| CheckDetail {
                name,
                passed: name != "quarantine",
                expected: String::new(),
                actual: String::new(),
                affected_files: if name == "quarantine" {
                    vec!["output.jxl".into()]
                } else {
                    vec![]
                },
                file_failure_reasons: Default::default(),
                failure_scope: CheckFailureScope::Shared,
            })
            .collect(),
        };
        assert_eq!(final_quarantine_count(&gate), Some(1));
        gate.checks[6].passed = false;
        assert_eq!(final_quarantine_count(&gate), None);
        gate.checks.pop();
        assert_eq!(final_quarantine_count(&gate), None);
    }

    #[test]
    fn final_quarantine_receipt_never_reports_delivery_or_cleanup() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 0);
        marker.failed_sources.clear();
        marker.src_jpeg_count = 0;
        marker.stage = FastImgStageName::Gate3Failed;
        let mut report = Report::from_marker(&marker, true, &[], 0, None)?;
        report.encoded = 4;
        report.photos_verified = 4;
        report.unprocessed = 4;
        report.skipped = 4;
        report.source_retained = Some(8);
        let locations = PhotosQuarantineLocations {
            working_copies: 0,
            photos_originals: 4,
            affected: 4,
        };
        let blocked = || PhotosQuarantineBlocked {
            locations,
            detail: "synthetic quarantine".into(),
        };
        report.record_execution(&Err(blocked().into()), true);
        assert_eq!(report.photos_quarantine_blocked, Some(4));
        assert_eq!(report.photos_quarantine_locations, Some(locations));
        assert!(report.primary_verification_complete);
        assert!(!report.primary_cleanup_complete);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.unprocessed, 4);
        assert!(report.finalization_error.is_some());
        report.record_execution(
            &Err(anyhow::Error::new(blocked()).context("checkpoint write failed")),
            true,
        );
        assert_eq!(report.photos_quarantine_blocked, None);
        assert_eq!(report.photos_quarantine_locations, None);
        Ok(())
    }

    #[test]
    fn retained_only_completion_requires_fresh_hashes_for_failures_and_skips() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 0);
        marker.stage = FastImgStageName::TranscodeComplete;
        marker.src_jpeg_count += 1;
        std::fs::create_dir(&marker.src_dir)?;
        let failed = marker.src_dir.join("invalid.jpg");
        let skipped = marker.src_dir.join("existing.jxl");
        std::fs::write(&failed, b"synthetic failed source")?;
        std::fs::write(&skipped, b"synthetic retained source")?;
        marker
            .failed_sources
            .get_mut("invalid.jpg")
            .context("failed entry")?
            .src = calculate_blake3_hash(&failed)?;
        marker.skipped_sources.insert(
            "existing.jxl".into(),
            SkippedSourceEntry {
                src: calculate_blake3_hash(&skipped)?,
                reason: "existing JXL verified".into(),
            },
        );
        let build_report = |execution: anyhow::Result<()>| -> anyhow::Result<Report> {
            let mut report = Report::from_marker(
                &marker,
                true,
                &[],
                0,
                execution
                    .as_ref()
                    .err()
                    .and_then(|error| error.downcast_ref::<FileFailures>()),
            )?;
            report.record_execution(&execution, true);
            Ok(report)
        };
        let report = build_report(Err(FileFailures::primary(1).into()))?;
        assert!(report.retained_only_complete);
        assert!(!report.primary_cleanup_complete);
        assert!(!report.primary_verification_complete);
        assert_eq!(report.source_retained, Some(2));
        assert_eq!(report.finalization_error, None);
        let wrapped = anyhow::Error::new(FileFailures::primary(1)).context("marker write failed");
        assert!(!build_report(Err(wrapped))?.retained_only_complete);
        std::fs::write(&skipped, b"modified retained source")?;
        let modified = build_report(Err(FileFailures::primary(1).into()))?;
        assert!(!modified.retained_only_complete);
        assert_eq!(modified.source_retained, None);
        assert!(modified.retention_unknown_reason.is_some());
        assert!(modified.finalization_error.is_some());
        Ok(())
    }

    #[test]
    fn unbound_failures_have_no_failed_source_retention_proof() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let marker = partial_marker(root.path(), 0);
        std::fs::create_dir_all(&marker.src_dir)?;
        std::fs::write(marker.src_dir.join("invalid.jpg"), b"failed source")?;
        let probe_failure = Report::from_marker(&marker, true, &[], 1, None)?;
        assert_eq!(probe_failure.failed, 2);
        assert_eq!(probe_failure.failed_sources_retained, None);
        assert_eq!(probe_failure.failed_sources_verified, None);
        let originals = [ModernLossyStaticCandidate {
            path: marker.src_dir.join("original.jxl"),
            rel_path: "original.jxl".into(),
            format: foundation::image::format_detect::FormatKind::Jxl,
            blake3: "original-hash".into(),
        }];
        let original_failure = Report::from_marker(
            &marker,
            true,
            &originals,
            0,
            Some(&FileFailures::originals(1)),
        )?;
        assert_eq!(original_failure.failed, 2);
        assert_eq!(original_failure.failed_sources_retained, None);
        assert_eq!(original_failure.failed_sources_verified, None);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn failed_source_inspection_error_is_unknown_not_missing_or_retained() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let marker = partial_marker(root.path(), 0);
        std::fs::create_dir_all(&marker.src_dir)?;
        std::os::unix::fs::symlink("invalid.jpg", marker.src_dir.join("invalid.jpg"))?;
        let report = Report::from_marker(&marker, true, &[], 0, None)?;
        assert_eq!(report.source_retained, None);
        assert_eq!(report.failed_sources_retained, None);
        assert!(report.retention_unknown_reason.is_some());
        Ok(())
    }

    #[test]
    fn encoded_but_unverified_is_not_reported_as_delivered() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 3);
        marker.stage = FastImgStageName::Gate1Passed;
        let report = Report::from_marker(&marker, true, &[], 0, None)?;
        assert_eq!(
            (
                report.succeeded,
                report.encoded,
                report.failed,
                report.unprocessed
            ),
            (0, 3, 1, 3)
        );
        let local = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!((local.succeeded, local.unprocessed), (3, 0));
        marker.skipped_sources = marker.failed_sources.clone();
        assert!(Report::from_marker(&marker, true, &[], 0, None).is_err());
        Ok(())
    }

    #[test]
    fn originals_with_photos_receipts_stay_pending_until_source_cleanup() -> anyhow::Result<()> {
        use foundation::pipeline::verification::LibraryAssetRecord;

        let root = tempfile::tempdir()?;
        let mut marker =
            WorkingCopyMarker::new(root.path().join("source"), root.path().join("output"), 0);
        std::fs::create_dir(&marker.src_dir)?;
        let source = marker.src_dir.join("original.webp");
        std::fs::write(&source, b"synthetic original")?;
        marker.stage = FastImgStageName::CleanupComplete;
        marker.tier2_in_progress = true;
        marker.tier2_imported_assets.push(LibraryAssetRecord {
            rel_path: "original.webp".into(),
            blake3: "a".repeat(64),
            photos_uuid: Some("synthetic-asset".into()),
            ..LibraryAssetRecord::default()
        });

        let pending = Report::from_marker(&marker, true, &[], 0, None)?;
        assert_eq!(
            (
                pending.succeeded,
                pending.unprocessed,
                pending.photos_verified
            ),
            (0, 1, 1)
        );
        assert_eq!(pending.source_retained, Some(1));
        assert_eq!(
            pending
                .original_delivery
                .context("missing custody counts")?
                .source_cleanup_confirmed,
            0
        );

        marker.tier2_in_progress = false;
        let conflicting = Report::from_marker(&marker, true, &[], 0, None)?;
        assert_eq!((conflicting.succeeded, conflicting.unprocessed), (0, 1));

        std::fs::remove_file(source)?;
        let complete = Report::from_marker(&marker, true, &[], 0, None)?;
        assert_eq!(
            (
                complete.succeeded,
                complete.unprocessed,
                complete.photos_verified
            ),
            (1, 0, 1)
        );
        assert_eq!(complete.source_retained, Some(0));
        assert_eq!(
            complete
                .original_delivery
                .context("missing cleanup counts")?
                .source_cleanup_confirmed,
            1
        );
        Ok(())
    }

    #[test]
    fn unclassified_originals_are_not_forged_into_file_failures() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let marker = partial_marker(root.path(), 3);
        let originals = [ModernLossyStaticCandidate {
            path: marker.src_dir.join("original.jxl"),
            rel_path: "original.jxl".into(),
            format: foundation::image::format_detect::FormatKind::Jxl,
            blake3: "original-hash".into(),
        }];
        let pending = Report::from_marker(&marker, true, &originals, 0, None)?;
        assert_eq!(
            (pending.succeeded, pending.failed, pending.unprocessed),
            (3, 1, 1)
        );
        let failed = Report::from_marker(
            &marker,
            true,
            &originals,
            0,
            Some(&FileFailures::originals(1)),
        )?;
        assert_eq!(
            (failed.succeeded, failed.failed, failed.unprocessed),
            (3, 2, 0)
        );
        assert!(
            FileFailures::originals(1)
                .to_string()
                .contains("original-media delivery recorded 1")
        );
        assert!(
            FileFailures::primary(2)
                .to_string()
                .contains("batch recorded 2")
        );
        let originals = (0..19)
            .map(|index| ModernLossyStaticCandidate {
                path: marker.src_dir.join(format!("original-{index}.jxl")),
                rel_path: format!("original-{index}.jxl"),
                format: foundation::image::format_detect::FormatKind::Jxl,
                blake3: format!("original-hash-{index}"),
            })
            .collect::<Vec<_>>();
        let failed = Report::from_marker(
            &marker,
            true,
            &originals,
            0,
            Some(&FileFailures::originals(19)),
        )?;
        assert_eq!(
            (failed.succeeded, failed.failed, failed.unprocessed),
            (3, 20, 0)
        );
        Ok(())
    }

    #[test]
    fn routed_retry_counts_exact_original_failures_and_pending_once() -> anyhow::Result<()> {
        use foundation::batch::{DiscoveredSource, planned_source::PlannedSource};
        use foundation::fast_img_store::FailedRetrySidecar;
        use foundation::pipeline::verification::SkippedSourceEntry;

        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        let src = root.join("source");
        std::fs::create_dir(&src)?;
        let mut admissions = BTreeMap::new();
        for (relative, route) in [
            ("primary.jpg", FailedRetrySourceRoute::Primary),
            ("retained.heic", FailedRetrySourceRoute::Original),
            ("pending.heic", FailedRetrySourceRoute::Original),
        ] {
            let path = src.join(relative);
            std::fs::write(&path, relative.as_bytes())?;
            let observed = DiscoveredSource::from_metadata(&path, &std::fs::metadata(&path)?)?;
            let hash = observed.content_blake3();
            admissions.insert(
                relative.to_owned(),
                FailedRetryAdmission {
                    route: Some(route),
                    source_rel: relative.into(),
                    parent_source_blake3: hash.clone(),
                    admitted_source_blake3: hash,
                    claim: PlannedSource::encode(&observed)?,
                    xmp: FailedRetrySidecar::Absent,
                    aae: FailedRetrySidecar::Absent,
                },
            );
        }
        let mut marker = WorkingCopyMarker::new(src.clone(), root.join("output"), 3);
        marker.failed_retry_primary_source_count = Some(1);
        for (relative, failures) in [
            ("primary.jpg", &mut marker.failed_sources),
            ("retained.heic", &mut marker.tier2_failed_sources),
        ] {
            failures.insert(
                relative.into(),
                SkippedSourceEntry {
                    src: admissions[relative].admitted_source_blake3.clone(),
                    reason: "synthetic failure".into(),
                },
            );
        }
        marker.stage = FastImgStageName::CleanupComplete;
        marker.tier2_in_progress = true;
        let build = |marker: &WorkingCopyMarker| {
            Report::from_execution(
                marker,
                Some(&admissions),
                true,
                &[],
                &ProbeFailures::default(),
                Some(&FileFailures::originals(1)),
                None,
            )
        };
        let report = build(&marker)?;
        assert_eq!(
            (report.succeeded, report.failed, report.unprocessed),
            (0, 2, 1)
        );
        assert_eq!(report.source_retained, Some(3));
        assert_eq!(report.failed_sources_verified, Some(2));
        std::fs::write(src.join("retained.xmp"), b"unexpected companion")?;
        assert_eq!(build(&marker)?.failed_sources_verified, None);
        assert!(
            Report::from_execution(
                &marker,
                None,
                true,
                &[],
                &ProbeFailures::default(),
                None,
                None
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn forged_inventory_overflow_fails_report_construction() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut marker = partial_marker(root.path(), 0);
        marker.src_jpeg_count = usize::MAX;
        let originals = [ModernLossyStaticCandidate {
            path: marker.src_dir.join("original.jxl"),
            rel_path: "original.jxl".into(),
            format: foundation::image::format_detect::FormatKind::Jxl,
            blake3: "original-hash".into(),
        }];

        let error = Report::from_marker(&marker, true, &originals, 0, None)
            .err()
            .ok_or_else(|| anyhow::anyhow!("inventory addition must not wrap"))?;
        assert!(error.to_string().contains("inventory count overflow"));
        Ok(())
    }
}
