//! Keep delivery facts separate from finalization errors and the process exit.
#![expect(
    clippy::redundant_pub_crate,
    reason = "CLI result types remain internal; public visibility conflicts with unreachable_pub"
)]
use foundation::ModernLossyStaticCandidate;
use foundation::pipeline::verification::{
    FastImgStageName, WorkingCopyMarker, gate1_complete_or_later, gate3_complete_or_later,
    write_marker_atomic,
};
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
            "fast-img recorded {} failed source(s); failed sources retained",
            self.total
        )
    }
}
impl std::error::Error for FileFailures {}

#[derive(Debug, serde::Serialize)]
struct Report {
    schema_version: u8,
    succeeded: usize,
    skipped: usize,
    ignored: usize,
    failed: usize,
    unprocessed: usize,
    encoded: usize,
    photos_verified: usize,
    source_retained: Option<usize>,
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
        let retention = sources.iter().try_fold(0usize, |count, rel| {
            marker
                .src_dir
                .join(rel)
                .try_exists()
                .map(|exists| count + usize::from(exists))
        });
        let (source_retained, retention_unknown_reason) = match retention {
            Err(error) => (
                None,
                Some(format!("source existence inspection failed: {error}")),
            ),
            Ok(_) if probe_failures > 0 || !marker.source_disposition_is_complete() => (
                None,
                Some("source inventory has unclassified or unaccounted entries".into()),
            ),
            Ok(count) => (Some(count), None),
        };
        Ok(Self {
            schema_version: 1,
            succeeded,
            skipped,
            ignored: 0,
            failed,
            unprocessed,
            encoded: marker.blake3_log.len(),
            photos_verified,
            source_retained,
            retention_unknown_reason,
            primary_cleanup_complete: marker.stage == FastImgStageName::CleanupComplete,
            finalization_error: None,
        })
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
    report.finalization_error = execution.as_ref().err().map(|error| format!("{error:#}"));
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
