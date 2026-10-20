//! Keep delivery facts separate from finalization errors and the process exit.
#![expect(
    clippy::redundant_pub_crate,
    reason = "CLI result types remain internal; public visibility conflicts with unreachable_pub"
)]
use foundation::ModernLossyStaticCandidate;
use foundation::common_utils::calculate_blake3_hash;
use foundation::pipeline::verification::{
    FastImgStageName, WorkingCopyMarker, gate1_complete_or_later, gate3_complete_or_later,
    write_marker_atomic,
};
use foundation::unified_error::PhotosDeliveryDeferred;
use std::collections::BTreeSet;

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

#[derive(Debug, serde::Serialize)]
struct Report {
    schema_version: u8,
    source_directory: std::path::PathBuf,
    output_directory: std::path::PathBuf,
    photos_delivery_deferred: bool,
    primary_verification_complete: bool,
    succeeded: usize,
    skipped: usize,
    ignored: usize,
    failed: usize,
    unprocessed: usize,
    encoded: usize,
    photos_verified: usize,
    source_retained: Option<usize>,
    failed_sources_retained: Option<usize>,
    failed_sources_verified: Option<usize>,
    failed_source_verification_error: Option<String>,
    retention_unknown_reason: Option<String>,
    primary_cleanup_complete: bool,
    finalization_error: Option<String>,
}

impl Report {
    fn from_marker(
        marker: &WorkingCopyMarker,
        photos: bool,
        originals: &[ModernLossyStaticCandidate],
        probe_failures: usize,
        recorded_failures: Option<&FileFailures>,
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
        let tier2 = originals
            .iter()
            .map(|c| c.rel_path.as_str())
            .chain(
                marker
                    .tier2_imported_assets
                    .iter()
                    .map(|a| a.rel_path.as_str()),
            )
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
        let succeeded = checked_add(
            primary_succeeded,
            marker.tier2_imported_assets.len(),
            "succeeded",
        )?;
        // Count only explicit file failures; unresolved originals stay unprocessed.
        let failed = recorded_failures.into_iter().try_fold(
            checked_add(marker.failed_sources.len(), probe_failures, "failed")?,
            |total, failures| checked_add(total, failures.originals, "failed"),
        )?;
        let skipped = marker.skipped_sources.len();
        let total = checked_add(
            checked_add(marker.src_jpeg_count, tier2.len(), "inventory")?,
            probe_failures,
            "inventory",
        )?;
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
            marker.tier2_imported_assets.len(),
            "Photos-verified",
        )?;
        let sources = marker
            .blake3_log
            .keys()
            .map(String::as_str)
            .chain(marker.failed_sources.keys().map(String::as_str))
            .chain(marker.skipped_sources.keys().map(String::as_str))
            .chain(tier2)
            .collect::<BTreeSet<_>>();
        let retention = sources
            .iter()
            .try_fold((0usize, 0usize), |(count, failures), rel| {
                marker.src_dir.join(rel).try_exists().map(|exists| {
                    (
                        count + usize::from(exists),
                        failures + usize::from(exists && marker.failed_sources.contains_key(*rel)),
                    )
                })
            });
        let failed_sources_retained = match &retention {
            Ok((_, count))
                if probe_failures == 0
                    && recorded_failures.is_none_or(|failures| failures.originals == 0) =>
            {
                Some(*count)
            }
            _ => None,
        };
        let (source_retained, retention_unknown_reason) = match retention {
            Err(error) => (
                None,
                Some(format!("source existence inspection failed: {error}")),
            ),
            Ok(_) if probe_failures > 0 || !marker.source_disposition_is_complete() => (
                None,
                Some("source inventory has unclassified or unaccounted entries".into()),
            ),
            Ok((count, _)) => (Some(count), None),
        };
        let failed_verification = (|| -> anyhow::Result<usize> {
            anyhow::ensure!(
                probe_failures == 0
                    && recorded_failures.is_none_or(|failures| failures.originals == 0),
                "failed sources are not all bound to recorded source hashes"
            );
            for (rel, entry) in &marker.failed_sources {
                let mut path = marker.src_dir.clone();
                let root = std::fs::symlink_metadata(&path)?;
                anyhow::ensure!(root.is_dir(), "failed source root is not a real directory");
                for component in std::path::Path::new(rel).components() {
                    path.push(component);
                    let metadata = std::fs::symlink_metadata(&path)?;
                    anyhow::ensure!(
                        !metadata.file_type().is_symlink(),
                        "failed source path contains a symlink: {rel}"
                    );
                }
                anyhow::ensure!(
                    std::fs::symlink_metadata(&path)?.is_file(),
                    "failed source is not a regular file: {rel}"
                );
                anyhow::ensure!(
                    calculate_blake3_hash(&path)? == entry.src,
                    "failed source no longer matches its recorded content hash: {rel}"
                );
            }
            Ok(marker.failed_sources.len())
        })();
        let (failed_sources_verified, failed_source_verification_error) = match failed_verification
        {
            Ok(count) => (Some(count), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        Ok(Self {
            schema_version: 1,
            source_directory: std::path::absolute(&marker.src_dir)?,
            output_directory: std::path::absolute(&marker.working_copy)?,
            photos_delivery_deferred: false,
            primary_verification_complete: gate1_complete_or_later(&marker.stage),
            succeeded,
            skipped,
            ignored: 0,
            failed,
            unprocessed,
            encoded: marker.blake3_log.len(),
            photos_verified,
            source_retained,
            failed_sources_retained,
            failed_sources_verified,
            failed_source_verification_error,
            retention_unknown_reason,
            primary_cleanup_complete: marker.stage == FastImgStageName::CleanupComplete,
            finalization_error: None,
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
        self.finalization_error = execution.as_ref().err().and_then(|error| {
            let isolated_file_failures =
                error
                    .downcast_ref::<FileFailures>()
                    .is_some_and(|failures| {
                        error.chain().count() == 1
                            && failures.total == self.failed
                            && self.unprocessed == 0
                            && self.primary_cleanup_complete
                            && self.failed_sources_verified == Some(self.failed)
                            && (!photos || self.photos_verified == self.succeeded)
                    });
            (!isolated_file_failures).then(|| format!("{error:#}"))
        });
    }
}

pub(super) fn finish(
    marker: &WorkingCopyMarker,
    photos: bool,
    originals: &[ModernLossyStaticCandidate],
    probe_failures: usize,
    ignored: usize,
    execution: anyhow::Result<()>,
) -> anyhow::Result<()> {
    let recorded_failures = execution
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<FileFailures>());
    let mut report =
        Report::from_marker(marker, photos, originals, probe_failures, recorded_failures).map_err(
            |error| match execution.as_ref() {
                Ok(()) => error,
                Err(execution) => error.context(format!("execution also failed: {execution:#}")),
            },
        )?;
    report.record_execution(&execution, photos);
    report.ignored = ignored;
    println!("MFB_FAST_IMG_RESULT={}", serde_json::to_string(&report)?);
    println!(
        "Succeeded: {}\nSkipped: {}\nFailed: {}\nIgnored: {}\nUnprocessed: {}",
        report.succeeded, report.skipped, report.failed, report.ignored, report.unprocessed
    );
    println!(
        "[RESULT  ] encoded={} Photos-verified={} unprocessed={} primary-cleanup-complete={}",
        report.encoded, report.photos_verified, report.unprocessed, report.primary_cleanup_complete
    );
    execution?;
    if report.unprocessed != 0 {
        anyhow::bail!(
            "fast-img left {} input(s) unprocessed; sources retained",
            report.unprocessed
        );
    }
    if report.failed != 0 {
        return Err(FileFailures::primary(report.failed).into());
    }
    Ok(())
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
            let per_file = finish(&marker, true, &[], 0, 0, Ok(()))
                .err()
                .ok_or_else(|| anyhow::anyhow!("file failure must produce an error"))?;
            assert!(per_file.is::<FileFailures>());
            let infrastructure = finish(
                &marker,
                true,
                &[],
                0,
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

        std::fs::write(&source, b"changed failed source")?;
        let report = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!(report.failed_sources_retained, Some(1));
        assert_eq!(report.failed_sources_verified, None);
        assert!(report.failed_source_verification_error.is_some());

        std::fs::remove_file(&source)?;
        std::fs::create_dir(&source)?;
        let report = Report::from_marker(&marker, false, &[], 0, None)?;
        assert_eq!(report.failed_sources_retained, Some(1));
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
        assert_eq!(report.failed_sources_retained, Some(1));
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
            finish(&marker, false, &[], 0, 0, failure)
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
