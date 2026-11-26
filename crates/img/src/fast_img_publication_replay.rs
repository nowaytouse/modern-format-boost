//! Resume interrupted publication before allocating collision-safe names for new work.

use anyhow::{Context, Result, ensure};
use foundation::batch::AdmittedImageFile;
use foundation::fast_img_store::{
    FailedRetryContext, FailedRetrySourceRoute, PendingPublication, failed_retry_context,
    pending_publications, verify_failed_retry_admission,
};
use foundation::pipeline::verification::{
    validate_no_symlink_components, verify_regular_file_blake3,
};
use std::path::Path;

use super::{
    FastImgEncodeContext, FastImgEncodeResultSummary, FastImgTranscodeOutcome, FileFailurePhase,
    fast_img_apply_encode_results_at_attempt, fast_img_avif_metadata_policy,
    fast_img_may_have_primary_photos_submission, fast_img_publication, fast_img_verify_jxl_output,
    verify_final_avif_delivery_integrity,
};

#[expect(
    clippy::redundant_pub_crate,
    reason = "This CLI entry point requires restricted visibility under unreachable_pub"
)]
pub(super) fn recover(context: &mut FastImgEncodeContext<'_>) -> Result<()> {
    let pending = pending_publications(context.marker)?;
    if pending.is_empty() {
        return Ok(());
    }
    let _uncached = foundation::common_utils::UncachedVerificationScope::new()?;
    let failed_retry = failed_retry_context(context.working_copy)?;
    let mut summary = FastImgEncodeResultSummary {
        encoded: context
            .marker
            .blake3_log
            .values()
            .filter(|entry| !entry.out.is_empty())
            .count(),
        ..Default::default()
    };
    for record in &pending {
        foundation::batch_control::checkpoint()?;
        recover_one(context, record, failed_retry.as_ref(), &mut summary).with_context(|| {
            format!(
                "recover interrupted Fast IMG publication for {}",
                record.source_rel
            )
        })?;
    }
    Ok(())
}

fn recover_one(
    context: &mut FastImgEncodeContext<'_>,
    record: &PendingPublication,
    failed_retry: Option<&FailedRetryContext>,
    summary: &mut FastImgEncodeResultSummary,
) -> Result<()> {
    let rel = &record.source_rel;
    ensure!(
        context.current_source_hashes.get(rel) == Some(&record.source_blake3)
            && !context.scan_failures.contains_key(rel),
        "pending publication source is outside the current primary inventory or has changed"
    );
    validate_no_symlink_components(context.src_dir, Path::new(rel)).map_err(anyhow::Error::msg)?;
    let source = context.src_dir.join(rel);
    let source_identity =
        AdmittedImageFile::from_metadata(&source, &std::fs::symlink_metadata(&source)?)?;
    if let Some(retry) = failed_retry {
        let admission = retry
            .admissions()
            .get(rel)
            .context("pending source outside failed-only retry")?;
        ensure!(
            admission.route != Some(FailedRetrySourceRoute::Original)
                && admission.admitted_source_blake3 == record.source_blake3,
            "failed-only admission differs from pending publication"
        );
        verify_failed_retry_admission(context.src_dir, admission)?;
    } else {
        verify_regular_file_blake3(&source, &record.source_blake3)?;
    }
    let marker = &mut *context.marker;
    ensure!(
        !marker.skipped_sources.contains_key(rel)
            && (!marker.failed_sources.contains_key(rel)
                || context.retry_failed_sources_from_cleanup.0),
        "pending publication conflicts with a retained source disposition"
    );
    let settled = match marker.blake3_log.get(rel) {
        Some(entry) if !entry.out.is_empty() => {
            ensure!(
                entry.src == record.source_blake3
                    && entry.out == record.output_blake3
                    && entry.out_rel.as_deref() == Some(record.output_rel.as_str()),
                "pending publication conflicts with the saved output receipt"
            );
            true
        }
        _ => false,
    };
    let (proof, attempt) = match fast_img_publication::publish(marker, record) {
        Ok(result) => result,
        Err(error)
            if !settled
                && !fast_img_may_have_primary_photos_submission(marker)
                && (error.is::<fast_img_publication::PublicationMissing>()
                    || error.is::<fast_img_publication::PublicationConflict>()) =>
        {
            fast_img_publication::finish(marker, record)?;
            println!(
                "[RESUME  ] returned {} to encoding after its pending publication became unavailable: {error}",
                record.source_rel
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let output = context.working_copy.join(&record.output_rel);
    let output_identity =
        AdmittedImageFile::from_metadata(&output, &std::fs::symlink_metadata(&output)?)?;
    if context.strategy == "avif" {
        verify_final_avif_delivery_integrity(&source, &output, fast_img_avif_metadata_policy())?;
    } else {
        fast_img_verify_jxl_output(&source, &output)?;
    }
    verify_regular_file_blake3(&source, &record.source_blake3)?;
    verify_regular_file_blake3(&output, &record.output_blake3)?;
    source_identity.verify_regular_file_unchanged()?;
    output_identity.verify_regular_file_unchanged()?;
    fast_img_publication::verify_published_identity(marker, record)?;
    if !settled {
        fast_img_apply_encode_results_at_attempt(
            marker,
            [Ok(FastImgTranscodeOutcome::Converted(Box::new(proof)))],
            summary,
            context.src_dir,
            context.working_copy,
            FileFailurePhase::EncodeOrPublish,
            attempt,
        )?;
    }
    fast_img_publication::finish(marker, record)?;
    println!(
        "[RESUME  ] recovered verified output {} from its saved publication",
        record.output_rel
    );
    Ok(())
}
