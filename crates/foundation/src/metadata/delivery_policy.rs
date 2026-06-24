//! Delivery metadata policy (M23 CONTRACT).
//!
//! Conversion **must not** fail solely because the source lacks EXIF, xattrs,
//! or sidecars. Layers audit and continue; [`MetadataDeliveryReport`] makes the
//! outcome explicit.

use std::io;
use std::path::Path;
use std::process::Output;

/// Per-layer outcome for a single `preserve_for_delivery` run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataLayerOutcome {
    /// Layer ran and completed without a recorded soft failure.
    #[default]
    Applied,
    /// Tool missing (e.g. `exiftool` not on `PATH`) — non-blocking for
    /// delivery.
    SkippedNoTool,
    /// Source had no corresponding metadata (empty tags, no xattrs, unsupported
    /// xattr API).
    SkippedNoSourceMetadata,
    /// Partial failure audited; delivery continues.
    PartialAudit,
}

/// Explicit delivery metadata preservation result.
#[derive(Debug, Clone, Default)]
pub struct MetadataDeliveryReport {
    pub exif: MetadataLayerOutcome,
    pub xattr: MetadataLayerOutcome,
    pub timestamps: MetadataLayerOutcome,
}

impl MetadataDeliveryReport {
    #[must_use]
    pub const fn any_partial_or_skipped(&self) -> bool {
        !matches!(self.exif, MetadataLayerOutcome::Applied)
            || !matches!(self.xattr, MetadataLayerOutcome::Applied)
            || !matches!(self.timestamps, MetadataLayerOutcome::Applied)
    }
}

/// CONTRACT: xattr list/get API unavailable — not a delivery failure.
#[must_use]
pub(in crate::metadata) fn is_xattr_api_absence(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::NotFound
    ) || matches!(err.raw_os_error(), Some(93 | 95 | 524))
        || err
            .to_string()
            .to_ascii_lowercase()
            .contains("not supported")
        || err
            .to_string()
            .to_ascii_lowercase()
            .contains("operation not supported")
        || err
            .to_string()
            .to_ascii_lowercase()
            .contains("attribute not found")
}

/// CONTRACT: `ExifTool` reported nothing writable on the source — non-blocking.
#[must_use]
pub(in crate::metadata) fn exiftool_combined_output_indicates_no_source_tags(
    combined: &str,
) -> bool {
    let s = combined.to_ascii_lowercase();
    s.contains("0 image files updated")
        || s.contains("nothing to do")
        || s.contains("no writable tags")
        || s.contains("no exif data")
        || (s.contains("doesn't exist") && s.contains("tag"))
        || s.contains("file not contain metadata")
}

/// CONTRACT: `ExifTool` process output indicates absence-only (not structural
/// corruption).
#[must_use]
pub(in crate::metadata) fn exiftool_output_indicates_no_source_tags(output: &Output) -> bool {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    exiftool_combined_output_indicates_no_source_tags(&combined)
}

/// CONTRACT: I/O error text from a metadata layer is soft for delivery (audit +
/// continue).
#[must_use]
pub(in crate::metadata) fn is_metadata_delivery_soft_error(err: &io::Error) -> bool {
    if is_xattr_api_absence(err) {
        return true;
    }
    exiftool_combined_output_indicates_no_source_tags(&err.to_string())
}

/// Best-effort metadata preservation for conversion delivery (never blocks on
/// empty source tags).
///
/// # Errors
/// Returns an error only when the destination cannot be accessed for
/// preservation (missing output, permission denied on `dst`, etc.). A missing
/// or unreadable `src` yields [`MetadataDeliveryReport`] with skipped layers
/// and `Ok`.
pub fn preserve_for_delivery(src: &Path, dst: &Path) -> io::Result<MetadataDeliveryReport> {
    let mut report = MetadataDeliveryReport::default();

    match std::fs::metadata(src) {
        Ok(_) => {}
        Err(e) => {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_DELIVERY_SKIP_MISSING_SOURCE
                    .replace("{}", &format!("{}: {e}", src.display())),
            );
            report.exif = MetadataLayerOutcome::SkippedNoSourceMetadata;
            report.xattr = MetadataLayerOutcome::SkippedNoSourceMetadata;
            report.timestamps = MetadataLayerOutcome::SkippedNoSourceMetadata;
            return Ok(report);
        }
    }

    std::fs::metadata(dst).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "Failed to read destination metadata for preservation into {}: {e}",
                dst.display()
            ),
        )
    })?;

    super::preserve_pro_for_delivery(src, dst, &mut report)?;
    Ok(report)
}

/// Best-effort timestamp sync after delivery mutations (audit only on partial
/// failure).
///
/// # Errors
/// Does not propagate errors; partial timestamp failures are audited into
/// `report`.
pub fn apply_file_timestamps_for_delivery(
    src: &Path,
    dst: &Path,
    report: &mut MetadataDeliveryReport,
) -> io::Result<()> {
    match super::apply_file_timestamps(src, dst) {
        Ok(()) => {
            report.timestamps = MetadataLayerOutcome::Applied;
            Ok(())
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata_timestamp",
                crate::infra::static_logs::messages::MSG_METADATA_DELIVERY_TIMESTAMP_PARTIAL
                    .replace("{}", &e.to_string()),
            );
            report.timestamps = MetadataLayerOutcome::PartialAudit;
            Ok(())
        }
    }
}
