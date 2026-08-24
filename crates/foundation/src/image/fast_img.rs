//! Fast JPEG-only image pipeline utilities.
//!
//! Implements §Detection, §Codec, §Integrity, §Orientation, §Import, §Delete
//! Safety from `mfb_fast_img_mode_spec part 2.md`.
//!
//! # Open decisions
//! - \[D2\] chose: abort+rollback — safest; no partial state on destructive
//!   delete
//! - \[D4\] chose: Rust-only — no Python img module
//! - \[D5\] chose: subcommand in main.rs — matches existing Commands enum
//! - \[D6\] chose: verified source delete is mandatory after Gate 1/3 pass
//! - \[D7\] revised by Part 5: shared magic-byte detection is the admission
//!   source of truth
//! - \[D8\] chose: decode-probe-only for AVIF — roundtrip hash valid only for
//!   JXL lossless encode
//! - \[I1\] chose: shortest-path import uses Photos `AppleScript` UUID import
//!   plus osxphotos query verifier and fails closed
//! - \[I2\] chose: default verifier proves Photos local custody; iCloud upload
//!   completion polling is explicit opt-in to avoid pressuring Photos/cloud
//!   daemons

use crate::pipeline::verification::{
    Blake3Entry, LibraryAssetRecord, LibraryHandle, WorkingCopyMarker, write_marker_atomic,
};
use crate::unified_error::{BatchErrorMode, ImgQualityError, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
#[cfg(all(target_os = "macos", not(test)))]
use std::time::Instant;

fn run_fast_img_command_with_timeout(
    command: &mut std::process::Command,
    timeout: Duration,
    context: &str,
) -> std::io::Result<std::process::Output> {
    crate::process_runner::run_command_with_liveness_timeout(command, timeout, timeout, context)
}

/// Returns `true` when `path` has true JPEG magic bytes.
///
/// Content detection is never extension-only (§Detection). Deep forensic tool
/// validation is exposed separately by `format_detect` for audit flows;
/// fast-img admission does not revalidate a file already identified by JPEG
/// magic.
///
/// # Errors
/// Propagates I/O errors from
/// [`crate::image::format_detect::detect_true_format`].
pub fn is_true_jpeg(path: &Path) -> Result<bool> {
    use crate::image::format_detect::{FormatKind, detect_true_format};
    Ok(detect_true_format(path)? == FormatKind::Jpeg)
}

/// Roundtrip BLAKE3 integrity check for a raw JXL lossless encode
/// (§Integrity).
///
/// Decodes `jxl_output` back to a JPEG via `djxl`, then compares
/// `BLAKE3(decoded.jpg) == BLAKE3(source_jpeg)`.
///
/// This is bit-exact proof that either a raw or final JXL container preserves
/// the source JPEG bitstream. Delivery keeps reconstruction-owned metadata
/// frozen and appends any external XMP as an overlay; final fast-img delivery
/// adds [`verify_final_jxl_delivery_integrity`] for the remaining container,
/// orientation, and custody gates.
///
/// Fails closed when `djxl` is unavailable; a JXL integrity proof requires
/// a decoded roundtrip hash, not a non-empty output file.
///
/// \[D8\] AVIF: roundtrip hash not valid (encoder may silently degrade to
/// lossy); for AVIF outputs call `verify_decode_probe` instead.
///
/// # Errors
/// Returns an error if the roundtrip hash mismatches or decoding fails.
pub fn verify_jxl_roundtrip_integrity(
    source_jpeg: &Path,
    jxl_output: &Path,
) -> Result<IntegrityResult> {
    use crate::DjxlBuilder;
    use crate::ToolBuilder;
    use crate::common_utils::calculate_blake3_hash;

    // Decode probe: output must exist and be non-empty regardless
    let out_meta = std::fs::metadata(jxl_output).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "integrity: cannot stat output {}: {e}",
            jxl_output.display()
        ))
    })?;
    if out_meta.len() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "integrity: output is empty: {}",
            jxl_output.display()
        )));
    }

    let output_hash = calculate_blake3_hash(jxl_output).map_err(|e| {
        ImgQualityError::AnalysisError(format!("integrity: BLAKE3(output) failed: {e}"))
    })?;

    if !DjxlBuilder::check_available() {
        tracing::error!(
            target: "fast_img_integrity",
            output = %jxl_output.display(),
            output_blake3 = %output_hash,
            "djxl unavailable; refusing decode-probe-only JXL integrity"
        );
        return Err(ImgQualityError::AnalysisError(format!(
            "integrity: djxl unavailable; refusing decode-probe-only JXL proof for {}",
            jxl_output.display()
        )));
    }

    // Decode JXL → JPEG in a temp file
    let temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "fast_img_roundtrip",
        None,
        Some(".jpg"),
    )
    .map_err(|e| ImgQualityError::AnalysisError(format!("integrity: temp alloc failed: {e}")))?;
    let temp_path = temp.path();

    let mut decode_command = DjxlBuilder::new()
        .input(jxl_output)
        .output(temp_path)
        .build();
    decode_command.arg("--reconstruct_jpeg");
    let decode_output = run_fast_img_command_with_timeout(
        &mut decode_command,
        FAST_IMG_MEDIA_PROBE_TIMEOUT,
        "fast-img JXL roundtrip decode",
    )
    .map_err(|e| ImgQualityError::AnalysisError(format!("integrity: djxl decode failed: {e}")))?;

    if !decode_output.status.success() {
        let stderr = first_nonempty_tool_line(&decode_output.stderr).unwrap_or("<empty stderr>");
        return Err(ImgQualityError::AnalysisError(format!(
            "integrity: djxl exited non-zero for {}: {stderr}",
            jxl_output.display()
        )));
    }
    log_suppressed_tool_output(
        "djxl roundtrip decode output captured",
        &decode_output.stdout,
        &decode_output.stderr,
    );

    let decoded_hash = calculate_blake3_hash(temp_path).map_err(|e| {
        ImgQualityError::AnalysisError(format!("integrity: BLAKE3(decoded) failed: {e}"))
    })?;
    let source_hash = calculate_blake3_hash(source_jpeg).map_err(|e| {
        ImgQualityError::AnalysisError(format!("integrity: BLAKE3(source) failed: {e}"))
    })?;

    tracing::info!(
        target: "fast_img_integrity",
        source = %source_jpeg.display(),
        source_blake3 = %source_hash,
        output = %jxl_output.display(),
        output_blake3 = %output_hash,
        decoded_blake3 = %decoded_hash,
        "roundtrip integrity check"
    );

    if decoded_hash != source_hash {
        return Err(ImgQualityError::AnalysisError(format!(
            "integrity FAIL: roundtrip hash mismatch for {} (src={source_hash}, \
             decoded={decoded_hash})",
            jxl_output.display()
        )));
    }

    Ok(IntegrityResult::RoundtripMatch {
        source_hash,
        output_hash,
    })
}

/// Pixel-equivalence integrity proof for JPEG→JXL encodes without JBRD.
///
/// This is used for `cjxl --lossless_jpeg=1 --allow_jpeg_reconstruction=0`
/// and sanitized-tail retries. Those outputs preserve decoded
/// pixels/DCT-derived image content but cannot reconstruct the original JPEG
/// byte stream, so the BLAKE3 roundtrip proof is intentionally not applicable.
///
/// # Errors
/// Returns an error if `djxl`/pixel proof is unavailable or mismatches.
pub fn verify_jxl_pixel_equivalence_integrity(
    source_jpeg: &Path,
    jxl_output: &Path,
) -> Result<IntegrityResult> {
    use crate::image::format_detect::FormatKind;
    verify_pixel_equivalence_integrity(source_jpeg, jxl_output, FormatKind::Jxl)
}

/// Pixel-equivalence integrity proof for modern format encodes.
///
/// Supports JXL (without JBRD), AVIF, and other lossy modern formats.
/// Uses format-specific pixel diff tolerance from orientation policy.
///
/// # Errors
/// Returns an error if decoder/pixel proof is unavailable or mismatches.
pub fn verify_pixel_equivalence_integrity(
    source_jpeg: &Path,
    output: &Path,
    format: crate::image::format_detect::FormatKind,
) -> Result<IntegrityResult> {
    use crate::common_utils::calculate_blake3_hash;
    use crate::image::orientation::{
        PixelDiffResult, pixel_equivalence_diff_tolerance_for_format, verify_orientation_pixel_diff,
    };

    let out_meta = std::fs::metadata(output).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "pixel-equivalence: cannot stat output {}: {e}",
            output.display()
        ))
    })?;
    if out_meta.len() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-equivalence: output is empty: {}",
            output.display()
        )));
    }

    let tolerance = pixel_equivalence_diff_tolerance_for_format(format).ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "pixel-equivalence: missing {format:?} orientation policy"
        ))
    })?;
    match verify_orientation_pixel_diff(source_jpeg, output, format, tolerance)? {
        PixelDiffResult::Match => {}
        PixelDiffResult::SkippedToolAbsent { tool } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "pixel-equivalence: proof unavailable for {}: missing {tool}",
                output.display()
            )));
        }
        PixelDiffResult::Mismatch { max_delta, channel } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "pixel-equivalence: proof failed for {}: max_delta={max_delta} channel={channel}",
                output.display()
            )));
        }
    }

    let source_hash = calculate_blake3_hash(source_jpeg).map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-equivalence: BLAKE3(source) failed: {e}"))
    })?;
    let output_hash = calculate_blake3_hash(output).map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-equivalence: BLAKE3(output) failed: {e}"))
    })?;

    tracing::info!(
        target: "fast_img_integrity",
        source = %source_jpeg.display(),
        source_blake3 = %source_hash,
        output = %output.display(),
        output_blake3 = %output_hash,
        "pixel-equivalence integrity check"
    );

    Ok(IntegrityResult::JxlPixelEquivalent {
        source_hash,
        output_hash,
    })
}

/// Verify a final delivered JXL is safe to use as the sole retained
/// JPEG-derived asset.
///
/// The final JXL rechecks byte-identical JPEG reconstruction after metadata
/// preservation, then audits orientation/structure without treating
/// cross-decoder channel rounding as media damage. Pixel equivalence alone is
/// never sufficient for JPEG-source delivery or deletion. A reconstructible
/// JXL may retain the source JPEG Orientation required by `jbrd`.
///
/// # Errors
/// Returns an error if any proof step fails.
pub fn verify_final_jxl_delivery_integrity(
    source_jpeg: &Path,
    jxl_output: &Path,
) -> Result<IntegrityResult> {
    use crate::image::format_detect::FormatKind;
    verify_final_delivery_integrity(source_jpeg, jxl_output, FormatKind::Jxl)
}

/// Same as [`verify_final_jxl_delivery_integrity`] but for AVIF meme mode.
///
/// # Errors
/// Returns an error if any proof step fails.
pub fn verify_final_avif_delivery_integrity(
    source_jpeg: &Path,
    avif_output: &Path,
) -> Result<IntegrityResult> {
    use crate::image::format_detect::FormatKind;
    verify_final_delivery_integrity(source_jpeg, avif_output, FormatKind::Avif)
}

/// Verify a final delivered modern format output is safe to use as the sole
/// retained JPEG-derived asset.
///
/// Supports JXL, AVIF, and other modern formats. Checks source/output hash,
/// non-empty output, decoder readability, and orientation-correct pixels.
/// JXL delivery requires byte-identical JPEG reconstruction. Other modern
/// formats use their format-specific pixel and orientation proof.
///
/// # Errors
/// Returns an error if any proof step fails.
pub fn verify_final_delivery_integrity(
    source_jpeg: &Path,
    output: &Path,
    format: crate::image::format_detect::FormatKind,
) -> Result<IntegrityResult> {
    use crate::common_utils::calculate_blake3_hash;
    use crate::image::orientation::{
        PixelDiffResult, pixel_equivalence_diff_tolerance_for_format, verify_orientation_pixel_diff,
    };

    let out_meta = std::fs::metadata(output).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "final-integrity: cannot stat output {}: {e}",
            output.display()
        ))
    })?;
    if out_meta.len() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "final-integrity: output is empty: {}",
            output.display()
        )));
    }

    // djxl is the decode-verification tool for JXL only; skip for other formats.
    if format == crate::image::format_detect::FormatKind::Jxl
        && !crate::DjxlBuilder::check_available()
    {
        return Err(ImgQualityError::AnalysisError(format!(
            "final-integrity: djxl unavailable; cannot verify final JXL {}",
            output.display()
        )));
    }

    let pixel_tolerance = pixel_equivalence_diff_tolerance_for_format(format).ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "final-integrity: missing {format:?} orientation policy"
        ))
    })?;
    let jxl_byte_reconstructible = if format == crate::image::format_detect::FormatKind::Jxl {
        verify_jxl_roundtrip_integrity(source_jpeg, output)?;
        true
    } else {
        false
    };
    let tolerance = if jxl_byte_reconstructible {
        crate::image::orientation::DiffTolerance::JxlOrientation
    } else {
        pixel_tolerance
    };
    match verify_orientation_pixel_diff(source_jpeg, output, format, tolerance)? {
        PixelDiffResult::Match => {}
        PixelDiffResult::SkippedToolAbsent { tool } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "final-integrity: orientation proof unavailable for {}: missing {tool}",
                output.display()
            )));
        }
        PixelDiffResult::Mismatch { max_delta, channel } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "final-integrity: orientation proof failed for {}: max_delta={max_delta} \
                 channel={channel}",
                output.display()
            )));
        }
    }

    let source_hash = calculate_blake3_hash(source_jpeg).map_err(|e| {
        ImgQualityError::AnalysisError(format!("final-integrity: BLAKE3(source) failed: {e}"))
    })?;
    let output_hash = calculate_blake3_hash(output).map_err(|e| {
        ImgQualityError::AnalysisError(format!("final-integrity: BLAKE3(output) failed: {e}"))
    })?;

    tracing::info!(
        target: "fast_img_integrity",
        source = %source_jpeg.display(),
        source_blake3 = %source_hash,
        output = %output.display(),
        output_blake3 = %output_hash,
        format = ?format,
        "final modern format delivery integrity check"
    );

    if jxl_byte_reconstructible {
        Ok(IntegrityResult::RoundtripMatch {
            source_hash,
            output_hash,
        })
    } else {
        Ok(IntegrityResult::FinalModernDelivery {
            source_hash,
            output_hash,
        })
    }
}

fn first_nonempty_tool_line(bytes: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.lines().map(str::trim).find(|line| !line.is_empty()),
        Err(err) => {
            tracing::debug!(
                target: "fast_img_integrity",
                error = %err,
                "suppressed tool output was not valid UTF-8"
            );
            None
        }
    }
}

fn log_suppressed_tool_output(message: &'static str, stdout: &[u8], stderr: &[u8]) {
    let stdout_line = first_nonempty_tool_line(stdout);
    let stderr_line = first_nonempty_tool_line(stderr);
    if stdout_line.is_some() || stderr_line.is_some() {
        tracing::debug!(
            target: "fast_img_integrity",
            stdout = stdout_line.unwrap_or(""),
            stderr = stderr_line.unwrap_or(""),
            "{message}"
        );
    }
}

/// Result of an integrity check.
#[derive(Debug)]
pub enum IntegrityResult {
    /// Full roundtrip BLAKE3 match — bit-exact lossless proof.
    RoundtripMatch {
        source_hash: String,
        output_hash: String,
    },
    /// Intermediate pixel-equivalence proof. This never authorizes source
    /// deletion or final JXL delivery.
    JxlPixelEquivalent {
        source_hash: String,
        output_hash: String,
    },
    /// Final non-JXL modern-format delivery passed source/output hash, decode,
    /// metadata, and orientation proofs.
    FinalModernDelivery {
        source_hash: String,
        output_hash: String,
    },
    /// djxl unavailable; output is non-empty and readable. \[D8\] for AVIF.
    DecodeProbePassed { output_hash: String },
}

/// Delete source JPEG with §Integrity as gate 1, then output-exists + size > 0
/// (§Delete Safety).
///
/// Gates (all atomic — abort without deletion if any fail):
/// 1. §Integrity passed (caller provides the `IntegrityResult`).
/// 2. Output exists on disk + size > 0.
/// 3. Log: `src_path` | `BLAKE3(src)` | `BLAKE3(output)` | timestamp.
///
/// → all pass → delete src.
///
/// \[D2\] abort+rollback — source preserved on any gate failure.
/// \[D6\] verified source deletion is mandatory after fast-img gates pass.
///
/// # Errors
/// Returns an error (source preserved) if any gate fails.
pub fn safe_delete_jpeg_source(
    source: &Path,
    output: &Path,
    integrity: &IntegrityResult,
) -> Result<()> {
    use crate::common_utils::calculate_blake3_hash;
    use crate::io_utils::safe_remove_file;

    // @ANCHOR:delete-gate — delete iff integrity_passed && output_size>0 &&
    // blake3_logged; atomic Gate 1: §Integrity must have passed (caller
    // provides proof)
    let (claimed_source_hash, claimed_output_hash) = match integrity {
        IntegrityResult::RoundtripMatch {
            source_hash,
            output_hash,
        } => {
            tracing::info!(
                target: "fast_img_delete",
                %source_hash,
                %output_hash,
                "delete-gate 1: roundtrip BLAKE3 match confirmed"
            );
            (source_hash, output_hash)
        }
        IntegrityResult::FinalModernDelivery {
            source_hash,
            output_hash,
        } => {
            tracing::info!(
                target: "fast_img_delete",
                %source_hash,
                %output_hash,
                "delete-gate 1: final JXL delivery proof confirmed"
            );
            (source_hash, output_hash)
        }
        IntegrityResult::JxlPixelEquivalent { output_hash, .. }
        | IntegrityResult::DecodeProbePassed { output_hash } => {
            tracing::error!(
                target: "fast_img_delete",
                %output_hash,
                "delete-gate 1 FAIL: non-exact integrity is not sufficient for source deletion"
            );
            return Err(ImgQualityError::AnalysisError(
                "delete-gate 1 FAIL: non-exact integrity is insufficient; exact JXL roundtrip or \
                 final non-JXL delivery proof is required before deleting source JPEG"
                    .to_string(),
            ));
        }
    };

    // Gate 2: output exists + non-empty
    if !output.exists() {
        return Err(ImgQualityError::AnalysisError(format!(
            "delete-gate 2 FAIL: output does not exist: {}",
            output.display()
        )));
    }
    let meta = std::fs::metadata(output).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "delete-gate 2 FAIL: cannot stat output {}: {e}",
            output.display()
        ))
    })?;
    if meta.len() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "delete-gate 2 FAIL: output is empty: {}",
            output.display()
        )));
    }

    let current_source_hash = calculate_blake3_hash(source).map_err(|e| {
        ImgQualityError::AnalysisError(format!("delete-gate 3 FAIL: BLAKE3(source) failed: {e}"))
    })?;
    let current_output_hash = calculate_blake3_hash(output).map_err(|e| {
        ImgQualityError::AnalysisError(format!("delete-gate 3 FAIL: BLAKE3(output) failed: {e}"))
    })?;
    if current_source_hash != *claimed_source_hash || current_output_hash != *claimed_output_hash {
        tracing::error!(
            target: "fast_img_delete",
            source = %source.display(),
            claimed_source_blake3 = %claimed_source_hash,
            current_source_blake3 = %current_source_hash,
            output = %output.display(),
            claimed_output_blake3 = %claimed_output_hash,
            current_output_blake3 = %current_output_hash,
            "delete-gate 3 FAIL: stale or forged roundtrip proof"
        );
        return Err(ImgQualityError::AnalysisError(
            "delete-gate 3 FAIL: stale or forged roundtrip proof".to_string(),
        ));
    }

    // Gate 3: audit log
    tracing::info!(
        target: "fast_img_delete",
        source = %source.display(),
        source_blake3 = %current_source_hash,
        output = %output.display(),
        output_blake3 = %current_output_hash,
        "delete-gate PASS: removing source JPEG"
    );

    let matching_xmp_sidecar = crate::metadata::find_xmp_sidecar(source);
    safe_remove_file(source).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "delete failed for {} (output preserved at {}): {e}",
            source.display(),
            output.display()
        ))
    })?;

    delete_matching_xmp_sidecar_path(source, output, matching_xmp_sidecar.as_deref())?;

    Ok(())
}

/// Delete the XMP sidecar that matches a source after the caller has already
/// verified the primary source is gone and the delivery proof is still current.
///
/// # Errors
/// Returns an error if a matching sidecar exists but cannot be removed.
pub fn safe_delete_matching_xmp_sidecar(source: &Path, output: &Path) -> Result<bool> {
    let matching_xmp_sidecar = crate::metadata::find_xmp_sidecar(source);
    delete_matching_xmp_sidecar_path(source, output, matching_xmp_sidecar.as_deref())
}

fn delete_matching_xmp_sidecar_path(
    source: &Path,
    output: &Path,
    matching_xmp_sidecar: Option<&Path>,
) -> Result<bool> {
    use crate::io_utils::safe_remove_file;

    let Some(xmp_sidecar) = matching_xmp_sidecar else {
        return Ok(false);
    };
    let metadata = std::fs::metadata(xmp_sidecar).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "delete-gate sidecar FAIL: cannot stat matching XMP sidecar {} for {}: {err}",
            xmp_sidecar.display(),
            source.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(ImgQualityError::AnalysisError(format!(
            "delete-gate sidecar FAIL: matching XMP sidecar is not a file: {}",
            xmp_sidecar.display()
        )));
    }
    tracing::info!(
        target: "fast_img_delete",
        source = %source.display(),
        output = %output.display(),
        xmp_sidecar = %xmp_sidecar.display(),
        "delete-gate PASS: removing source XMP after verified delivery"
    );
    safe_remove_file(xmp_sidecar).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "delete-gate sidecar FAIL: failed to delete matching XMP sidecar {} for {} (output \
             preserved at {}): {err}",
            xmp_sidecar.display(),
            source.display(),
            output.display()
        ))
    })?;
    Ok(true)
}

/// Delete a tier-2 lossy modern static source after Photos import proof passes.
///
/// Gates (all atomic — abort without deletion if any fail):
/// 1. Photos library BLAKE3 matches the claimed source hash.
/// 2. Source exists on disk + size > 0.
/// 3. Current source BLAKE3 matches the claimed hash.
///
/// # Errors
/// Returns an error (source preserved) if any gate fails.
pub fn safe_delete_modern_lossy_static_source(
    source: &Path,
    import_proof: &crate::pipeline::verification::LibraryAssetRecord,
) -> Result<()> {
    use crate::common_utils::calculate_blake3_hash;
    use crate::io_utils::safe_remove_file;

    let claimed_blake3 = import_proof.blake3.as_str();
    if import_proof.quarantined {
        return Err(ImgQualityError::AnalysisError(format!(
            "delete-gate 1 FAIL: Photos import proof for {} is quarantined",
            source.display()
        )));
    }
    tracing::info!(
        target: "fast_img_delete",
        source = %source.display(),
        library_blake3 = %claimed_blake3,
        photos_uuid = import_proof.photos_uuid.as_deref().unwrap_or("<missing>"),
        "delete-gate 1: Photos import BLAKE3 proof confirmed"
    );

    if !source.exists() {
        return Err(ImgQualityError::AnalysisError(format!(
            "delete-gate 2 FAIL: source does not exist: {}",
            source.display()
        )));
    }
    let meta = std::fs::metadata(source).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "delete-gate 2 FAIL: cannot stat source {}: {e}",
            source.display()
        ))
    })?;
    if meta.len() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "delete-gate 2 FAIL: source is empty: {}",
            source.display()
        )));
    }

    let current_source_hash = calculate_blake3_hash(source).map_err(|e| {
        ImgQualityError::AnalysisError(format!("delete-gate 3 FAIL: BLAKE3(source) failed: {e}"))
    })?;
    if current_source_hash != claimed_blake3 {
        tracing::error!(
            target: "fast_img_delete",
            source = %source.display(),
            claimed_blake3 = %claimed_blake3,
            current_source_blake3 = %current_source_hash,
            "delete-gate 3 FAIL: stale or forged source hash"
        );
        return Err(ImgQualityError::AnalysisError(
            "delete-gate 3 FAIL: stale or forged source hash".to_string(),
        ));
    }

    tracing::info!(
        target: "fast_img_delete",
        source = %source.display(),
        source_blake3 = %current_source_hash,
        "delete-gate PASS: removing tier-2 lossy modern static source"
    );

    let matching_xmp_sidecar = crate::metadata::find_xmp_sidecar(source);
    safe_remove_file(source).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "delete failed for tier-2 source {} (Photos custody preserved): {e}",
            source.display()
        ))
    })?;
    delete_matching_xmp_sidecar_path(source, source, matching_xmp_sidecar.as_deref())?;
    Ok(())
}

/// Re-query Photos custody for tier-2 imports before destructive delete.
///
/// UUID order from `osxphotos query` is undefined; matching is always by UUID key,
/// never by response position.
pub fn reverify_modern_lossy_static_photos_custody(
    library_handle: &crate::pipeline::verification::LibraryHandle,
) -> Result<()> {
    if library_handle.imported_assets.is_empty() {
        return Ok(());
    }

    let mut uuids = Vec::with_capacity(library_handle.imported_assets.len());
    for asset in &library_handle.imported_assets {
        let Some(uuid) = asset
            .photos_uuid
            .as_deref()
            .filter(|value| !value.is_empty())
        else {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate missing Photos UUID for {}",
                asset.rel_path
            )));
        };
        uuids.push(uuid.to_string());
    }

    println!(
        "[VERIFY  ] final tier-2 Photos delete proofs pending {} · osxphotos custody re-check (UUID-keyed)",
        uuids.len()
    );
    tracing::info!(
        target: "fast_img_delete",
        pending = uuids.len(),
        "tier-2 final Photos custody verification start"
    );

    let probes = query_osxphotos_asset_probes(&uuids)?;
    let probe_by_uuid = index_photos_probes_by_uuid(&uuids, probes)?;

    for asset in &library_handle.imported_assets {
        let Some(uuid) = asset.photos_uuid.as_deref() else {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate missing Photos UUID for {}",
                asset.rel_path
            )));
        };
        let Some(probe) = probe_by_uuid.get(uuid) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate missing Photos probe for {} (uuid={uuid})",
                asset.rel_path
            )));
        };
        if probe.uuid != uuid {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate Photos UUID mismatch for {}: expected={uuid} query={}",
                asset.rel_path, probe.uuid
            )));
        }
        if probe.ismissing {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate Photos asset missing for {} (uuid={uuid})",
                asset.rel_path
            )));
        }
        if !probe.path.exists() {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate Photos asset path missing for {}: {}",
                asset.rel_path,
                probe.path.display()
            )));
        }
        let library_blake3 =
            crate::common_utils::calculate_blake3_hash(&probe.path).map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "tier-2 delete gate Photos BLAKE3 failed for {}: {err}",
                    probe.path.display()
                ))
            })?;
        let expected_library_blake3 = asset.library_blake3.as_deref().unwrap_or(&asset.blake3);
        if library_blake3 != expected_library_blake3 {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate Photos BLAKE3 drift for {} (uuid={uuid}): expected={expected_library_blake3} library={library_blake3}",
                asset.rel_path
            )));
        }
    }
    Ok(())
}

fn preflight_modern_lossy_static_source_deletion(
    src_dir: &Path,
    library_handle: &crate::pipeline::verification::LibraryHandle,
) -> Result<()> {
    if library_handle.import_error_count != 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "tier-2 source delete gate refuses import errors: {}",
            library_handle.import_error_count
        )));
    }
    let mut seen = BTreeSet::new();
    for asset in &library_handle.imported_assets {
        if asset.rel_path.trim().is_empty() || asset.blake3.trim().is_empty() {
            return Err(ImgQualityError::AnalysisError(
                "tier-2 source delete gate has incomplete import proof".to_string(),
            ));
        }
        if asset.quarantined {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 source delete gate refuses quarantined import proof for {}",
                asset.rel_path
            )));
        }
        let rel = Path::new(&asset.rel_path);
        if rel
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || !seen.insert(asset.rel_path.as_str())
        {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 source delete gate rejects unsafe or duplicate relative path: {}",
                asset.rel_path
            )));
        }
        let source = src_dir.join(rel);
        if source.exists() {
            let metadata = std::fs::metadata(&source).map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "tier-2 source delete preflight cannot stat {}: {err}",
                    source.display()
                ))
            })?;
            if !metadata.is_file() || metadata.len() == 0 {
                return Err(ImgQualityError::AnalysisError(format!(
                    "tier-2 source delete preflight requires a non-empty file: {}",
                    source.display()
                )));
            }
            let current = crate::common_utils::calculate_blake3_hash(&source).map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "tier-2 source delete preflight BLAKE3 failed for {}: {err}",
                    source.display()
                ))
            })?;
            if current != asset.blake3 {
                return Err(ImgQualityError::AnalysisError(format!(
                    "tier-2 source delete preflight detected source drift: {}",
                    source.display()
                )));
            }
        }
    }
    Ok(())
}

/// Delete tier-2 lossy modern static sources after Gate 3 and Photos custody re-verification.
pub fn delete_verified_modern_lossy_static_sources(
    src_dir: &Path,
    library_handle: &crate::pipeline::verification::LibraryHandle,
) -> Result<(usize, usize)> {
    if library_handle.imported_assets.is_empty() {
        return Ok((0, 0));
    }
    preflight_modern_lossy_static_source_deletion(src_dir, library_handle)?;
    reverify_modern_lossy_static_photos_custody(library_handle)?;

    let mut deleted = 0usize;
    let mut already_deleted = 0usize;
    for asset in &library_handle.imported_assets {
        let source = src_dir.join(&asset.rel_path);
        if !source.exists() {
            safe_delete_matching_xmp_sidecar(&source, &source).map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "tier-2 delete failed to remove matching XMP sidecar for already-absent source {}: {err}",
                    source.display()
                ))
            })?;
            already_deleted += 1;
            tracing::info!(
                target: "fast_img_delete",
                source = %source.display(),
                "tier-2 verified source already absent"
            );
            continue;
        }
        safe_delete_modern_lossy_static_source(&source, asset).map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "tier-2 delete failed for verified source {}: {err}",
                source.display()
            ))
        })?;
        deleted += 1;
    }
    Ok((deleted, already_deleted))
}

/// Persist tier-2 Photos import proof on the working-copy marker for resume/delete.
pub fn apply_tier2_library_assets_to_marker(
    marker: &mut crate::pipeline::verification::WorkingCopyMarker,
    library: &crate::pipeline::verification::LibraryHandle,
) -> Result<()> {
    for asset in &library.imported_assets {
        marker
            .tier2_imported_assets
            .retain(|persisted| persisted.rel_path != asset.rel_path);
        marker.tier2_imported_assets.push(asset.clone());
    }
    marker
        .tier2_imported_assets
        .sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(())
}

/// Rebuild tier-2 import proof from a persisted marker.
#[must_use]
pub fn library_handle_from_marker_tier2_proof(
    marker: &crate::pipeline::verification::WorkingCopyMarker,
) -> Option<crate::pipeline::verification::LibraryHandle> {
    if marker.tier2_imported_assets.is_empty() {
        return None;
    }
    Some(crate::pipeline::verification::LibraryHandle {
        imported_assets: marker.tier2_imported_assets.clone(),
        import_error_count: 0,
    })
}

/// Prune empty directories under `src_dir` that previously held tier-2 sources.
pub fn prune_empty_source_dirs_for_tier2_assets(
    src_dir: &Path,
    imported_assets: &[crate::pipeline::verification::LibraryAssetRecord],
    remove_selected_root: bool,
) -> Result<usize> {
    if !src_dir.is_dir() {
        return Ok(0);
    }
    let mut dirs = Vec::new();
    for asset in imported_assets {
        let source = src_dir.join(&asset.rel_path);
        if let Some(dir) = source.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    let result = if remove_selected_root {
        crate::io_utils::prune_empty_directories_within(src_dir, &dirs)
    } else {
        crate::io_utils::prune_empty_descendants_within(src_dir, &dirs)
    };
    result.map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "prune empty tier-2 source directories under {}: {err}",
            src_dir.display()
        ))
    })
}

#[derive(Debug, Clone)]
pub struct FastImgLibraryAssetProbe {
    pub uuid: String,
    pub path: PathBuf,
    pub iscloudasset: bool,
    pub incloud: Option<bool>,
    pub ismissing: bool,
}

#[derive(Debug, Clone)]
pub struct PhotosImportCandidate {
    pub rel_path: String,
    pub path: PathBuf,
    pub blake3: String,
    pub album_name: String,
}

#[derive(Debug, Default)]
struct PhotosMediaImportReport {
    report_pairs: Vec<(String, String)>,
    failed_count: usize,
}

#[derive(Debug, Deserialize)]
struct FastImgQueryRecord {
    uuid: String,
    path: String,
    iscloudasset: bool,
    incloud: Option<bool>,
    ismissing: bool,
}

#[derive(Debug)]
struct VerifiedLibraryProbe {
    probe: FastImgLibraryAssetProbe,
    blake3: String,
}

#[derive(Debug, Clone)]
struct PhotosImportPendingEntry {
    source_rel: String,
    rel_path: String,
    path: PathBuf,
    album_name: String,
    blake3_entry: Blake3Entry,
}

#[derive(Debug)]
struct PhotosImportCheckpointPlan {
    pending_entries: Vec<PhotosImportPendingEntry>,
    proven_assets: Vec<LibraryAssetRecord>,
}

#[derive(Debug, Default)]
struct PhotosCheckpointImportReport {
    imported_assets: Vec<LibraryAssetRecord>,
    failed_count: usize,
}

enum PhotosImportBatchOutcome {
    Imported(Vec<LibraryAssetRecord>),
    DeferredItem { source_rel: String, detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PhotosImportWindow {
    start: usize,
    len: usize,
    relaunch_photos_before: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhotosImportStrategy {
    FastSmallSet,
    StableCheckpointed,
}

impl PhotosImportStrategy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::FastSmallSet => "fast_small_set",
            Self::StableCheckpointed => "stable_checkpointed",
        }
    }
}

fn photos_import_fail_fast_enabled() -> bool {
    BatchErrorMode::current().is_fail_fast()
}

/// Import fast-img outputs into Photos with per-item UUID checkpoints.
///
/// This deliberately does not use `osxphotos import`: current osxphotos filters
/// `.JXL` at the CLI media-type layer before Photos.app sees the file. Instead,
/// Photos `AppleScript` performs the import and returns machine-readable UUIDs;
/// `osxphotos query` remains the verifier for local asset path and BLAKE3.
/// iCloud upload completion is deliberately not required by default because
/// that can apply sustained pressure to Photos/cloud daemons. Local custody
/// proof still uses a small bounded retry because Photos can return from import
/// before every new asset is query-visible. Set
/// `MFB_FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF=1` only when the caller explicitly
/// accepts that cost. If the selected proof cannot be established,
/// shortest-path mode fails closed while preserving its output directory and
/// source media.
/// On an explicit resume, persisted UUIDs are re-queried and any asset that
/// reached the target album before the last process stopped is reconciled
/// before pending files are imported again.
pub fn import_media_outputs_with_checkpointed_library_verifier(
    marker: &WorkingCopyMarker,
    reconcile_existing: bool,
) -> Result<LibraryHandle> {
    let _photos_import_lock = acquire_photos_import_lock()?;
    let output_paths = fast_img_marker_output_paths(marker)?;
    validate_fast_img_marker_output_hashes(marker)?;
    #[cfg(target_os = "macos")]
    let quarantine_probe = path_has_quarantine_xattr;
    #[cfg(not(target_os = "macos"))]
    let quarantine_probe = |path: &Path| Ok(path_has_quarantine_xattr(path));
    import_marker_outputs_with_photos_checkpoint(
        marker,
        &output_paths,
        reconcile_existing,
        query_osxphotos_asset_probes,
        quarantine_probe,
    )
}

pub fn import_media_outputs_with_library_verifier(
    candidates: &[PhotosImportCandidate],
) -> Result<LibraryHandle> {
    let _photos_import_lock = acquire_photos_import_lock()?;
    let import_report = import_media_outputs_with_photos_applescript(candidates)?;
    let imported_rel_paths = import_report
        .report_pairs
        .iter()
        .map(|(rel_path, _)| rel_path.as_str())
        .collect::<BTreeSet<_>>();
    let imported_candidates = candidates
        .iter()
        .filter(|candidate| imported_rel_paths.contains(candidate.rel_path.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    #[cfg(target_os = "macos")]
    let quarantine_probe = path_has_quarantine_xattr;
    #[cfg(not(target_os = "macos"))]
    let quarantine_probe = |path: &Path| Ok(path_has_quarantine_xattr(path));
    let mut library_handle = library_handle_from_media_output_probes(
        &imported_candidates,
        &import_report.report_pairs,
        query_osxphotos_asset_probes,
        quarantine_probe,
    )?;
    library_handle.import_error_count = import_report.failed_count;
    Ok(library_handle)
}

fn photos_import_report_pairs_from_persisted_assets(
    assets: &[LibraryAssetRecord],
) -> Result<Vec<(String, String)>> {
    assets
        .iter()
        .map(|asset| {
            let uuid = asset
                .photos_uuid
                .as_deref()
                .filter(|uuid| !uuid.is_empty())
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError(format!(
                        "persisted Photos import proof has no UUID for {}",
                        asset.rel_path
                    ))
                })?;
            Ok((asset.rel_path.clone(), uuid.to_string()))
        })
        .collect()
}

/// Re-query previously imported Photos UUIDs without importing the files again.
pub fn reverify_media_outputs_with_library_verifier(
    candidates: &[PhotosImportCandidate],
    persisted_assets: &[LibraryAssetRecord],
) -> Result<LibraryHandle> {
    let _photos_import_lock = acquire_photos_import_lock()?;
    let report_pairs = photos_import_report_pairs_from_persisted_assets(persisted_assets)?;
    #[cfg(target_os = "macos")]
    let quarantine_probe = path_has_quarantine_xattr;
    #[cfg(not(target_os = "macos"))]
    let quarantine_probe = |path: &Path| Ok(path_has_quarantine_xattr(path));
    library_handle_from_media_output_probes(
        candidates,
        &report_pairs,
        query_osxphotos_asset_probes,
        quarantine_probe,
    )
}

/// Build Photos import rows from the exact outputs recorded by a fast-img run.
///
/// Unlike the legacy JXL-only importer, this preserves each marker entry's
/// output extension so shortest-path AVIF delivery can use the same custody
/// verification without treating AVIF files as JXL.
pub fn build_fast_img_output_import_candidates(
    marker: &WorkingCopyMarker,
) -> Result<Vec<PhotosImportCandidate>> {
    validate_fast_img_marker_path_contract(marker)?;
    let mut candidates = marker
        .blake3_log
        .iter()
        .map(|(source_rel, entry)| {
            let rel_path = marker_entry_out_rel(source_rel, entry);
            PhotosImportCandidate {
                path: marker.working_copy.join(&rel_path),
                album_name: fast_img_optimized_import_album_name(marker, &rel_path),
                rel_path,
                blake3: entry.out.clone(),
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(candidates)
}

/// Build Photos import rows for fast-img tier 2 (lossy modern static originals).
#[must_use]
pub fn build_modern_lossy_static_import_candidates(
    src_dir: &Path,
    candidates: &[super::modern_lossy_static::ModernLossyStaticCandidate],
) -> Vec<PhotosImportCandidate> {
    let folder_name = src_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Imported");
    let cleaned = fast_img_strip_optimized_import_suffixes(folder_name);
    let inner_root = if cleaned.is_empty() {
        "✨Imported".to_string()
    } else if !cleaned.starts_with('✨') {
        format!("✨{cleaned}")
    } else {
        cleaned
    };

    candidates
        .iter()
        .map(|candidate| {
            let rel_parent = Path::new(&candidate.rel_path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .and_then(|parent| parent.to_str());
            let album_name = if let Some(sub) = rel_parent {
                format!("✨/{inner_root}/{sub}")
            } else {
                format!("✨/{inner_root}")
            };
            PhotosImportCandidate {
                rel_path: candidate.rel_path.clone(),
                path: candidate.path.clone(),
                blake3: candidate.blake3.clone(),
                album_name,
            }
        })
        .collect()
}

/// Import tier-2 lossy modern static sources directly into Photos.
pub fn import_modern_lossy_static_tier(
    src_dir: &Path,
    candidates: &[super::modern_lossy_static::ModernLossyStaticCandidate],
) -> Result<LibraryHandle> {
    if candidates.is_empty() {
        return Ok(LibraryHandle::default());
    }
    let import_candidates = build_modern_lossy_static_import_candidates(src_dir, candidates);
    import_or_reconcile_modern_lossy_static_candidates(&import_candidates)
}

fn import_or_reconcile_modern_lossy_static_candidates(
    candidates: &[PhotosImportCandidate],
) -> Result<LibraryHandle> {
    let _photos_import_lock = acquire_photos_import_lock()?;
    validate_photos_import_candidates(candidates)?;
    let mut imported_assets = Vec::new();
    let mut failed_count = 0usize;
    for candidate in candidates {
        let handle = import_or_reconcile_single_modern_lossy_candidate(candidate)?;
        imported_assets.extend(handle.imported_assets);
        failed_count = failed_count
            .checked_add(handle.import_error_count)
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "tier-2 Photos import failure count overflowed".to_string(),
                )
            })?;
    }
    imported_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(LibraryHandle {
        imported_assets,
        import_error_count: failed_count,
    })
}

fn reconcile_single_modern_lossy_candidate(
    candidate: &PhotosImportCandidate,
) -> Result<Option<LibraryAssetRecord>> {
    let candidates = std::slice::from_ref(candidate);
    let manifest_entries = photos_import_candidate_manifest_entries(candidates);
    let stdout = run_photos_import_applescript_session_mode(
        "tier-2 media reconciliation",
        &manifest_entries,
        "reconcile_all",
    )?;
    let candidate_ids = photos_reconciled_candidate_ids(candidates, &stdout)?;
    if candidate_ids[0].is_empty() {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    let quarantine_probe = path_has_quarantine_xattr;
    #[cfg(not(target_os = "macos"))]
    let quarantine_probe = |path: &Path| Ok(path_has_quarantine_xattr(path));
    for identifier in &candidate_ids[0] {
        let report_pair = vec![(candidate.rel_path.clone(), identifier.clone())];
        match library_handle_from_media_output_probes(
            candidates,
            &report_pair,
            query_osxphotos_asset_probes,
            quarantine_probe,
        ) {
            Ok(mut handle) => return Ok(handle.imported_assets.pop()),
            Err(err) if photos_reconciliation_content_mismatch(&err.to_string()) => {
                tracing::warn!(
                    target: "photos_import",
                    rel_path = %candidate.rel_path,
                    photos_uuid = %identifier,
                    error = %err,
                    "tier-2 filename match had different content; checking remaining matches"
                );
            }
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

fn reconcile_single_modern_lossy_candidate_with_recovery(
    candidate: &PhotosImportCandidate,
) -> Result<Option<LibraryAssetRecord>> {
    let mut poisoned_attempts = 0usize;
    loop {
        match reconcile_single_modern_lossy_candidate(candidate) {
            Ok(asset) => return Ok(asset),
            Err(err) => {
                let detail = err.to_string();
                let Some(reason) = photos_import_retry_reason(&detail) else {
                    return Err(err);
                };
                if poisoned_attempts >= FAST_IMG_PHOTOS_IMPORT_POISON_RETRY_LIMIT {
                    return Err(err);
                }
                handle_photos_import_recovery(
                    reason,
                    &mut relaunch_photos_for_import_recovery,
                    &mut probe_photos_import_session_health,
                )?;
                poisoned_attempts = poisoned_attempts.checked_add(1).ok_or_else(|| {
                    ImgQualityError::AnalysisError(
                        "tier-2 Photos reconciliation retry counter overflowed".to_string(),
                    )
                })?;
            }
        }
    }
}

fn photos_reconciliation_content_mismatch(detail: &str) -> bool {
    detail.contains("Photos verifier pixel-equivalence: mismatch")
        || detail.contains("Photos verifier BLAKE3 mismatch")
}

fn import_or_reconcile_single_modern_lossy_candidate(
    candidate: &PhotosImportCandidate,
) -> Result<LibraryHandle> {
    if let Some(asset) = reconcile_single_modern_lossy_candidate_with_recovery(candidate)? {
        return Ok(LibraryHandle {
            imported_assets: vec![asset],
            import_error_count: 0,
        });
    }

    let candidates = std::slice::from_ref(candidate);
    let mut attempt = 0usize;
    loop {
        let mut run_import_batch = |manifest_entries: &[(PathBuf, String)]| {
            run_photos_import_applescript_session("media", manifest_entries)
        };
        match import_media_outputs_with_photos_applescript_with(
            candidates,
            true,
            &mut run_import_batch,
        ) {
            Ok(report) => {
                #[cfg(target_os = "macos")]
                let quarantine_probe = path_has_quarantine_xattr;
                #[cfg(not(target_os = "macos"))]
                let quarantine_probe = |path: &Path| Ok(path_has_quarantine_xattr(path));
                return library_handle_from_media_output_probes(
                    candidates,
                    &report.report_pairs,
                    query_osxphotos_asset_probes,
                    quarantine_probe,
                );
            }
            Err(err) => {
                // A timed-out AppleEvent may have committed. Reconcile content
                // before every retry so an ambiguous result cannot duplicate it.
                if let Some(asset) =
                    reconcile_single_modern_lossy_candidate_with_recovery(candidate)?
                {
                    return Ok(LibraryHandle {
                        imported_assets: vec![asset],
                        import_error_count: 0,
                    });
                }
                let detail = err.to_string();
                let retry_reason = photos_import_retry_reason(&detail);
                if let Some(reason) = retry_reason
                    && attempt < FAST_IMG_PHOTOS_IMPORT_POISON_RETRY_LIMIT
                {
                    handle_photos_import_recovery(
                        reason,
                        &mut relaunch_photos_for_import_recovery,
                        &mut probe_photos_import_session_health,
                    )?;
                    attempt = attempt.checked_add(1).ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "tier-2 Photos retry counter overflowed".to_string(),
                        )
                    })?;
                    continue;
                }
                if photos_import_controllable_item_failure(&detail) {
                    return Ok(LibraryHandle {
                        imported_assets: Vec::new(),
                        import_error_count: 1,
                    });
                }
                return Err(err);
            }
        }
    }
}

fn photos_reconciled_candidate_ids(
    candidates: &[PhotosImportCandidate],
    stdout: &str,
) -> Result<Vec<Vec<String>>> {
    let lines = stdout.lines().map(str::trim).collect::<Vec<_>>();
    if lines.len() != candidates.len() {
        return Err(ImgQualityError::AnalysisError(format!(
            "tier-2 Photos reconciliation returned {} row(s) for {} candidate(s)",
            lines.len(),
            candidates.len()
        )));
    }
    Ok(lines
        .into_iter()
        .map(|identifiers| {
            identifiers
                .split('|')
                .map(str::trim)
                .filter(|identifier| !identifier.is_empty() && *identifier != "MFB_NOT_FOUND")
                .map(ToOwned::to_owned)
                .collect()
        })
        .collect())
}

const FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT: &str = r#"
on run argv
    if (count of argv) is not 7 then
        error "Photos import expected manifest path, batch size, batch delay, digest pause interval, digest pause seconds, hard timeout, and operation arguments"
    end if
    set manifestPath to item 1 of argv
    set batchSize to (item 2 of argv) as integer
    set batchDelayMs to (item 3 of argv) as integer
    set digestPauseInterval to (item 4 of argv) as integer
    set digestPauseSecs to (item 5 of argv) as integer
    set hardTimeoutSecs to (item 6 of argv) as integer
    set operationMode to item 7 of argv
    if batchSize < 1 then
        error "Photos import batch size must be at least 1"
    end if
    set rootFolderName to "✨"
    set skipDuplicateCheck to true
    set oldDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to linefeed
    set manifestText to read (POSIX file manifestPath) as «class utf8»
    if manifestText is "" then
        set AppleScript's text item delimiters to oldDelimiters
        return ""
    end if
    set manifestLines to every text item of manifestText
    if ((count of manifestLines) mod 2) is not 0 then
        error "Photos import manifest expected path/album line pairs"
    end if
    if operationMode is "reconcile_all" then
        set reconciledIds to {}
        repeat with lineIndex from 1 to (count of manifestLines) by 2
            set rawPath to item lineIndex of manifestLines
            set albumName to item (lineIndex + 1) of manifestLines
            set end of reconciledIds to my mfbFindExistingImportIds(contents of rawPath, contents of albumName)
        end repeat
        set resultText to reconciledIds as text
        set AppleScript's text item delimiters to oldDelimiters
        return resultText
    else if operationMode is "reconcile" then
        set reconciledIds to {}
        repeat with lineIndex from 1 to (count of manifestLines) by 2
            set rawPath to item lineIndex of manifestLines
            set albumName to item (lineIndex + 1) of manifestLines
            set end of reconciledIds to my mfbFindExistingImportId(contents of rawPath, contents of albumName)
        end repeat
        set resultText to reconciledIds as text
        set AppleScript's text item delimiters to oldDelimiters
        return resultText
    else if operationMode is not "import" then
        error "Photos import received an invalid operation"
    end if
    set importedIds to {}
    set batchNumber to 0
    with timeout of hardTimeoutSecs seconds
        tell application "Photos" to launch
        set currentAlbumName to ""
        set fileList to {}
        set rawPaths to {}
        repeat with lineIndex from 1 to (count of manifestLines) by 2
            set rawPath to item lineIndex of manifestLines
            set albumName to item (lineIndex + 1) of manifestLines
            set importPath to POSIX file (contents of rawPath)
            set mustFlush to false
            if (count of fileList) > 0 then
                if albumName is not currentAlbumName then
                    set mustFlush to true
                else if (count of fileList) is greater than or equal to batchSize then
                    set mustFlush to true
                end if
            end if
            if mustFlush then
                set batchNumber to batchNumber + 1
                set importedIds to importedIds & (my mfbImportFileList(fileList, rawPaths, currentAlbumName, skipDuplicateCheck, batchNumber))
                set fileList to {}
                set rawPaths to {}
                if batchDelayMs > 0 then
                    delay (batchDelayMs / 1000)
                end if
                if digestPauseInterval > 0 and digestPauseSecs > 0 then
                    if (batchNumber mod digestPauseInterval) is 0 then
                        delay digestPauseSecs
                    end if
                end if
            end if
            if (count of fileList) is 0 then
                set currentAlbumName to albumName
            end if
            set end of rawPaths to (contents of rawPath)
            set end of fileList to importPath
        end repeat
        if (count of fileList) > 0 then
            set batchNumber to batchNumber + 1
            set importedIds to importedIds & (my mfbImportFileList(fileList, rawPaths, currentAlbumName, skipDuplicateCheck, batchNumber))
        end if
    end timeout
    set resultText to importedIds as text
    set AppleScript's text item delimiters to oldDelimiters
    return resultText
end run

on mfbImportFileList(fileList, rawPaths, albumName, skipDuplicateCheck, batchNumber)
    set firstPath to item 1 of rawPaths
    set expectedCount to count of fileList
    set targetAlbumId to my mfbEnsureAlbumIdForPath(albumName)
    tell application "Photos"
                set importedItems to import fileList into album id (targetAlbumId) skip check duplicates skipDuplicateCheck
    end tell
    if importedItems is missing value then
        error "Photos returned missing value for " & firstPath & " (batch " & (batchNumber as text) & ")"
    end if
    set importedCount to count of importedItems
    if importedCount is not expectedCount then
        if expectedCount is 1 then
            error "Photos returned " & (importedCount as text) & " imported items for " & firstPath & " (batch " & (batchNumber as text) & ")"
        end if
        error "Photos returned " & (importedCount as text) & " imported items for batch starting " & firstPath & " (expected " & (expectedCount as text) & ", batch " & (batchNumber as text) & ")"
    end if
    set batchIds to {}
    repeat with importedItem in importedItems
        set end of batchIds to (id of importedItem as text)
    end repeat
    return batchIds
end mfbImportFileList

on mfbPathBasename(rawPath)
    set savedDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to "/"
    set pathItems to every text item of rawPath
    set AppleScript's text item delimiters to savedDelimiters
    return item (count of pathItems) of pathItems
end mfbPathBasename

on mfbFindExistingAlbumId(albumPath)
    set savedDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to "/"
    set rawPathItems to every text item of albumPath
    set AppleScript's text item delimiters to savedDelimiters
    set pathItems to {}
    repeat with rawPathItem in rawPathItems
        set pathItem to contents of rawPathItem
        if pathItem is not "" then set end of pathItems to pathItem
    end repeat
    if (count of pathItems) is 0 then return missing value
    tell application "Photos"
        try
            if (count of pathItems) is 1 then
                return (id of (first album whose name is item 1 of pathItems) as text)
            end if
            set targetFolder to first folder whose name is item 1 of pathItems
            if (count of pathItems) > 2 then
                repeat with pathIndex from 2 to ((count of pathItems) - 1)
                    set targetFolder to first folder of targetFolder whose name is item pathIndex of pathItems
                end repeat
            end if
            return (id of (first album of targetFolder whose name is item (count of pathItems) of pathItems) as text)
        on error
            return missing value
        end try
    end tell
end mfbFindExistingAlbumId

on mfbFindExistingImportId(rawPath, albumPath)
    set targetAlbumId to my mfbFindExistingAlbumId(albumPath)
    if targetAlbumId is missing value then return "MFB_NOT_FOUND"
    set expectedFilename to my mfbPathBasename(rawPath)
    tell application "Photos"
        try
            set matchingItems to every media item of album id targetAlbumId whose filename is expectedFilename
            if (count of matchingItems) is 0 then return "MFB_NOT_FOUND"
            return (id of last item of matchingItems as text)
        on error
            return "MFB_NOT_FOUND"
        end try
    end tell
end mfbFindExistingImportId

on mfbFindExistingImportIds(rawPath, albumPath)
    set targetAlbumId to my mfbFindExistingAlbumId(albumPath)
    if targetAlbumId is missing value then return "MFB_NOT_FOUND"
    set expectedFilename to my mfbPathBasename(rawPath)
    tell application "Photos"
        try
            set matchingItems to every media item of album id targetAlbumId whose filename is expectedFilename
            if (count of matchingItems) is 0 then return "MFB_NOT_FOUND"
            set matchingIds to {}
            repeat with matchingItem in matchingItems
                set end of matchingIds to (id of matchingItem as text)
            end repeat
            set savedDelimiters to AppleScript's text item delimiters
            set AppleScript's text item delimiters to "|"
            set resultText to matchingIds as text
            set AppleScript's text item delimiters to savedDelimiters
            return resultText
        on error
            return "MFB_NOT_FOUND"
        end try
    end tell
end mfbFindExistingImportIds

on mfbEnsureTopLevelFolder(folderName)
    tell application "Photos"
        try
            return first folder whose name is folderName
        on error
            return make new folder named folderName
        end try
    end tell
end mfbEnsureTopLevelFolder

on mfbEnsureChildFolder(parentFolder, folderName)
    tell application "Photos"
        try
            return first folder of parentFolder whose name is folderName
        on error
            return make new folder named folderName at parentFolder
        end try
    end tell
end mfbEnsureChildFolder

on mfbEnsureAlbumIdForPath(albumPath)
    set savedDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to "/"
    set rawPathItems to every text item of albumPath
    set AppleScript's text item delimiters to savedDelimiters
    set pathItems to {}
    repeat with rawPathItem in rawPathItems
        set pathItem to contents of rawPathItem
        if pathItem is not "" then
            set end of pathItems to pathItem
        end if
    end repeat
    if (count of pathItems) is 0 then
        error "Photos import received an empty album path"
    end if
    if (count of pathItems) > 1 then
        set targetFolder to my mfbEnsureTopLevelFolder(item 1 of pathItems)
        if (count of pathItems) > 2 then
            repeat with pathIndex from 2 to ((count of pathItems) - 1)
                set targetFolder to my mfbEnsureChildFolder(targetFolder, item pathIndex of pathItems)
            end repeat
        end if
        set targetAlbumName to item (count of pathItems) of pathItems
        tell application "Photos"
            try
                return (id of (first album of targetFolder whose name is targetAlbumName) as text)
            on error
                set createdAlbum to make new album named targetAlbumName at targetFolder
                return (id of createdAlbum as text)
            end try
        end tell
    else
        set targetAlbumName to item 1 of pathItems
        tell application "Photos"
            try
                return (id of (first album whose name is targetAlbumName) as text)
            on error
                set createdAlbum to make new album named targetAlbumName
                return (id of createdAlbum as text)
            end try
        end tell
    end if
end mfbEnsureAlbumIdForPath
"#;

fn import_marker_outputs_with_photos_checkpoint<Q, P>(
    marker: &WorkingCopyMarker,
    output_paths: &[(String, PathBuf)],
    reconcile_existing: bool,
    mut query_assets: Q,
    mut is_quarantined: P,
) -> Result<LibraryHandle>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    prepare_photos_import_output_paths(output_paths)?;
    let mut checkpoint_marker = marker.clone();
    let mut plan = photos_import_checkpoint_plan(&checkpoint_marker, &mut is_quarantined)?;
    if reconcile_existing {
        reverify_checkpointed_photos_assets(
            &checkpoint_marker,
            &plan.proven_assets,
            &mut query_assets,
            &mut is_quarantined,
        )?;
        let mut reconcile_imports = |entries: &[(PathBuf, String)]| {
            run_photos_import_applescript_session_mode("media reconciliation", entries, "reconcile")
        };
        reconcile_uncheckpointed_photos_assets(
            &mut checkpoint_marker,
            &plan.pending_entries,
            &mut query_assets,
            &mut is_quarantined,
            &mut reconcile_imports,
        )?;
        plan = photos_import_checkpoint_plan(&checkpoint_marker, &mut is_quarantined)?;
    }
    let expected_output_count = marker.expected_output_count();
    let PhotosImportCheckpointPlan {
        pending_entries,
        proven_assets,
    } = plan;
    let mut imported_assets = proven_assets;
    let mut prepare_import_session = prepare_photos_import_session;
    let mut run_import_batch = |batch_entries: &[(PathBuf, String)]| {
        run_photos_import_applescript_session("fast-img", batch_entries)
    };
    let mut pending_report = import_pending_media_entries_with_checkpoint(
        &mut checkpoint_marker,
        &pending_entries,
        photos_import_fail_fast_enabled(),
        &mut query_assets,
        &mut is_quarantined,
        &mut prepare_import_session,
        &mut run_import_batch,
    )?;
    imported_assets.append(&mut pending_report.imported_assets);
    imported_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    if imported_assets
        .len()
        .checked_add(pending_report.failed_count)
        != Some(expected_output_count)
    {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos AppleScript import established {} verified assets and {} controlled failures \
             for {} outputs (marker expected {}). The importer checkpoints each verified item \
             and resumes pending assets on rerun.",
            imported_assets.len(),
            pending_report.failed_count,
            output_paths.len(),
            expected_output_count
        )));
    }
    tracing::info!(
        target: "fast_img",
        imported = imported_assets.len(),
        transaction_size = FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE,
        window_file_cap = FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP,
        "Photos AppleScript import complete"
    );
    Ok(LibraryHandle {
        imported_assets,
        import_error_count: pending_report.failed_count,
    })
}

fn import_media_outputs_with_photos_applescript(
    candidates: &[PhotosImportCandidate],
) -> Result<PhotosMediaImportReport> {
    let mut run_import_batch = |manifest_entries: &[(PathBuf, String)]| {
        run_photos_import_applescript_session("media", manifest_entries)
    };
    import_media_outputs_with_photos_applescript_with(
        candidates,
        photos_import_fail_fast_enabled(),
        &mut run_import_batch,
    )
}

fn import_media_outputs_with_photos_applescript_with<R>(
    candidates: &[PhotosImportCandidate],
    fail_fast: bool,
    run_import_batch: &mut R,
) -> Result<PhotosMediaImportReport>
where
    R: FnMut(&[(PathBuf, String)]) -> Result<String>,
{
    if candidates.is_empty() {
        return Ok(PhotosMediaImportReport::default());
    }
    validate_photos_import_candidates(candidates)?;
    let mut report = PhotosMediaImportReport::default();
    // A short/empty Photos result cannot identify the rejected item; contain that
    // uncertainty to one Photos transaction instead of widening the custody blast radius.
    let media_batch_size =
        fast_img_photos_import_batch_size().min(FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE);
    let batch_count = candidates.len().div_ceil(media_batch_size);
    for (batch_index, candidate_batch) in candidates.chunks(media_batch_size).enumerate() {
        let batch_number = batch_index + 1;
        let manifest_entries = photos_import_candidate_manifest_entries(candidate_batch);
        let mut poisoned_attempts = 0usize;
        let result = loop {
            let result = run_import_batch(&manifest_entries).and_then(|stdout| {
                photos_import_pairs_from_candidates(candidate_batch, stdout.as_bytes())
            });
            match result {
                Err(err) if !fail_fast => {
                    let detail = err.to_string();
                    if let Some(poison_reason) = photos_import_retry_reason(&detail)
                        && poisoned_attempts < FAST_IMG_PHOTOS_IMPORT_POISON_RETRY_LIMIT
                    {
                        tracing::warn!(
                            target: "photos_import",
                            batch_number,
                            batch_count,
                            batch_files = candidate_batch.len(),
                            poisoned_attempts,
                            poison_reason,
                            detail = %detail,
                            "Photos media import hit a recoverable session failure; relaunching Photos and retrying current batch"
                        );
                        handle_photos_import_recovery(
                            "poisoned_session",
                            &mut relaunch_photos_for_import_recovery,
                            &mut probe_photos_import_session_health,
                        )?;
                        poisoned_attempts = poisoned_attempts.checked_add(1).ok_or_else(|| {
                            ImgQualityError::AnalysisError(
                                "Photos media import poison retry counter overflowed".to_string(),
                            )
                        })?;
                        continue;
                    }
                    break Err(err);
                }
                result => break result,
            }
        };
        match result {
            Ok(mut report_pairs) => report.report_pairs.append(&mut report_pairs),
            Err(err) if fail_fast => return Err(err),
            Err(err) if photos_import_controllable_item_failure(&err.to_string()) => {
                report.failed_count = report
                    .failed_count
                    .checked_add(candidate_batch.len())
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "Photos media import failure count overflowed".to_string(),
                        )
                    })?;
                tracing::error!(
                    target: "photos_import",
                    batch_number,
                    batch_count,
                    batch_files = candidate_batch.len(),
                    detail = %err,
                    "Photos returned no imported items for one media batch; preserving its sources and continuing normal-mode import"
                );
            }
            Err(err) => return Err(err),
        }
    }
    tracing::info!(
        target: "photos_import",
        imported = report.report_pairs.len(),
        failed = report.failed_count,
        batch_size = media_batch_size,
        "Photos AppleScript media import complete"
    );
    Ok(report)
}

fn photos_import_checkpoint_plan<P>(
    marker: &WorkingCopyMarker,
    mut is_quarantined: P,
) -> Result<PhotosImportCheckpointPlan>
where
    P: FnMut(&Path) -> Result<bool>,
{
    validate_fast_img_marker_path_contract(marker)?;
    let mut pending_entries = Vec::new();
    let mut proven_assets = Vec::new();
    let persisted_assets = marker
        .photos_imported_assets
        .iter()
        .map(|asset| (asset.rel_path.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    for (source_rel, entry) in &marker.blake3_log {
        let rel_path = marker_entry_out_rel(source_rel, entry);
        let path = marker.working_copy.join(&rel_path);
        let Some(library_asset) = entry.library_asset.as_ref() else {
            pending_entries.push(PhotosImportPendingEntry {
                source_rel: source_rel.clone(),
                rel_path: rel_path.clone(),
                path: path.clone(),
                album_name: fast_img_optimized_import_album_name(marker, &rel_path),
                blake3_entry: entry.clone(),
            });
            continue;
        };
        if library_asset != &entry.out {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import checkpoint hash drift for {source_rel}: output={} \
                 library={library_asset}",
                entry.out
            )));
        }
        let Some(persisted) = persisted_assets.get(rel_path.as_str()) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import checkpoint UUID proof missing for {source_rel}; refusing hash-only resume"
            )));
        };
        if persisted.blake3 != *library_asset
            || persisted.photos_uuid.as_deref().is_none_or(str::is_empty)
        {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import checkpoint UUID/hash proof invalid for {source_rel}"
            )));
        }
        let mut persisted = (*persisted).clone();
        persisted.quarantined = is_quarantined(&path)?;
        proven_assets.push(persisted);
    }
    pending_entries.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    proven_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(PhotosImportCheckpointPlan {
        pending_entries,
        proven_assets,
    })
}

fn photos_import_candidate_from_pending(entry: &PhotosImportPendingEntry) -> PhotosImportCandidate {
    PhotosImportCandidate {
        path: entry.path.clone(),
        album_name: entry.album_name.clone(),
        rel_path: entry.rel_path.clone(),
        blake3: entry.blake3_entry.out.clone(),
    }
}

fn reverify_checkpointed_photos_assets<Q, P>(
    marker: &WorkingCopyMarker,
    persisted_assets: &[LibraryAssetRecord],
    query_assets: &mut Q,
    is_quarantined: &mut P,
) -> Result<()>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    if persisted_assets.is_empty() {
        return Ok(());
    }
    let persisted_paths = persisted_assets
        .iter()
        .map(|asset| asset.rel_path.as_str())
        .collect::<BTreeSet<_>>();
    let candidates = build_fast_img_output_import_candidates(marker)?
        .into_iter()
        .filter(|candidate| persisted_paths.contains(candidate.rel_path.as_str()))
        .collect::<Vec<_>>();
    if candidates.len() != persisted_assets.len() {
        return Err(ImgQualityError::AnalysisError(
            "Photos resume checkpoint candidate count mismatch".to_string(),
        ));
    }
    let report_pairs = photos_import_report_pairs_from_persisted_assets(persisted_assets)?;
    let verified = library_handle_from_media_output_probes(
        &candidates,
        &report_pairs,
        query_assets,
        is_quarantined,
    )?;
    if verified.imported_assets.len() != persisted_assets.len() {
        return Err(ImgQualityError::AnalysisError(
            "Photos resume checkpoint live verification count mismatch".to_string(),
        ));
    }
    tracing::info!(
        target: "photos_import",
        verified = verified.imported_assets.len(),
        "Reverified persisted Photos UUID checkpoints before resuming import"
    );
    Ok(())
}

fn photos_reconciled_import_pairs(
    pending_entries: &[PhotosImportPendingEntry],
    stdout: &str,
) -> Result<Vec<(String, String)>> {
    let lines = stdout.lines().map(str::trim).collect::<Vec<_>>();
    if lines.len() != pending_entries.len() {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos resume reconciliation returned {} row(s) for {} pending item(s)",
            lines.len(),
            pending_entries.len()
        )));
    }
    Ok(pending_entries
        .iter()
        .zip(lines)
        .filter(|(_, identifier)| !identifier.is_empty() && *identifier != "MFB_NOT_FOUND")
        .map(|(entry, identifier)| (entry.rel_path.clone(), identifier.to_string()))
        .collect())
}

fn reconcile_uncheckpointed_photos_assets<Q, P, R>(
    marker: &mut WorkingCopyMarker,
    pending_entries: &[PhotosImportPendingEntry],
    query_assets: &mut Q,
    is_quarantined: &mut P,
    recover_import_ids: &mut R,
) -> Result<usize>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
    R: FnMut(&[(PathBuf, String)]) -> Result<String>,
{
    if pending_entries.is_empty() {
        return Ok(0);
    }
    let manifest_entries = pending_entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.album_name.clone()))
        .collect::<Vec<_>>();
    let stdout = recover_import_ids(&manifest_entries)?;
    let report_pairs = photos_reconciled_import_pairs(pending_entries, &stdout)?;
    if report_pairs.is_empty() {
        return Ok(0);
    }
    let recovered_paths = report_pairs
        .iter()
        .map(|(rel_path, _)| rel_path.as_str())
        .collect::<BTreeSet<_>>();
    let recovered_entries = pending_entries
        .iter()
        .filter(|entry| recovered_paths.contains(entry.rel_path.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let candidates = recovered_entries
        .iter()
        .map(photos_import_candidate_from_pending)
        .collect::<Vec<_>>();
    let verified = library_handle_from_media_output_probes(
        &candidates,
        &report_pairs,
        query_assets,
        is_quarantined,
    )?;
    checkpoint_photos_import_window(marker, &recovered_entries, &verified.imported_assets)?;
    tracing::info!(
        target: "photos_import",
        recovered = verified.imported_assets.len(),
        "Recovered Photos assets imported before the previous process stopped"
    );
    Ok(verified.imported_assets.len())
}

#[cfg(all(target_os = "macos", not(test)))]
fn reconcile_photos_batch_after_session_failure<Q, P>(
    marker: &mut WorkingCopyMarker,
    batch_entries: &[PhotosImportPendingEntry],
    query_assets: &mut Q,
    is_quarantined: &mut P,
) -> Result<Option<Vec<LibraryAssetRecord>>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    let mut reconcile_imports = |entries: &[(PathBuf, String)]| {
        run_photos_import_applescript_session_mode(
            "same-run media reconciliation",
            entries,
            "reconcile",
        )
    };
    let recovered = reconcile_uncheckpointed_photos_assets(
        marker,
        batch_entries,
        query_assets,
        is_quarantined,
        &mut reconcile_imports,
    )?;
    if recovered != batch_entries.len() {
        return Ok(None);
    }

    let batch_paths = batch_entries
        .iter()
        .map(|entry| entry.rel_path.as_str())
        .collect::<BTreeSet<_>>();
    let recovered_assets = marker
        .photos_imported_assets
        .iter()
        .filter(|asset| batch_paths.contains(asset.rel_path.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if recovered_assets.len() != batch_entries.len() {
        return Ok(None);
    }
    tracing::info!(
        target: "photos_import",
        recovered,
        "Photos error occurred after commit; recovered verified assets instead of importing duplicates"
    );
    Ok(Some(recovered_assets))
}

// `marker` is only mutated by the macOS production recovery branch; the
// test/non-macOS cfg leaves it read-only, which trips needless_pass_by_ref_mut.
#[cfg_attr(
    any(not(target_os = "macos"), test),
    allow(clippy::needless_pass_by_ref_mut)
)]
fn import_photos_batch_with_recovery<Q, P, R>(
    marker: &mut WorkingCopyMarker,
    batch_entries: &[PhotosImportPendingEntry],
    fail_fast: bool,
    window_start: usize,
    batch_number: usize,
    batch_count: usize,
    query_assets: &mut Q,
    is_quarantined: &mut P,
    run_import_batch: &mut R,
) -> Result<PhotosImportBatchOutcome>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
    R: FnMut(&[(PathBuf, String)]) -> Result<String>,
{
    let manifest_entries = batch_entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.album_name.clone()))
        .collect::<Vec<_>>();
    let output_paths = batch_entries
        .iter()
        .map(|entry| (entry.rel_path.clone(), entry.path.clone()))
        .collect::<Vec<_>>();
    let mut poisoned_attempts = 0usize;
    loop {
        let attempt_result = (|| {
            let stdout = run_import_batch(&manifest_entries)?;
            let report_pairs = fast_img_pairs_from_photos_import_ids(
                &output_paths,
                stdout.as_bytes(),
                batch_entries.len(),
            )?;
            library_records_from_pending_import(
                batch_entries,
                &report_pairs,
                query_assets,
                is_quarantined,
            )
        })();
        match attempt_result {
            Ok(batch_assets) => return Ok(PhotosImportBatchOutcome::Imported(batch_assets)),
            Err(err) => {
                let detail = err.to_string();
                if fail_fast {
                    return Err(err);
                }
                if let Some(poison_reason) = photos_import_retry_reason(&detail)
                    && poisoned_attempts < FAST_IMG_PHOTOS_IMPORT_POISON_RETRY_LIMIT
                {
                    tracing::warn!(
                        target: "photos_import",
                        window_start,
                        batch_number,
                        batch_count,
                        poisoned_attempts,
                        poison_reason,
                        detail = %detail,
                        "Photos import batch hit a recoverable session failure; relaunching Photos and retrying"
                    );
                    handle_photos_import_recovery(
                        "poisoned_session",
                        &mut relaunch_photos_for_import_recovery,
                        &mut probe_photos_import_session_health,
                    )?;
                    poisoned_attempts = poisoned_attempts.checked_add(1).ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "Photos import poison retry counter overflowed".to_string(),
                        )
                    })?;
                    tracing::info!(
                        target: "photos_import",
                        window_start,
                        batch_number,
                        batch_count,
                        poisoned_attempts,
                        poison_reason,
                        "Photos import recovery complete; automatically retrying current batch"
                    );
                    #[cfg(all(target_os = "macos", not(test)))]
                    if let Some(recovered_assets) = reconcile_photos_batch_after_session_failure(
                        marker,
                        batch_entries,
                        query_assets,
                        is_quarantined,
                    )? {
                        return Ok(PhotosImportBatchOutcome::Imported(recovered_assets));
                    }
                    // Non-macOS/test builds never take the marker-based
                    // recovery branch above; the parameter exists for the
                    // contract shared with those builds.
                    #[cfg(any(not(target_os = "macos"), test))]
                    let _ = marker;
                    continue;
                }
                if batch_entries.len() == 1 && photos_import_controllable_item_failure(&detail) {
                    let source_rel = batch_entries[0].source_rel.clone();
                    tracing::error!(
                        target: "photos_import",
                        source_rel = %source_rel,
                        detail = %detail,
                        "Photos rejected one file; continuing normal-mode import and deferring final failure"
                    );
                    return Ok(PhotosImportBatchOutcome::DeferredItem { source_rel, detail });
                }
                return Err(err);
            }
        }
    }
}

fn import_pending_media_entries_with_checkpoint<Q, P, R>(
    marker: &mut WorkingCopyMarker,
    pending_entries: &[PhotosImportPendingEntry],
    fail_fast: bool,
    query_assets: &mut Q,
    is_quarantined: &mut P,
    prepare_import_session: &mut impl FnMut(&str) -> Result<()>,
    run_import_batch: &mut R,
) -> Result<PhotosCheckpointImportReport>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
    R: FnMut(&[(PathBuf, String)]) -> Result<String>,
{
    if pending_entries.is_empty() {
        return Ok(PhotosCheckpointImportReport::default());
    }
    let strategy = photos_import_strategy(pending_entries.len());
    tracing::info!(
        target: "photos_import",
        pending_files = pending_entries.len(),
        strategy = strategy.as_str(),
        fast_path_file_cap = FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP,
        transaction_size = FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE,
        "Selected Photos import strategy"
    );
    if photos_import_strategy_requires_initial_warmup(strategy) {
        prepare_import_session("initial_import_warmup")?;
    }
    let windows = photos_import_windows(
        pending_entries.len(),
        photos_import_strategy_window_file_cap(strategy),
        FAST_IMG_PHOTOS_IMPORT_RELAUNCH_INTERVAL_FILES,
    )?;
    let mut imported_assets = Vec::new();
    let mut deferred_item_failures = Vec::new();
    for window in windows {
        let end = window.start.checked_add(window.len).ok_or_else(|| {
            ImgQualityError::AnalysisError(
                "Photos import window range overflowed pending entry count".to_string(),
            )
        })?;
        let entries = pending_entries.get(window.start..end).ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "Photos import window out of bounds: start={} len={} pending={}",
                window.start,
                window.len,
                pending_entries.len()
            ))
        })?;
        if window.relaunch_photos_before {
            handle_photos_import_recovery(
                "periodic_window_boundary",
                &mut relaunch_photos_for_import_recovery,
                &mut probe_photos_import_session_health,
            )?;
        }
        let batch_sizes = photos_import_batch_sizes(entries.len());
        let batch_count = batch_sizes.len();
        let mut offset = 0usize;
        for (batch_index, batch_size) in batch_sizes.into_iter().enumerate() {
            let batch_number = batch_index + 1;
            let end = offset.checked_add(batch_size).ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "Photos import batch range overflowed pending entry count".to_string(),
                )
            })?;
            let batch_entries = entries.get(offset..end).ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "Photos import batch out of bounds: start={} len={} pending={}",
                    offset,
                    batch_size,
                    entries.len()
                ))
            })?;
            tracing::info!(
                target: "photos_import",
                window_start = window.start,
                window_len = window.len,
                batch_number,
                batch_count,
                batch_files = batch_entries.len(),
                "Starting Photos import batch"
            );
            let mut batch_assets = match import_photos_batch_with_recovery(
                marker,
                batch_entries,
                fail_fast,
                window.start,
                batch_number,
                batch_count,
                query_assets,
                is_quarantined,
                run_import_batch,
            )? {
                PhotosImportBatchOutcome::Imported(batch_assets) => batch_assets,
                PhotosImportBatchOutcome::DeferredItem { source_rel, detail } => {
                    deferred_item_failures.push((source_rel, detail));
                    offset = end;
                    continue;
                }
            };
            checkpoint_photos_import_window(marker, batch_entries, &batch_assets)?;
            imported_assets.append(&mut batch_assets);
            offset = end;
            let completed_transactions = batch_index + 1;
            if !cfg!(test)
                && completed_transactions < batch_count
                && completed_transactions % FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE == 0
            {
                std::thread::sleep(Duration::from_millis(FAST_IMG_PHOTOS_IMPORT_BATCH_DELAY_MS));
            }
            if !cfg!(test)
                && FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL > 0
                && completed_transactions
                    % (FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL
                        * FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE)
                    == 0
            {
                std::thread::sleep(Duration::from_secs(
                    FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_SECS,
                ));
            }
        }
        if !cfg!(test) && end < pending_entries.len() {
            std::thread::sleep(Duration::from_secs(
                FAST_IMG_PHOTOS_IMPORT_WINDOW_PAUSE_SECS,
            ));
        }
    }
    if !deferred_item_failures.is_empty() {
        let failed_files = deferred_item_failures
            .iter()
            .map(|(source_rel, _)| source_rel.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        tracing::error!(
            target: "photos_import",
            failed = deferred_item_failures.len(),
            pending = %failed_files,
            "Photos import exhausted bounded retries for controllable files; verified successes remain checkpointed"
        );
    }
    Ok(PhotosCheckpointImportReport {
        imported_assets,
        failed_count: deferred_item_failures.len(),
    })
}

fn library_records_from_pending_import<Q, P>(
    entries: &[PhotosImportPendingEntry],
    report_pairs: &[(String, String)],
    query_assets: &mut Q,
    is_quarantined: &mut P,
) -> Result<Vec<LibraryAssetRecord>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    let report_index = report_pairs
        .iter()
        .cloned()
        .collect::<BTreeMap<String, String>>();
    let import_targets = fast_img_import_probe_targets(report_pairs)?;
    let probes = query_uploaded_asset_probes_batch_with_retry(
        &import_targets,
        fast_img_icloud_upload_verify_attempts(),
        fast_img_icloud_upload_verify_batch_size(),
        fast_img_icloud_upload_verify_delay(),
        fast_img_require_icloud_upload_proof(),
        query_assets,
        std::thread::sleep,
    )?;
    let mut verified_probes = verified_library_probes_from_query(probes)?;
    let mut records = Vec::new();
    for entry in entries {
        let Some(import_identifier) = report_index.get(&entry.rel_path) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier missing import identifier for {}",
                entry.rel_path
            )));
        };
        let expected_uuid = osxphotos_uuid_from_photos_import_identifier(import_identifier)?;
        let verified_probe = remove_matching_library_probe_by_uuid(
            &mut verified_probes,
            &entry.rel_path,
            expected_uuid,
            &entry.blake3_entry.out,
            &entry.path,
            "Import aborted before checkpoint because Photos library bytes do not match the \
             working copy.",
        )?;
        records.push(LibraryAssetRecord {
            rel_path: entry.rel_path.clone(),
            blake3: entry.blake3_entry.out.clone(),
            sync_status: photos_sync_status(&verified_probe.probe).to_string(),
            quarantined: is_quarantined(&entry.path)?,
            photos_uuid: Some(verified_probe.probe.uuid.clone()),
            library_blake3: None,
        });
    }
    records.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(records)
}

fn verified_library_probes_from_query(
    probes: BTreeMap<String, FastImgLibraryAssetProbe>,
) -> Result<Vec<VerifiedLibraryProbe>> {
    probes
        .into_iter()
        .map(|(report_rel_path, probe)| {
            if !probe.path.exists() {
                return Err(ImgQualityError::AnalysisError(format!(
                    "Photos verifier asset path missing for {}: {}",
                    report_rel_path,
                    probe.path.display()
                )));
            }
            let blake3 = crate::common_utils::calculate_blake3_hash(&probe.path).map_err(|e| {
                ImgQualityError::AnalysisError(format!(
                    "Photos verifier BLAKE3 failed for {}: {e}",
                    probe.path.display()
                ))
            })?;
            Ok(VerifiedLibraryProbe { probe, blake3 })
        })
        .collect()
}

fn remove_matching_library_probe_by_uuid(
    verified_probes: &mut Vec<VerifiedLibraryProbe>,
    rel_path: &str,
    expected_uuid: &str,
    expected_hash: &str,
    output_path: &Path,
    error_suffix: &str,
) -> Result<VerifiedLibraryProbe> {
    let Some(index) = verified_probes
        .iter()
        .position(|verified| verified.probe.uuid == expected_uuid)
    else {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos verifier missing library probe for {rel_path}: uuid={expected_uuid} \
             output={expected_hash} output_path={}",
            output_path.display()
        )));
    };
    let candidate = verified_probes.remove(index);
    if candidate.blake3 == expected_hash {
        return Ok(candidate);
    }

    tracing::error!(
        target: "photos_import",
        rel_path = %rel_path,
        photos_uuid = %expected_uuid,
        output = %expected_hash,
        library = %candidate.blake3,
        output_path = %output_path.display(),
        library_path = %candidate.probe.path.display(),
        "Photos imported bytes diverged from working copy"
    );
    Err(ImgQualityError::AnalysisError(format!(
        "Photos verifier BLAKE3 mismatch for {rel_path}: output={expected_hash} library={} \
         output_path={} library_path={}. {error_suffix}",
        candidate.blake3,
        output_path.display(),
        candidate.probe.path.display()
    )))
}

fn checkpoint_photos_import_window(
    marker: &mut WorkingCopyMarker,
    entries: &[PhotosImportPendingEntry],
    assets: &[LibraryAssetRecord],
) -> Result<()> {
    let asset_index = assets
        .iter()
        .map(|asset| (asset.rel_path.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    for entry in entries {
        let Some(asset) = asset_index.get(entry.rel_path.as_str()) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import checkpoint missing verified asset for {}",
                entry.rel_path
            )));
        };
        let Some(marker_entry) = marker.blake3_log.get_mut(&entry.source_rel) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import checkpoint missing marker entry for {}",
                entry.source_rel
            )));
        };
        if marker_entry.out != asset.blake3 {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import checkpoint BLAKE3 mismatch for {}: marker={} asset={}",
                entry.source_rel, marker_entry.out, asset.blake3
            )));
        }
        marker_entry.library_asset = Some(asset.blake3.clone());
    }
    for asset in assets {
        marker
            .photos_imported_assets
            .retain(|persisted| persisted.rel_path != asset.rel_path);
        marker.photos_imported_assets.push(asset.clone());
    }
    marker
        .photos_imported_assets
        .sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    write_marker_atomic(marker).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "write Photos import checkpoint marker failed: {err}"
        ))
    })?;
    tracing::info!(
        target: "photos_import",
        checkpointed = assets.len(),
        "Photos import checkpoint marker updated"
    );
    Ok(())
}

fn photos_import_windows(
    total_files: usize,
    window_file_cap: usize,
    relaunch_interval_files: usize,
) -> Result<Vec<PhotosImportWindow>> {
    if total_files == 0 {
        return Ok(Vec::new());
    }
    if window_file_cap == 0 {
        return Err(ImgQualityError::AnalysisError(
            "Photos import window size must be at least 1".to_string(),
        ));
    }

    // Check if periodic relaunch is disabled via env
    let disable_relaunch = match std::env::var(FAST_IMG_DISABLE_PERIODIC_RELAUNCH_ENV) {
        Ok(raw) => {
            let disable = match raw.trim().parse::<u8>() {
                Ok(1) => true,
                Ok(_) => false,
                Err(_) => {
                    tracing::warn!(
                        target: "photos_import",
                        "Invalid {FAST_IMG_DISABLE_PERIODIC_RELAUNCH_ENV}={raw:?}; treating as disable_relaunch=false"
                    );
                    false
                }
            };
            if disable {
                tracing::warn!(
                    target: "photos_import",
                    "Periodic Photos relaunch disabled via {}; session may accumulate state",
                    FAST_IMG_DISABLE_PERIODIC_RELAUNCH_ENV
                );
            }
            disable
        }
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            tracing::warn!(
                target: "photos_import",
                "Failed to read {} environment variable: {e}; defaulting to false",
                FAST_IMG_DISABLE_PERIODIC_RELAUNCH_ENV
            );
            false
        }
    };

    let mut windows = Vec::new();
    let mut start = 0usize;
    let mut files_since_relaunch = 0usize;
    while start < total_files {
        let remaining = total_files - start;
        let len = remaining.min(window_file_cap);
        let relaunch_photos_before = !disable_relaunch
            && start > 0
            && relaunch_interval_files > 0
            && files_since_relaunch.checked_add(len).ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "Photos import relaunch interval counter overflowed".to_string(),
                )
            })? > relaunch_interval_files;
        windows.push(PhotosImportWindow {
            start,
            len,
            relaunch_photos_before,
        });
        files_since_relaunch = if relaunch_photos_before {
            len
        } else {
            files_since_relaunch.checked_add(len).ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "Photos import relaunch interval counter overflowed".to_string(),
                )
            })?
        };
        start = start.checked_add(len).ok_or_else(|| {
            ImgQualityError::AnalysisError("Photos import window cursor overflowed".to_string())
        })?;
    }
    Ok(windows)
}

/// Run one short Photos import session. Large libraries are split in Rust so
/// verified assets can be checkpointed before Photos/iCloud session poison.
fn run_photos_import_applescript_session(
    media_kind: &str,
    manifest_entries: &[(PathBuf, String)],
) -> Result<String> {
    run_photos_import_applescript_session_mode(media_kind, manifest_entries, "import")
}

fn run_photos_import_applescript_session_mode(
    media_kind: &str,
    manifest_entries: &[(PathBuf, String)],
    operation: &str,
) -> Result<String> {
    use std::io::Write as _;

    if manifest_entries.is_empty() {
        return Ok(String::new());
    }

    let chunks: Vec<&[(PathBuf, String)]> = manifest_entries
        .chunks(FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP)
        .collect();
    let mut all_results = String::new();

    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        let chunk_number = chunk_idx + 1;
        let batch_count = chunk
            .len()
            .div_ceil(FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE)
            .max(1);

        tracing::info!(
            target: "photos_import",
            media_kind,
            chunk_number,
            total_chunks = chunks.len(),
            chunk_files = chunk.len(),
            chunk_batches = batch_count,
            "Starting Photos import chunk"
        );

        // Log Photos memory state before chunk
        log_photos_resource_state(chunk_number, "before");

        let manifest = photos_import_manifest_text(chunk)?;
        let mut manifest_file =
            crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "photos import manifest",
                Some("mfb-photos-import-manifest"),
                Some(".txt"),
            )
            .map_err(|e| {
                ImgQualityError::AnalysisError(format!(
                    "Photos import manifest tempfile failed: {e}"
                ))
            })?;
        manifest_file
            .write_all(manifest.as_bytes())
            .and_then(|()| manifest_file.flush())
            .map_err(|e| {
                ImgQualityError::AnalysisError(format!("Photos import manifest write failed: {e}"))
            })?;

        let timeout = photos_import_session_timeout(batch_count)?;
        let osascript = resolve_osascript_command();
        let mut command = std::process::Command::new(&osascript);
        command
            .arg("-e")
            .arg(FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT)
            .arg(manifest_file.path())
            .arg(FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE.to_string())
            .arg(FAST_IMG_PHOTOS_IMPORT_BATCH_DELAY_MS.to_string())
            .arg(FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL.to_string())
            .arg(FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_SECS.to_string())
            .arg(timeout.as_secs().to_string())
            .arg(operation);

        let output = crate::process_runner::ManagedProcess::spawn(&mut command)
            .and_then(|process| {
                process.wait_liveness_timeout(timeout, timeout, "Photos AppleScript import chunk")
            })
            .map_err(|e| {
                ImgQualityError::AnalysisError(format!(
                    "Photos AppleScript {media_kind} import chunk {chunk_number}/{} failed via \
                     {}: {e}",
                    chunks.len(),
                    osascript.display()
                ))
            })?;

        if !output.status.success() {
            return Err(photos_applescript_import_chunk_error(
                media_kind,
                chunk_number,
                chunks.len(),
                batch_count,
                &output.stderr,
            ));
        }

        // Log Photos memory state after chunk
        log_photos_resource_state(chunk_number, "after");

        let chunk_stdout = output.stdout.trim();
        if !chunk_stdout.is_empty() {
            if !all_results.is_empty() {
                all_results.push('\n');
            }
            all_results.push_str(chunk_stdout);
        }

        tracing::info!(
            target: "photos_import",
            media_kind,
            chunk_number,
            total_chunks = chunks.len(),
            "Photos import chunk complete"
        );

        // Inter-chunk breather: let Photos flush buffers before next chunk
        if chunk_idx + 1 < chunks.len() {
            tracing::info!(
                target: "photos_import",
                "Pausing between Photos import windows for buffer flush"
            );
            std::thread::sleep(Duration::from_secs(
                FAST_IMG_PHOTOS_IMPORT_WINDOW_PAUSE_SECS,
            ));
        }
    }

    tracing::info!(
        target: "photos_import",
        media_kind,
        total_files = manifest_entries.len(),
        total_chunks = chunks.len(),
        "Photos AppleScript import session complete"
    );
    Ok(all_results)
}

fn photos_import_session_timeout(batch_count: usize) -> Result<Duration> {
    let batch_count_u32 =
        crate::numeric_cast::usize_to_u32_strict(batch_count, "photos_import_batch_count")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "Photos import batch count exceeds timeout multiplier range: {batch_count}"
                ))
            })?;
    let digest_pause_count = batch_count
        .checked_div(FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL.max(1))
        .ok_or_else(|| {
            ImgQualityError::AnalysisError("Photos import digest pause divisor failed".to_string())
        })?;
    let digest_pause_count_u64 = crate::numeric_cast::usize_to_u64_strict(
        digest_pause_count,
        "photos_import_digest_pause_count",
    )
    .ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "Photos import digest pause count exceeds u64: {digest_pause_count}"
        ))
    })?;
    Ok(fast_img_photos_import_timeout()
        .saturating_mul(batch_count_u32)
        .saturating_add(Duration::from_secs(
            FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_SECS.saturating_mul(digest_pause_count_u64),
        )))
}

fn log_photos_resource_state(chunk_number: usize, phase: &str) {
    match get_photos_pid() {
        Ok(Some(pid)) => {
            let mut command = std::process::Command::new(MACOS_PS_PATH);
            command.args(["-p", &pid, "-o", "rss=,vsz="]);
            let output = run_fast_img_command_with_timeout(
                &mut command,
                FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
                "Photos process memory probe",
            );

            match output {
                Ok(output) if output.status.success() => {
                    let mem_info = String::from_utf8_lossy(&output.stdout);
                    tracing::info!(
                        target: "photos_import",
                        chunk_number,
                        phase,
                        photos_pid = %pid,
                        photos_memory = %mem_info.trim(),
                        "Photos process memory state"
                    );
                }
                Ok(output) => {
                    tracing::warn!(
                        target: "photos_import",
                        chunk_number,
                        phase,
                        photos_pid = %pid,
                        status = ?output.status.code(),
                        stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                        "Photos process memory probe failed"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        target: "photos_import",
                        chunk_number,
                        phase,
                        photos_pid = %pid,
                        "Photos process memory probe command failed: {err}"
                    );
                }
            }
        }
        Ok(None) => {
            tracing::info!(
                target: "photos_import",
                chunk_number,
                phase,
                "Photos process not running during resource probe"
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "photos_import",
                chunk_number,
                phase,
                "Photos process lookup failed during resource probe: {err}"
            );
        }
    }

    let mut command = std::process::Command::new(MACOS_VM_STAT_PATH);
    match run_fast_img_command_with_timeout(
        &mut command,
        FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
        "Photos system memory probe",
    ) {
        Ok(output) if output.status.success() => {
            let vm_stat = String::from_utf8_lossy(&output.stdout);
            if let Some(free_line) = vm_stat.lines().find(|l| l.contains("Pages free:")) {
                tracing::info!(
                    target: "photos_import",
                    chunk_number,
                    phase,
                    vm_stat_free = %free_line.trim(),
                    "System memory state"
                );
            }
        }
        Ok(output) => {
            tracing::warn!(
                target: "photos_import",
                chunk_number,
                phase,
                status = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "vm_stat probe failed"
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "photos_import",
                chunk_number,
                phase,
                "vm_stat probe command failed: {err}"
            );
        }
    }
}

fn get_photos_pid() -> Result<Option<String>> {
    let mut command = std::process::Command::new(MACOS_PGREP_PATH);
    command.args(["-x", "Photos"]);
    let output = run_fast_img_command_with_timeout(
        &mut command,
        FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
        "Photos process lookup",
    )
    .map_err(|err| {
        tracing::warn!(
            target: "photos_import",
            "Photos process lookup command failed: {err}"
        );
        ImgQualityError::AnalysisError(format!("Photos process lookup command failed: {err}"))
    })?;

    if output.status.success() {
        let stdout = String::from_utf8(output.stdout).map_err(|err| {
            tracing::warn!(
                target: "photos_import",
                "Photos process lookup returned non-UTF-8 stdout: {err}"
            );
            ImgQualityError::AnalysisError(format!(
                "Photos process lookup returned non-UTF-8 stdout: {err}"
            ))
        })?;
        Ok(stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(ToOwned::to_owned))
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            target: "photos_import",
            status = ?output.status.code(),
            stderr = %stderr.trim(),
            "Photos process lookup failed"
        );
        Err(ImgQualityError::AnalysisError(format!(
            "Photos process lookup failed: {}",
            stderr.trim()
        )))
    }
}

fn photos_import_retry_reason(detail: &str) -> Option<&'static str> {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("not authorized")
        || lower.contains("not permitted")
        || lower.contains("permission denied")
        || lower.contains("(-1743)")
        || lower.contains("unsupported format")
        || lower.contains("no such file")
        || lower.contains("file is corrupt")
        || lower.contains("blake3 mismatch")
        || lower.contains("pixel-equivalence")
        || lower.contains("uuid mismatch")
        || lower.contains("unexpected uuid")
        || lower.contains("duplicate uuid")
        || detail.contains("没有权限")
        || detail.contains("不允许")
        || detail.contains("文件已损坏")
    {
        None
    } else if photos_zero_import_context(detail).is_some()
        || lower.contains("photos returned 0 imported items")
        || lower.contains("photos applescript import returned 0 ids for ")
    {
        Some("zero_import_items")
    } else if (lower.contains("photos returned") && lower.contains(" imported items for "))
        || (lower.contains("photos applescript import returned") && lower.contains(" ids for "))
    {
        None
    } else if lower.contains("invalid connection")
        || lower.contains("connection is invalid")
        || lower.contains("(-609)")
        || detail.contains("连接无效")
    {
        Some("invalid_connection")
    } else if lower.contains("(-1712)")
        || lower.contains("timed out at hard timeout")
        || (lower.contains("photos applescript import chunk") && lower.contains("timed out after"))
        || detail.contains("超时")
        || detail.contains("AppleEvent已超时")
    {
        Some("appleevent_timeout")
    } else {
        Some("unknown_photos_error")
    }
}

fn photos_import_controllable_item_failure(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    photos_zero_import_context(detail).is_some()
        || lower.contains("photos returned 0 imported items")
        || lower.contains("photos applescript import returned 0 ids for ")
        || (lower.contains("photos verifier has") && lower.contains("without required proof after"))
}

fn handle_photos_import_recovery(
    reason: &str,
    relaunch: &mut impl FnMut(&str) -> Result<()>,
    probe_session_health: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    match relaunch(reason) {
        Ok(()) => Ok(()),
        Err(err) if reason == "periodic_window_boundary" => {
            tracing::warn!(
                target: "photos_import",
                reason,
                error = %err,
                "Periodic Photos recovery failed; proving the existing session is still responsive"
            );
            probe_session_health().map_err(|probe_err| {
                tracing::error!(
                    target: "photos_import",
                    reason,
                    recovery_error = %err,
                    probe_error = %probe_err,
                    "Periodic Photos recovery and functional session probe both failed"
                );
                ImgQualityError::AnalysisError(format!(
                    "Periodic Photos recovery failed ({err}); functional session probe also failed ({probe_err})"
                ))
            })?;
            tracing::warn!(
                target: "photos_import",
                reason,
                "Periodic recovery failed, but a bounded Photos AppleEvent probe proved the existing session responsive; continuing import"
            );
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn probe_photos_import_session_health() -> Result<()> {
    let osascript = resolve_osascript_command();
    let mut command = std::process::Command::new(&osascript);
    command.arg("-e").arg(
        r#"with timeout of 15 seconds
tell application "Photos" to get version
end timeout"#,
    );
    let output = crate::process_runner::ManagedProcess::spawn(&mut command)
        .and_then(|process| {
            process.wait_timeout(
                FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
                "Photos import recovery health probe",
            )
        })
        .map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "Photos recovery health probe failed via {}: {err}",
                osascript.display()
            ))
        })?;
    if !output.status.success() {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos recovery health probe exited unsuccessfully: {}",
            output.stderr.trim()
        )));
    }
    let version = output.stdout.trim();
    if version.is_empty() {
        return Err(ImgQualityError::AnalysisError(
            "Photos recovery health probe returned an empty version".to_string(),
        ));
    }
    tracing::info!(
        target: "photos_import",
        photos_version = version,
        "Photos recovery health probe succeeded"
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
const fn probe_photos_import_session_health() -> Result<()> {
    Ok(())
}

#[cfg(all(not(target_os = "macos"), not(test)))]
fn probe_photos_import_session_health() -> Result<()> {
    Err(ImgQualityError::AnalysisError(
        "Photos import recovery health probe is only supported on macOS".to_string(),
    ))
}

#[cfg(any(test, target_os = "macos"))]
fn complete_photos_quit_recovery(
    reason: &str,
    quit_result: Result<()>,
    wait_for_quit: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    if let Err(err) = quit_result {
        tracing::warn!(
            target: "photos_import",
            reason,
            error = %err,
            "Photos graceful quit failed; falling back to process-state recovery"
        );
    }
    wait_for_quit()
}

#[cfg(all(target_os = "macos", not(test)))]
fn relaunch_photos_for_import_recovery(reason: &str) -> Result<()> {
    tracing::warn!(
        target: "photos_import",
        reason,
        "Relaunching Photos for import recovery"
    );
    let osascript = resolve_osascript_command();
    let mut quit_command = std::process::Command::new(&osascript);
    quit_command
        .arg("-e")
        .arg("if application \"Photos\" is running then tell application \"Photos\" to quit");
    let quit_result = crate::process_runner::ManagedProcess::spawn(&mut quit_command)
        .and_then(|process| {
            process.wait_timeout(
                FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
                "Photos import recovery quit",
            )
        })
        .map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "Photos recovery quit command failed via {}: {err}",
                osascript.display()
            ))
        })
        .and_then(|quit_output| {
            if quit_output.status.success() {
                Ok(())
            } else {
                Err(ImgQualityError::AnalysisError(format!(
                    "Photos recovery quit command exited unsuccessfully: {}",
                    quit_output.stderr.trim()
                )))
            }
        });
    complete_photos_quit_recovery(reason, quit_result, &mut || {
        wait_for_photos_process_state(false, "quit")
    })?;

    let mut last_launch_error = None;
    for attempt in 1..=FAST_IMG_PHOTOS_IMPORT_RELAUNCH_OPEN_ATTEMPTS {
        let mut open_command = std::process::Command::new(MACOS_OPEN_PATH);
        open_command.args(["-a", "Photos"]);
        let open_output = crate::process_runner::ManagedProcess::spawn(&mut open_command)
            .and_then(|process| {
                process.wait_timeout(
                    FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
                    "Photos import recovery launch",
                )
            })
            .map_err(|err| {
                tracing::error!(
                    target: "photos_import",
                    reason,
                    attempt,
                    "Photos recovery launch command failed: {err}"
                );
                ImgQualityError::AnalysisError(format!(
                    "Photos recovery launch command failed: {err}"
                ))
            })?;
        if open_output.status.success() {
            last_launch_error = None;
            break;
        }
        let stderr = open_output.stderr.trim().to_string();
        tracing::warn!(
            target: "photos_import",
            reason,
            attempt,
            attempts = FAST_IMG_PHOTOS_IMPORT_RELAUNCH_OPEN_ATTEMPTS,
            status = ?open_output.status.code(),
            stderr = %stderr,
            "Photos recovery launch command exited unsuccessfully; retrying if attempts remain"
        );
        last_launch_error = Some(stderr);
        std::thread::sleep(Duration::from_secs(
            FAST_IMG_PHOTOS_IMPORT_RELAUNCH_OPEN_RETRY_SECS,
        ));
    }
    if let Some(stderr) = last_launch_error {
        tracing::error!(
            target: "photos_import",
            reason,
            stderr = %stderr,
            "Photos recovery launch command exhausted retries"
        );
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos recovery launch command exhausted retries: {stderr}"
        )));
    }

    wait_for_photos_process_state(true, "launch")?;
    std::thread::sleep(Duration::from_secs(
        FAST_IMG_PHOTOS_IMPORT_RELAUNCH_SETTLE_SECS,
    ));
    tracing::warn!(
        target: "photos_import",
        reason,
        "Photos relaunch complete for import recovery"
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
fn relaunch_photos_for_import_recovery(reason: &str) -> Result<()> {
    tracing::warn!(
        target: "photos_import",
        reason,
        "Photos relaunch stubbed during tests"
    );
    Ok(())
}

#[cfg(all(not(target_os = "macos"), not(test)))]
fn relaunch_photos_for_import_recovery(reason: &str) -> Result<()> {
    tracing::error!(
        target: "photos_import",
        reason,
        "Photos import recovery relaunch requested on non-macOS target"
    );
    Err(ImgQualityError::AnalysisError(
        "Photos import recovery relaunch is only supported on macOS".to_string(),
    ))
}

fn ensure_photos_import_session_ready<G, R>(
    reason: &str,
    mut get_photos_pid: G,
    mut relaunch: R,
) -> Result<()>
where
    G: FnMut() -> Result<Option<String>>,
    R: FnMut(&str) -> Result<()>,
{
    let pid = get_photos_pid()?;
    if let Some(pid) = pid {
        tracing::info!(
            target: "photos_import",
            reason,
            photos_pid = %pid,
            "Photos already running — relaunching import session for clean start"
        );
    } else {
        tracing::warn!(
            target: "photos_import",
            reason,
            "Photos not running — launching for import session warmup"
        );
    }
    relaunch(reason)?;
    Ok(())
}

fn prepare_photos_import_session(reason: &str) -> Result<()> {
    ensure_photos_import_session_ready(reason, get_photos_pid, relaunch_photos_for_import_recovery)
}

#[cfg(all(target_os = "macos", not(test)))]
fn attempt_photos_force_kill(signal: &str, phase: &str) {
    let mut command = std::process::Command::new(MACOS_KILLALL_PATH);
    command.args([signal, "Photos"]);
    match run_fast_img_command_with_timeout(
        &mut command,
        FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
        "Photos force-kill",
    ) {
        Ok(output) if output.status.success() => {}
        Ok(output) => tracing::warn!(
            target: "photos_import",
            phase,
            signal,
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "Photos force-kill command exited unsuccessfully"
        ),
        Err(err) => tracing::warn!(
            target: "photos_import",
            phase,
            signal,
            "Photos force-kill command failed: {err}"
        ),
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn wait_for_photos_process_state(expected_running: bool, phase: &str) -> Result<()> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(
            FAST_IMG_PHOTOS_IMPORT_RELAUNCH_PROCESS_TIMEOUT_SECS,
        ))
        .ok_or_else(|| {
            ImgQualityError::AnalysisError(
                "Photos recovery process deadline overflowed".to_string(),
            )
        })?;
    while Instant::now() < deadline {
        match get_photos_pid()? {
            Some(pid) if expected_running => {
                tracing::info!(
                    target: "photos_import",
                    phase,
                    photos_pid = %pid,
                    "Photos process reached expected recovery state"
                );
                return Ok(());
            }
            None if !expected_running => {
                tracing::info!(
                    target: "photos_import",
                    phase,
                    "Photos process reached expected recovery state"
                );
                return Ok(());
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }

    if !expected_running {
        tracing::warn!(
            target: "photos_import",
            phase,
            "Photos failed to quit gracefully after {}s, forcing termination via killall -9",
            FAST_IMG_PHOTOS_IMPORT_RELAUNCH_PROCESS_TIMEOUT_SECS
        );
        attempt_photos_force_kill("-9", phase);
        std::thread::sleep(Duration::from_secs(2)); // Wait for kill to take effect

        // Verify kill succeeded
        if get_photos_pid()?.is_none() {
            tracing::info!(
                target: "photos_import",
                phase,
                "Photos force-killed successfully"
            );
            return Ok(());
        }

        // Last resort: try killall again
        tracing::error!(
            target: "photos_import",
            phase,
            "First killall failed, retrying with -KILL signal"
        );
        attempt_photos_force_kill("-KILL", phase);
        std::thread::sleep(Duration::from_secs(2));

        if get_photos_pid()?.is_none() {
            return Ok(());
        }

        // Ultimate fallback: Photos is unkillable (SIP protection or kernel deadlock)
        tracing::error!(
            target: "photos_import",
            phase,
            "Photos process survived force-kill attempts; may be protected by SIP or kernel state"
        );
    }

    // For quit phase: if force-kill exhausted, treat as best-effort failure
    // (caller can decide to proceed with degraded session or abort)
    if !expected_running {
        tracing::error!(
            target: "photos_import",
            phase,
            "Photos quit force-kill failed after all attempts; caller should assess risk"
        );
        return Err(ImgQualityError::AnalysisError(
            "Photos process unkillable; system may require manual intervention".to_string(),
        ));
    }

    tracing::error!(
        target: "photos_import",
        phase,
        expected_running,
        "Timed out waiting for Photos process recovery state"
    );
    Err(ImgQualityError::AnalysisError(format!(
        "timed out waiting for Photos process {phase} state"
    )))
}

fn photos_import_manifest_text(entries: &[(PathBuf, String)]) -> Result<String> {
    let mut lines = Vec::with_capacity(entries.len() * 2);
    for (path, album_name) in entries {
        let path_str = path.to_str().ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "Photos import path is not valid UTF-8: {}",
                path.display()
            ))
        })?;
        if path_str.contains('\n')
            || path_str.contains('\r')
            || album_name.contains('\n')
            || album_name.contains('\r')
        {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import manifest entries must not contain line breaks: {}",
                path.display()
            )));
        }
        lines.push(path_str.to_owned());
        lines.push(album_name.clone());
    }
    Ok(lines.join("\n"))
}

fn photos_applescript_import_chunk_error(
    media_kind: &str,
    chunk_number: usize,
    total_chunks: usize,
    batch_count: usize,
    stderr: &str,
) -> ImgQualityError {
    let stderr = stderr.trim();
    let context = photos_zero_import_context(stderr);
    let advice =
        if stderr.contains("-1743") || stderr.contains("Not authorized to send Apple events") {
            " ❌ Import Failed: Missing AppleScript Automation Privilege! Please open macOS System \
             Settings -> Privacy & Security -> Automation, check 'Photos' under your terminal app \
             (or Modern Format Boost), and try again."
        } else if context.is_some() {
            " Photos returned success with an empty import result for this file; this usually \
             means the Photos/iCloud Photos library session is unhealthy or rejected the item \
             before creating an asset. The verifier fails closed and preserves sources until \
             destructive cleanup gates pass."
        } else {
            " The verifier fails closed and preserves sources until destructive cleanup gates pass."
        };
    let detail = if let Some(context) = context {
        format!("{stderr} ({context})")
    } else if stderr.is_empty() {
        "<empty stderr>".to_string()
    } else {
        stderr.to_string()
    };
    tracing::error!(
        target: "photos_import",
        media_kind,
        chunk_number,
        total_chunks,
        batch_count,
        detail = %detail,
        "Photos AppleScript import chunk failed"
    );
    ImgQualityError::AnalysisError(format!(
        "Photos AppleScript {media_kind} import chunk {chunk_number}/{total_chunks} \
         ({batch_count} batches) failed: {detail}.{advice}"
    ))
}

fn photos_zero_import_context(stderr: &str) -> Option<String> {
    let marker = "Photos returned 0 imported items for ";
    let start = stderr.find(marker)? + marker.len();
    let mut path = stderr[start..].trim();
    if let Some(stripped) = path.strip_suffix("(-2700)") {
        path = stripped.trim();
    }
    if let Some(stripped) = path.strip_prefix("batch starting ") {
        path = stripped
            .split_once(" (expected ")
            .map_or(stripped, |(batch_path, _)| batch_path)
            .trim();
    }
    path = path
        .split_once(" (batch ")
        .map_or(path, |(bare_path, _)| bare_path)
        .trim();
    if path.is_empty() {
        Some("zero imported items".to_string())
    } else {
        Some(format!("zero imported items: {path}"))
    }
}

fn validate_photos_import_output_paths(output_paths: &[(String, PathBuf)]) -> Result<()> {
    for (rel_path, output_path) in output_paths {
        let metadata = std::fs::metadata(output_path).map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "Photos import preflight missing JXL output {} ({}): {err}",
                rel_path,
                output_path.display()
            ))
        })?;
        if metadata.len() == 0 {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import preflight empty JXL output {} ({})",
                rel_path,
                output_path.display()
            )));
        }
    }
    Ok(())
}

fn validate_photos_import_candidates(candidates: &[PhotosImportCandidate]) -> Result<()> {
    let mut rel_paths = BTreeSet::new();
    for candidate in candidates {
        if !rel_paths.insert(candidate.rel_path.as_str()) {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import candidate duplicated relative path {}",
                candidate.rel_path
            )));
        }
        let metadata = std::fs::metadata(&candidate.path).map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "Photos import preflight missing media output {} ({}): {err}",
                candidate.rel_path,
                candidate.path.display()
            ))
        })?;
        if metadata.len() == 0 {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import preflight empty media output {} ({})",
                candidate.rel_path,
                candidate.path.display()
            )));
        }
        let current_hash =
            crate::common_utils::calculate_blake3_hash(&candidate.path).map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "Photos import preflight BLAKE3 failed for {} ({}): {err}",
                    candidate.rel_path,
                    candidate.path.display()
                ))
            })?;
        if current_hash != candidate.blake3 {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import preflight BLAKE3 mismatch for {} ({}): expected={} actual={current_hash}",
                candidate.rel_path,
                candidate.path.display(),
                candidate.blake3
            )));
        }
        #[cfg(target_os = "macos")]
        clear_quarantine_xattr(&candidate.path).map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "Photos import preflight could not clear quarantine from {} ({}): {err}",
                candidate.rel_path,
                candidate.path.display()
            ))
        })?;
        #[cfg(not(target_os = "macos"))]
        clear_quarantine_xattr(&candidate.path);
    }
    Ok(())
}

fn validate_fast_img_marker_output_hashes(marker: &WorkingCopyMarker) -> Result<()> {
    for (source_rel, entry) in &marker.blake3_log {
        let rel_path = marker_entry_out_rel(source_rel, entry);
        let output_path = marker.working_copy.join(&rel_path);
        let current_hash =
            crate::common_utils::calculate_blake3_hash(&output_path).map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "Photos import preflight BLAKE3 failed for {rel_path} ({}): {err}",
                    output_path.display()
                ))
            })?;
        if current_hash != entry.out {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import preflight BLAKE3 mismatch for {rel_path} ({}): expected={} actual={current_hash}",
                output_path.display(),
                entry.out
            )));
        }
    }
    Ok(())
}

fn prepare_photos_import_output_paths(output_paths: &[(String, PathBuf)]) -> Result<()> {
    validate_photos_import_output_paths(output_paths)?;
    for (rel_path, output_path) in output_paths {
        #[cfg(target_os = "macos")]
        clear_quarantine_xattr(output_path).map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "Photos import preflight could not clear quarantine from {} ({}): {err}",
                rel_path,
                output_path.display()
            ))
        })?;
        #[cfg(not(target_os = "macos"))]
        {
            let _ = rel_path;
            clear_quarantine_xattr(output_path);
        }
    }
    Ok(())
}

fn fast_img_pairs_from_photos_import_ids(
    output_paths: &[(String, PathBuf)],
    stdout: &[u8],
    full_expected_count: usize,
) -> Result<Vec<(String, String)>> {
    if output_paths.len() != 1 {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos UUID order is undefined; each import transaction must contain exactly one JXL output, got {}",
            output_paths.len()
        )));
    }
    let stdout = std::str::from_utf8(stdout).map_err(|e| {
        ImgQualityError::AnalysisError(format!("parse Photos AppleScript import IDs failed: {e}"))
    })?;
    let ids: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if ids.len() != output_paths.len() {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos AppleScript import returned {} IDs for {} JXL outputs (marker expected {}). \
             The osxphotos import CLI is not used because osxphotos import filters .JXL before \
             Photos sees it; if this persists, the current Photos/iCloud environment cannot \
             import JXL with a machine-readable UUID.",
            ids.len(),
            output_paths.len(),
            full_expected_count
        )));
    }
    Ok(output_paths
        .iter()
        .zip(ids)
        .map(|((rel_path, _path), uuid)| (rel_path.clone(), uuid))
        .collect())
}

fn photos_import_pairs_from_candidates(
    candidates: &[PhotosImportCandidate],
    stdout: &[u8],
) -> Result<Vec<(String, String)>> {
    let stdout = std::str::from_utf8(stdout).map_err(|e| {
        ImgQualityError::AnalysisError(format!("parse Photos AppleScript import IDs failed: {e}"))
    })?;
    let ids: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if ids.len() != candidates.len() {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos AppleScript import returned {} IDs for {} media outputs",
            ids.len(),
            candidates.len()
        )));
    }
    Ok(candidates
        .iter()
        .zip(ids)
        .map(|(candidate, uuid)| (candidate.rel_path.clone(), uuid))
        .collect())
}

fn photos_import_candidate_manifest_entries(
    candidates: &[PhotosImportCandidate],
) -> Vec<(PathBuf, String)> {
    candidates
        .iter()
        .map(|candidate| (candidate.path.clone(), candidate.album_name.clone()))
        .collect()
}

fn photos_import_batch_sizes(total: usize) -> Vec<usize> {
    vec![1; total]
}

const fn photos_import_strategy(total: usize) -> PhotosImportStrategy {
    if total <= FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP {
        PhotosImportStrategy::FastSmallSet
    } else {
        PhotosImportStrategy::StableCheckpointed
    }
}

const fn photos_import_strategy_window_file_cap(strategy: PhotosImportStrategy) -> usize {
    match strategy {
        PhotosImportStrategy::FastSmallSet => FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP,
        PhotosImportStrategy::StableCheckpointed => FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP,
    }
}

const fn photos_import_strategy_requires_initial_warmup(strategy: PhotosImportStrategy) -> bool {
    match strategy {
        PhotosImportStrategy::FastSmallSet => false,
        PhotosImportStrategy::StableCheckpointed => true,
    }
}

#[cfg(test)]
fn fast_img_photos_import_manifest_entries(
    marker: &WorkingCopyMarker,
    output_paths: &[(String, PathBuf)],
) -> Vec<(PathBuf, String)> {
    output_paths
        .iter()
        .map(|(rel_path, path)| {
            (
                path.clone(),
                fast_img_optimized_import_album_name(marker, rel_path),
            )
        })
        .collect()
}

fn osxphotos_uuid_from_photos_import_identifier(import_identifier: &str) -> Result<&str> {
    let Some(uuid) = import_identifier
        .split('/')
        .next()
        .map(str::trim)
        .filter(|uuid| !uuid.is_empty())
    else {
        return Err(ImgQualityError::AnalysisError(
            "Photos import returned an empty asset identifier".to_string(),
        ));
    };
    Ok(uuid)
}

fn verify_import_probe_uuid_binding(
    target: &FastImgImportProbeTarget,
    probe: &FastImgLibraryAssetProbe,
) -> Result<()> {
    if probe.uuid != target.osxphotos_uuid {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos verifier UUID mismatch for {}: import_id={} expected_uuid={} query_uuid={}",
            target.rel_path, target.import_identifier, target.osxphotos_uuid, probe.uuid
        )));
    }
    Ok(())
}

/// Index osxphotos query results by UUID and fail closed on missing/duplicate/extra rows.
fn index_photos_probes_by_uuid(
    expected_uuids: &[String],
    probes: Vec<FastImgLibraryAssetProbe>,
) -> Result<BTreeMap<String, FastImgLibraryAssetProbe>> {
    let expected = expected_uuids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut by_uuid = BTreeMap::new();
    for probe in probes {
        if !expected.contains(probe.uuid.as_str()) {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos query returned unexpected UUID {}",
                probe.uuid
            )));
        }
        if by_uuid.insert(probe.uuid.clone(), probe).is_some() {
            return Err(ImgQualityError::AnalysisError(
                "Photos query returned duplicate UUID".to_string(),
            ));
        }
    }
    if by_uuid.len() != expected.len() {
        let missing = expected_uuids
            .iter()
            .filter(|uuid| !by_uuid.contains_key(*uuid))
            .cloned()
            .collect::<Vec<_>>();
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos query returned {} of {} requested UUID(s); missing: {}",
            by_uuid.len(),
            expected.len(),
            missing.join(", ")
        )));
    }
    Ok(by_uuid)
}

fn fast_img_optimized_import_album_name(marker: &WorkingCopyMarker, rel_path: &str) -> String {
    let folder_name = marker
        .working_copy
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Imported");
    let cleaned = fast_img_strip_optimized_import_suffixes(folder_name);
    let inner_root = if cleaned.is_empty() {
        "✨Imported".to_string()
    } else if !cleaned.starts_with('✨') {
        format!("✨{cleaned}")
    } else {
        cleaned
    };

    let rel_parent = Path::new(rel_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(|parent| parent.to_str());

    if let Some(sub) = rel_parent {
        format!("✨/{inner_root}/{sub}")
    } else {
        format!("✨/{inner_root}")
    }
}

fn fast_img_strip_optimized_import_suffixes(folder_name: &str) -> String {
    let mut cleaned = folder_name;
    for suffix in [
        "_optimized_collected",
        "_collected_optimized",
        "_optimized",
        "_collected",
    ] {
        cleaned = cleaned.strip_suffix(suffix).unwrap_or(cleaned);
    }
    cleaned.to_string()
}

pub fn library_handle_from_media_output_probes(
    candidates: &[PhotosImportCandidate],
    report_pairs: &[(String, String)],
    query_assets: impl FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    is_quarantined: impl FnMut(&Path) -> Result<bool>,
) -> Result<LibraryHandle> {
    library_handle_from_media_output_probes_with_pixel_verifier(
        candidates,
        report_pairs,
        query_assets,
        is_quarantined,
        crate::image::orientation::verify_orientation_pixel_diff,
    )
}

fn library_handle_from_media_output_probes_with_pixel_verifier(
    candidates: &[PhotosImportCandidate],
    report_pairs: &[(String, String)],
    mut query_assets: impl FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    mut is_quarantined: impl FnMut(&Path) -> Result<bool>,
    mut verify_pixel_diff: impl FnMut(
        &Path,
        &Path,
        crate::image::format_detect::FormatKind,
        crate::image::orientation::DiffTolerance,
    ) -> Result<crate::image::orientation::PixelDiffResult>,
) -> Result<LibraryHandle> {
    if report_pairs.len() != candidates.len() {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos import report count mismatch: expected {} got {}",
            candidates.len(),
            report_pairs.len()
        )));
    }
    let mut candidate_index = BTreeMap::new();
    for candidate in candidates {
        if candidate_index
            .insert(candidate.rel_path.as_str(), candidate)
            .is_some()
        {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import candidate duplicated relative path {}",
                candidate.rel_path
            )));
        }
    }
    let import_targets = fast_img_import_probe_targets(report_pairs)?;
    let probes_by_rel = query_uploaded_asset_probes_batch_with_retry(
        &import_targets,
        fast_img_icloud_upload_verify_attempts(),
        fast_img_icloud_upload_verify_batch_size(),
        fast_img_icloud_upload_verify_delay(),
        fast_img_require_icloud_upload_proof(),
        &mut query_assets,
        std::thread::sleep,
    )?;

    let mut imported_assets = Vec::new();
    for target in import_targets {
        let Some(candidate) = candidate_index.get(target.rel_path.as_str()) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier asset has no import candidate: {}",
                target.rel_path
            )));
        };
        let Some(probe) = probes_by_rel.get(&target.rel_path) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier missing uploaded probe for {} (uuid={})",
                target.rel_path, target.osxphotos_uuid
            )));
        };
        verify_import_probe_uuid_binding(&target, probe)?;
        if !probe.path.exists() {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier asset path missing for {}: {}",
                target.rel_path,
                probe.path.display()
            )));
        }
        let library_blake3 =
            crate::common_utils::calculate_blake3_hash(&probe.path).map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "Photos verifier BLAKE3 failed for {}: {err}",
                    probe.path.display()
                ))
            })?;
        let mut final_library_blake3 = None;
        if library_blake3 != candidate.blake3 {
            let fmt = crate::image::format_detect::detect_true_format(&candidate.path)?;
            if let Some(tolerance) =
                crate::image::orientation::pixel_equivalence_diff_tolerance_for_format(fmt)
            {
                match verify_pixel_diff(&candidate.path, &probe.path, fmt, tolerance) {
                    Ok(crate::image::orientation::PixelDiffResult::Match) => {
                        tracing::info!(
                            target: "photos_import",
                            rel_path = %target.rel_path,
                            candidate_blake3 = %candidate.blake3,
                            library_blake3 = %library_blake3,
                            "Photos imported bytes diverged but pixel-equivalence proof passed; recording library_blake3"
                        );
                        final_library_blake3 = Some(library_blake3);
                    }
                    Ok(crate::image::orientation::PixelDiffResult::SkippedToolAbsent { tool }) => {
                        return Err(ImgQualityError::AnalysisError(format!(
                            "Photos verifier pixel-equivalence: proof unavailable for {} (uuid={}): missing {tool}",
                            target.rel_path, probe.uuid
                        )));
                    }
                    Ok(crate::image::orientation::PixelDiffResult::Mismatch {
                        max_delta,
                        channel,
                    }) => {
                        return Err(ImgQualityError::AnalysisError(format!(
                            "Photos verifier pixel-equivalence: mismatch for {} (uuid={}): max_delta={max_delta} channel={channel:?}",
                            target.rel_path, probe.uuid
                        )));
                    }
                    Err(err) => return Err(err),
                }
            } else {
                tracing::error!(
                    target: "photos_import",
                    rel_path = %target.rel_path,
                    candidate_blake3 = %candidate.blake3,
                    library_blake3 = %library_blake3,
                    photos_uuid = %probe.uuid,
                    candidate_path = %candidate.path.display(),
                    library_path = %probe.path.display(),
                    "Photos imported bytes diverged from tier-2 source"
                );
                return Err(ImgQualityError::AnalysisError(format!(
                    "Photos verifier BLAKE3 mismatch for {} (uuid={}): source={} library={library_blake3}. \
                     Import aborted because Photos library bytes do not match the source file and pixel equivalence check is not supported.",
                    target.rel_path, probe.uuid, candidate.blake3
                )));
            }
        }
        let quarantined = is_quarantined(&candidate.path)?;
        imported_assets.push(LibraryAssetRecord {
            rel_path: target.rel_path,
            blake3: candidate.blake3.clone(),
            sync_status: photos_sync_status(probe).to_string(),
            quarantined,
            photos_uuid: Some(probe.uuid.clone()),
            library_blake3: final_library_blake3,
        });
    }
    imported_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(LibraryHandle {
        imported_assets,
        import_error_count: 0,
    })
}

pub fn library_handle_from_probes(
    marker: &WorkingCopyMarker,
    report_pairs: &[(String, String)],
    mut query_asset: impl FnMut(&str) -> Result<FastImgLibraryAssetProbe>,
    mut is_quarantined: impl FnMut(&Path) -> Result<bool>,
) -> Result<LibraryHandle> {
    library_handle_from_batch_probes(
        marker,
        report_pairs,
        |uuids: &[String]| uuids.iter().map(|uuid| query_asset(uuid)).collect(),
        &mut is_quarantined,
    )
}

fn library_handle_from_batch_probes(
    marker: &WorkingCopyMarker,
    report_pairs: &[(String, String)],
    mut query_assets: impl FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    mut is_quarantined: impl FnMut(&Path) -> Result<bool>,
) -> Result<LibraryHandle> {
    validate_fast_img_marker_path_contract(marker)?;
    let expected_output_count = marker.expected_output_count();
    if report_pairs.len() != expected_output_count {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos import report count mismatch: expected {} got {}",
            expected_output_count,
            report_pairs.len()
        )));
    }
    let import_targets = fast_img_import_probe_targets(report_pairs)?;
    let mut probes = query_uploaded_asset_probes_batch_with_retry(
        &import_targets,
        fast_img_icloud_upload_verify_attempts(),
        fast_img_icloud_upload_verify_batch_size(),
        fast_img_icloud_upload_verify_delay(),
        fast_img_require_icloud_upload_proof(),
        &mut query_assets,
        std::thread::sleep,
    )?;

    let mut imported_assets = Vec::new();
    for target in import_targets {
        let Some(probe) = probes.remove(&target.rel_path) else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier missing uploaded probe for {}",
                target.rel_path
            )));
        };
        if !probe.path.exists() {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier asset path missing for {}: {}",
                target.rel_path,
                probe.path.display()
            )));
        }
        let blake3 = crate::common_utils::calculate_blake3_hash(&probe.path).map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "Photos verifier BLAKE3 failed for {}: {e}",
                probe.path.display()
            ))
        })?;
        let output_path = marker.working_copy.join(&target.rel_path);
        let quarantined = is_quarantined(&output_path)?;
        imported_assets.push(LibraryAssetRecord {
            rel_path: target.rel_path,
            blake3,
            sync_status: photos_sync_status(&probe).to_string(),
            quarantined,
            photos_uuid: Some(target.osxphotos_uuid.clone()),
            library_blake3: None,
        });
    }
    imported_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(LibraryHandle {
        imported_assets,
        import_error_count: 0,
    })
}

pub fn apply_library_assets_to_marker(
    marker: &mut WorkingCopyMarker,
    library: &LibraryHandle,
) -> Result<()> {
    for asset in &library.imported_assets {
        let Some((source_rel, entry)) = marker
            .blake3_log
            .iter_mut()
            .find(|(source_rel, entry)| marker_entry_out_rel(source_rel, entry) == asset.rel_path)
        else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier asset has no marker output: {}",
                asset.rel_path
            )));
        };
        if entry.out != asset.blake3 {
            tracing::error!(
                target: "photos_import",
                source_rel = %source_rel,
                marker = %entry.out,
                library = %asset.blake3,
                marker_path = %marker.working_copy.join(&asset.rel_path).display(),
                library_path = %marker.working_copy.join(&asset.rel_path).display(),
                "Photos import proof diverged from marker output hash"
            );
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos import proof BLAKE3 mismatch for {source_rel}: output={} library={}",
                entry.out, asset.blake3
            )));
        }
        entry.library_asset = Some(asset.blake3.clone());
    }
    marker
        .photos_imported_assets
        .clone_from(&library.imported_assets);
    Ok(())
}

fn query_osxphotos_asset_probes_in_libraries_with<Q>(
    uuids: &[String],
    libraries: &[PathBuf],
    mut query_library: Q,
) -> Result<(Vec<FastImgLibraryAssetProbe>, Option<PathBuf>)>
where
    Q: FnMut(&Path, &[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
{
    if libraries.is_empty() {
        return Err(ImgQualityError::AnalysisError(
            "Photos verifier has no candidate library to query".to_string(),
        ));
    }
    let mut pending = uuids.to_vec();
    let mut probes = Vec::new();
    let mut resolved_library = None;
    for library in libraries {
        if pending.is_empty() {
            break;
        }
        match query_library(library, &pending) {
            Ok(library_probes) => {
                if !library_probes.is_empty() {
                    resolved_library = Some(library.clone());
                }
                for probe in library_probes {
                    let Some(position) = pending.iter().position(|uuid| uuid == &probe.uuid) else {
                        return Err(ImgQualityError::AnalysisError(format!(
                            "Photos verifier library query returned duplicate or unexpected UUID {}",
                            probe.uuid
                        )));
                    };
                    pending.remove(position);
                    probes.push(probe);
                }
            }
            Err(err) => return Err(err),
        }
    }
    Ok((probes, resolved_library))
}

fn query_osxphotos_asset_probes_from_library(
    uuids: &[String],
    library: &Path,
) -> Result<Vec<FastImgLibraryAssetProbe>> {
    if uuids.is_empty() {
        return Ok(Vec::new());
    }
    let osxphotos = resolve_osxphotos_command()?;
    let mut uuid_file = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "fast_img_osxphotos_uuid_query",
        Some("mfb_fast_img_osxphotos_uuid"),
        Some(".txt"),
    )
    .map_err(|e| {
        ImgQualityError::AnalysisError(format!("create osxphotos UUID query file failed: {e}"))
    })?;
    for uuid in uuids {
        writeln!(uuid_file, "{uuid}").map_err(|e| {
            ImgQualityError::AnalysisError(format!("write osxphotos UUID query file failed: {e}"))
        })?;
    }
    uuid_file.flush().map_err(|e| {
        ImgQualityError::AnalysisError(format!("flush osxphotos UUID query file failed: {e}"))
    })?;
    let mut command = std::process::Command::new(&osxphotos);
    command
        .arg("query")
        .arg("--db")
        .arg(library)
        .arg("--uuid-from-file")
        .arg(uuid_file.path())
        .arg("--mute")
        .arg("--json");
    let timeout = fast_img_osxphotos_query_timeout(uuids.len());
    tracing::info!(
        target: "photos_import",
        uuid_count = uuids.len(),
        timeout_secs = timeout.as_secs(),
        library = %library.display(),
        "Starting osxphotos query"
    );
    let start = std::time::Instant::now();
    let output = crate::process_runner::ManagedProcess::spawn(&mut command)
        .and_then(|process| {
            process.wait_liveness_timeout(timeout, timeout, "fast-img osxphotos batch query")
        })
        .map_err(|e| ImgQualityError::AnalysisError(format!("osxphotos query failed: {e}")))?;
    let elapsed = start.elapsed();
    record_osxphotos_query_startup_time(elapsed.as_secs());
    tracing::info!(
        target: "photos_import",
        uuid_count = uuids.len(),
        elapsed_secs = elapsed.as_secs(),
        library = %library.display(),
        "osxphotos query completed"
    );
    if !output.status.success() {
        return Err(ImgQualityError::AnalysisError(format!(
            "osxphotos query failed for {} UUID(s): {}",
            uuids.len(),
            output.stderr.trim()
        )));
    }
    let stderr = output.stderr.trim();
    if stderr.contains("OperationalError")
        || stderr.contains("unable to open database file")
        || stderr.contains("Operation not permitted")
    {
        return Err(ImgQualityError::AnalysisError(format!(
            "osxphotos query blocked by system permissions or locked DB: {stderr}"
        )));
    }
    let records: Vec<FastImgQueryRecord> = serde_json::from_str(&output.stdout).map_err(|e| {
        ImgQualityError::AnalysisError(format!("parse osxphotos query JSON failed: {e}"))
    })?;
    let requested = uuids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    records
        .into_iter()
        .map(|record| {
            if !requested.contains(record.uuid.as_str()) {
                return Err(ImgQualityError::AnalysisError(format!(
                    "osxphotos query returned unexpected UUID {}",
                    record.uuid
                )));
            }
            Ok(FastImgLibraryAssetProbe {
                uuid: record.uuid,
                path: PathBuf::from(record.path),
                iscloudasset: record.iscloudasset,
                incloud: record.incloud,
                ismissing: record.ismissing,
            })
        })
        .collect()
}

fn query_osxphotos_asset_probes(uuids: &[String]) -> Result<Vec<FastImgLibraryAssetProbe>> {
    if uuids.is_empty() {
        return Ok(Vec::new());
    }
    let mut libraries = crate::common_utils::photos_library_paths()?;
    let library_hint = OSXPHOTOS_IMPORT_LIBRARY_HINT
        .lock()
        .map_err(|_| {
            ImgQualityError::AnalysisError(
                "Photos verifier library hint lock is poisoned".to_string(),
            )
        })?
        .clone();
    if let Some(library_hint) = library_hint
        && library_hint.is_dir()
    {
        libraries.retain(|library| library != &library_hint);
        libraries.insert(0, library_hint);
    }
    let (probes, resolved_library) =
        query_osxphotos_asset_probes_in_libraries_with(uuids, &libraries, |library, uuids| {
            query_osxphotos_asset_probes_from_library(uuids, library)
        })?;
    if let Some(resolved_library) = resolved_library {
        *OSXPHOTOS_IMPORT_LIBRARY_HINT.lock().map_err(|_| {
            ImgQualityError::AnalysisError(
                "Photos verifier library hint lock is poisoned".to_string(),
            )
        })? = Some(resolved_library);
    }
    Ok(probes)
}

const FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_ENV: &str = "MFB_FAST_IMG_ICLOUD_VERIFY_ATTEMPTS";
const FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE_ENV: &str = "MFB_FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE";
const FAST_IMG_ICLOUD_VERIFY_DELAY_MS_ENV: &str = "MFB_FAST_IMG_ICLOUD_VERIFY_DELAY_MS";
const FAST_IMG_OSXPHOTOS_QUERY_TIMEOUT_SECS_ENV: &str = "MFB_FAST_IMG_OSXPHOTOS_QUERY_TIMEOUT_SECS";
const FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF_ENV: &str = "MFB_FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF";
const FAST_IMG_PHOTOS_IMPORT_TIMEOUT_SECS_ENV: &str = "MFB_FAST_IMG_PHOTOS_IMPORT_TIMEOUT_SECS";
const FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE_ENV: &str = "MFB_FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE";
const MACOS_OSASCRIPT_PATH: &str = "/usr/bin/osascript";
const MACOS_PS_PATH: &str = "/bin/ps";
const MACOS_VM_STAT_PATH: &str = "/usr/bin/vm_stat";
const MACOS_PGREP_PATH: &str = "/usr/bin/pgrep";
const FAST_IMG_SYSTEM_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const FAST_IMG_MEDIA_PROBE_TIMEOUT: Duration = Duration::from_mins(5);
#[cfg(target_os = "macos")]
const MACOS_OPEN_PATH: &str = "/usr/bin/open";
#[cfg(target_os = "macos")]
const MACOS_KILLALL_PATH: &str = "/usr/bin/killall";
#[cfg(target_os = "macos")]
const MACOS_XATTR_PATH: &str = "/usr/bin/xattr";
const FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_DEFAULT: usize = 5;
const FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_MAX: usize = 5;
const FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE_DEFAULT: usize = 64;
const FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE_MAX: usize = 128;
const FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE_DEFAULT: usize = 50;
const FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE_MAX: usize = 50;
const FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE: usize = 10;
const FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP: usize = 150;
const FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP: usize = 100;
const FAST_IMG_PHOTOS_IMPORT_RELAUNCH_INTERVAL_FILES: usize = 500; // Increased from 250 to reduce restart frequency

/// Session-level cache for adaptive osxphotos query timeout.
/// Shared across all three timeout functions to avoid the "same name, different
/// storage" bug.
static OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(120);
/// Ordering hint only: every proof query still asks osxphotos for its pending
/// UUIDs and continues through other libraries when the hint is incomplete.
static OSXPHOTOS_IMPORT_LIBRARY_HINT: std::sync::Mutex<Option<PathBuf>> =
    std::sync::Mutex::new(None);

/// Session-level flag tracking whether osxphotos has been proven responsive.
/// First successful query sets this to true, enabling faster subsequent
/// queries.
static OSXPHOTOS_WARMED_UP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
const FAST_IMG_PHOTOS_IMPORT_BATCH_DELAY_MS: u64 = 2_000;
const FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL: usize = 20;
const FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_SECS: u64 = 30;
const FAST_IMG_PHOTOS_IMPORT_WINDOW_PAUSE_SECS: u64 = 60;
// Initial attempt + four bounded recovery attempts. Permanent authorization,
// unsupported-format, missing-file, and corruption errors are never retried.
const FAST_IMG_PHOTOS_IMPORT_POISON_RETRY_LIMIT: usize = 4;
#[cfg(all(target_os = "macos", not(test)))]
const FAST_IMG_PHOTOS_IMPORT_RELAUNCH_PROCESS_TIMEOUT_SECS: u64 = 120; // Increased from 45s
#[cfg(all(target_os = "macos", not(test)))]
const FAST_IMG_PHOTOS_IMPORT_RELAUNCH_SETTLE_SECS: u64 = 20; // Increased from 15s
#[cfg(all(target_os = "macos", not(test)))]
const FAST_IMG_PHOTOS_IMPORT_RELAUNCH_OPEN_ATTEMPTS: usize = 3;
#[cfg(all(target_os = "macos", not(test)))]
const FAST_IMG_PHOTOS_IMPORT_RELAUNCH_OPEN_RETRY_SECS: u64 = 5;

/// ENV: Disable periodic Photos relaunch for extreme edge cases where relaunch
/// is unstable. WARNING: May cause Photos to accumulate state and eventually
/// poison the session. Only use if relaunch timeout is consistently fatal.
const FAST_IMG_DISABLE_PERIODIC_RELAUNCH_ENV: &str = "MFB_FAST_IMG_DISABLE_PERIODIC_RELAUNCH";
const FAST_IMG_PHOTOS_IMPORT_LOCK_FILE: &str = "photos_import.lock";

#[derive(Debug)]
struct FastImgImportProbeTarget {
    rel_path: String,
    import_identifier: String,
    osxphotos_uuid: String,
}

fn fast_img_import_probe_targets(
    report_pairs: &[(String, String)],
) -> Result<Vec<FastImgImportProbeTarget>> {
    let mut rel_paths = BTreeSet::new();
    report_pairs
        .iter()
        .map(|(rel_path, import_identifier)| {
            if !rel_paths.insert(rel_path) {
                return Err(ImgQualityError::AnalysisError(format!(
                    "Photos import report duplicated relative path {rel_path}"
                )));
            }
            Ok(FastImgImportProbeTarget {
                rel_path: rel_path.clone(),
                import_identifier: import_identifier.clone(),
                osxphotos_uuid: osxphotos_uuid_from_photos_import_identifier(import_identifier)?
                    .to_string(),
            })
        })
        .collect()
}

fn fast_img_env_usize(name: &'static str) -> Option<usize> {
    match std::env::var(name) {
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                crate::media_conversion_gate::delivery_jxl_batch_audit(
                    "fast_img_env_parse",
                    format!("{name} has malformed usize value {value:?}: {err}"),
                );
                None
            }
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(err) => {
            crate::media_conversion_gate::delivery_jxl_batch_audit(
                "fast_img_env_parse",
                format!("{name} could not be read: {err}"),
            );
            None
        }
    }
}

fn fast_img_env_u64(name: &'static str) -> Option<u64> {
    match std::env::var(name) {
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                crate::media_conversion_gate::delivery_jxl_batch_audit(
                    "fast_img_env_parse",
                    format!("{name} has malformed u64 value {value:?}: {err}"),
                );
                None
            }
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(err) => {
            crate::media_conversion_gate::delivery_jxl_batch_audit(
                "fast_img_env_parse",
                format!("{name} could not be read: {err}"),
            );
            None
        }
    }
}

fn fast_img_positive_usize_env(name: &'static str, default: usize, max: usize) -> usize {
    match fast_img_env_usize(name) {
        Some(value) if value > 0 => value.min(max),
        Some(value) => {
            crate::media_conversion_gate::delivery_jxl_batch_audit(
                "fast_img_env_parse",
                format!("{name} must be positive; got {value}; using default {default}"),
            );
            default
        }
        None => default,
    }
}

fn fast_img_positive_secs_env(name: &'static str, default: Duration) -> Duration {
    match fast_img_env_u64(name) {
        Some(value) if value > 0 => Duration::from_secs(value),
        Some(value) => {
            crate::media_conversion_gate::delivery_jxl_batch_audit(
                "fast_img_env_parse",
                format!("{name} must be positive seconds; got {value}; using default {default:?}"),
            );
            default
        }
        None => default,
    }
}

fn fast_img_icloud_upload_verify_attempts() -> usize {
    fast_img_positive_usize_env(
        FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_ENV,
        FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_DEFAULT,
        FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_MAX,
    )
}

fn fast_img_icloud_upload_verify_batch_size() -> usize {
    fast_img_positive_usize_env(
        FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE_ENV,
        FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE_DEFAULT,
        FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE_MAX,
    )
}

fn fast_img_icloud_upload_verify_delay() -> Duration {
    match fast_img_env_u64(FAST_IMG_ICLOUD_VERIFY_DELAY_MS_ENV) {
        Some(value) => Duration::from_millis(value),
        None => Duration::from_secs(2),
    }
}

/// Adaptive timeout for osxphotos query based on library warm state.
///
/// Strategy: Start conservative (2 min), extend on timeout to 8 min max.
/// This avoids penalizing small/fast libraries while remaining safe for large
/// ones.
fn fast_img_osxphotos_query_timeout(item_count: usize) -> Duration {
    use std::sync::atomic::Ordering;

    match std::env::var(FAST_IMG_OSXPHOTOS_QUERY_TIMEOUT_SECS_ENV) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(v) if v > 0 => return Duration::from_secs(v),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    target: "photos_import",
                    "Invalid {FAST_IMG_OSXPHOTOS_QUERY_TIMEOUT_SECS_ENV}={raw:?}: {e}; using adaptive timeout instead"
                );
            }
        },
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            tracing::warn!(
                target: "photos_import",
                "Failed to read {FAST_IMG_OSXPHOTOS_QUERY_TIMEOUT_SECS_ENV}: {e}; using adaptive timeout instead"
            );
        }
    }

    // Use cached base from previous successful query, or initial conservative value
    let base_secs = OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::Relaxed);

    // If osxphotos hasn't warmed up yet, add extra buffer for cold start
    let cold_start_buffer = if OSXPHOTOS_WARMED_UP.load(Ordering::Relaxed) {
        0
    } else {
        180 // +3 min for first query cold start
    };

    // Scaling: +1 minute per 100 items, capped at 10 minutes
    let scaling_secs = ((item_count as u64) / 100 * 60).min(600);
    Duration::from_secs(base_secs + cold_start_buffer + scaling_secs)
}

/// Called after successful query to cache the observed startup time.
/// Next query will use this as the base, avoiding over-waiting.
fn record_osxphotos_query_startup_time(secs: u64) {
    use std::sync::atomic::Ordering;

    // Mark osxphotos as warmed up after first success
    OSXPHOTOS_WARMED_UP.store(true, Ordering::Relaxed);

    // Dynamic ratchet with decay: allow base to decrease if consistently fast
    // Only increase if new_base is significantly higher (>50% increase)
    let new_base = (secs + 30).min(480); // Cap at 8 min
    let old = OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::Relaxed);

    if new_base > old {
        // Only increase if gap is significant (prevents noise)
        if new_base > old + old / 2 {
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(new_base, Ordering::Relaxed);
            tracing::info!(
                target: "photos_import",
                old_base_secs = old,
                new_base_secs = new_base,
                "Adaptive osxphotos timeout increased (significant slowdown)"
            );
        }
    } else if old > 180 && new_base < old / 2 {
        // Gradual decay: if consistently fast and base is high, decrease slowly
        let decayed = (old * 3 / 4).max(120); // Decay 25%, floor at 2min
        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(decayed, Ordering::Relaxed);
        tracing::info!(
            target: "photos_import",
            old_base_secs = old,
            new_base_secs = decayed,
            actual_secs = secs,
            "Adaptive osxphotos timeout decreased (consistent fast performance)"
        );
    }
}

fn fast_img_photos_import_timeout() -> Duration {
    fast_img_positive_secs_env(
        FAST_IMG_PHOTOS_IMPORT_TIMEOUT_SECS_ENV,
        Duration::from_secs(120),
    )
}

fn fast_img_photos_import_batch_size() -> usize {
    fast_img_positive_usize_env(
        FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE_ENV,
        FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE_DEFAULT,
        FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE_MAX,
    )
}

fn fast_img_require_icloud_upload_proof() -> bool {
    let Some(value) = std::env::var_os(FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF_ENV) else {
        return false;
    };
    let raw = value.to_string_lossy();
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        other => {
            tracing::warn!(
                target: "fast_img",
                env = FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF_ENV,
                value = %other,
                "invalid fast-img iCloud upload proof flag; defaulting to Photos local custody proof"
            );
            false
        }
    }
}

const fn photos_sync_status(probe: &FastImgLibraryAssetProbe) -> &'static str {
    if probe.iscloudasset && matches!(probe.incloud, Some(true)) {
        "uploaded"
    } else {
        "photos_local"
    }
}

fn photos_probe_satisfies_policy(
    probe: &FastImgLibraryAssetProbe,
    require_icloud_upload: bool,
) -> bool {
    if probe.ismissing {
        return false;
    }
    !require_icloud_upload || (probe.iscloudasset && probe.incloud == Some(true))
}

fn query_uploaded_asset_probes_batch_with_retry<Q, S>(
    targets: &[FastImgImportProbeTarget],
    attempts: usize,
    batch_size: usize,
    delay: Duration,
    require_icloud_upload: bool,
    query_assets: &mut Q,
    sleep: S,
) -> Result<BTreeMap<String, FastImgLibraryAssetProbe>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    S: Fn(Duration),
{
    let attempts = attempts.max(1);
    let batch_size = batch_size.max(1);
    let mut verified = BTreeMap::new();
    let mut last_states = BTreeMap::new();
    for attempt in 1..=attempts {
        let mut query_round_failed = false;
        let pending_targets = targets
            .iter()
            .filter(|target| !verified.contains_key(&target.rel_path))
            .collect::<Vec<_>>();
        if pending_targets.is_empty() {
            return Ok(verified);
        }
        let mut checked_this_round = 0usize;
        for chunk in pending_targets.chunks(batch_size) {
            let query_uuids = chunk
                .iter()
                .map(|target| target.osxphotos_uuid.clone())
                .collect::<Vec<_>>();
            let queried_uuid_set = query_uuids
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let queried = match query_assets(&query_uuids) {
                Ok(probes) => probes,
                Err(err)
                    if attempt < attempts
                        && photos_import_retry_reason(&err.to_string()).is_some() =>
                {
                    tracing::warn!(
                        target: "photos_import",
                        attempt,
                        attempts,
                        error = %err,
                        "Photos verifier query failed transiently; retrying the proof query"
                    );
                    query_round_failed = true;
                    break;
                }
                Err(err) => return Err(err),
            };
            let mut probes_by_uuid = BTreeMap::new();
            for probe in queried {
                if !queried_uuid_set.contains(probe.uuid.as_str()) {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "Photos verifier batch query returned unexpected UUID {}",
                        probe.uuid
                    )));
                }
                if probes_by_uuid.insert(probe.uuid.clone(), probe).is_some() {
                    return Err(ImgQualityError::AnalysisError(
                        "Photos verifier batch query returned duplicate UUID".to_string(),
                    ));
                }
            }
            checked_this_round = checked_this_round.saturating_add(query_uuids.len());
            for target in chunk {
                let Some(probe) = probes_by_uuid.remove(&target.osxphotos_uuid) else {
                    last_states.insert(
                        target.rel_path.clone(),
                        format!("uuid={} query returned no record", target.osxphotos_uuid),
                    );
                    continue;
                };
                if probe.uuid != target.osxphotos_uuid {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "Photos verifier UUID mismatch for {}: import_id={} query_uuid={} query={}",
                        target.rel_path,
                        target.import_identifier,
                        target.osxphotos_uuid,
                        probe.uuid
                    )));
                }
                if photos_probe_satisfies_policy(&probe, require_icloud_upload) {
                    verified.insert(target.rel_path.clone(), probe);
                    continue;
                }
                last_states.insert(
                    target.rel_path.clone(),
                    format!(
                        "uuid={} iscloudasset={} incloud={:?} ismissing={} policy={}",
                        target.osxphotos_uuid,
                        probe.iscloudasset,
                        probe.incloud,
                        probe.ismissing,
                        if require_icloud_upload {
                            "icloud_upload_required"
                        } else {
                            "photos_local_required"
                        }
                    ),
                );
            }
        }

        if query_round_failed {
            sleep(delay);
            continue;
        }

        if verified.len() == targets.len() {
            if attempt > 1 {
                tracing::info!(
                    target: "fast_img",
                    attempt,
                    uploaded = verified.len(),
                    total = targets.len(),
                    "Photos verifier proof complete"
                );
            }
            return Ok(verified);
        }

        if attempt < attempts {
            let pending = targets.len().saturating_sub(verified.len());
            let sample = last_states
                .iter()
                .find(|(rel_path, _)| !verified.contains_key(*rel_path))
                .map_or_else(
                    || "no pending state recorded".to_string(),
                    |(rel_path, state)| format!("{rel_path}: {state}"),
                );
            tracing::info!(
                target: "fast_img",
                attempt,
                attempts,
                verified = verified.len(),
                pending,
                total = targets.len(),
                batch_size,
                checked_this_round,
                require_icloud_upload,
                sample = %sample,
                "Photos verifier waiting for throttled proof"
            );
            sleep(delay);
        }
    }

    Err(ImgQualityError::AnalysisError(format!(
        "Photos verifier has {} asset(s) without required proof after {attempts} batch query \
         attempt(s): {}",
        targets.len().saturating_sub(verified.len()),
        format_pending_upload_states(targets, &verified, &last_states)
    )))
}

fn format_pending_upload_states(
    targets: &[FastImgImportProbeTarget],
    verified: &BTreeMap<String, FastImgLibraryAssetProbe>,
    last_states: &BTreeMap<String, String>,
) -> String {
    let examples: Vec<String> = targets
        .iter()
        .filter(|target| !verified.contains_key(&target.rel_path))
        .take(5)
        .map(|target| {
            let state = last_states
                .get(&target.rel_path)
                .map_or("not queried", String::as_str);
            format!("{} ({state})", target.rel_path)
        })
        .collect();
    if examples.is_empty() {
        "none".to_string()
    } else {
        examples.join("; ")
    }
}

fn osxphotos_candidate_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".local/bin/osxphotos"));
        candidates.push(home.join(".cargo/bin/osxphotos"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/osxphotos"));
    candidates.push(PathBuf::from("/usr/local/bin/osxphotos"));
    candidates
}

fn resolve_osxphotos_command() -> Result<PathBuf> {
    let home = std::env::var_os(crate::constants::ENV_HOME).map(PathBuf::from);
    for candidate in osxphotos_candidate_paths(home.as_deref()) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(path) = crate::common_utils::resolve_tool_path("osxphotos") {
        return Ok(path);
    }
    Err(ImgQualityError::AnalysisError(
        "osxphotos not found; tried ~/.local/bin, ~/.cargo/bin, /opt/homebrew/bin, \
         /usr/local/bin, and PATH. Install osxphotos or ensure its directory is visible to the \
         app launch environment."
            .to_string(),
    ))
}

fn resolve_osascript_command() -> PathBuf {
    PathBuf::from(MACOS_OSASCRIPT_PATH)
}

#[derive(Debug)]
struct PhotosImportLock {
    file: File,
}

impl Drop for PhotosImportLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: `file` is an open lock-file descriptor owned by this guard; unlocking
            // an advisory flock on drop is side-effect isolated to this
            // descriptor.
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

fn photos_import_lock_path() -> Result<PathBuf> {
    let lock_dir = crate::process_lock::get_mfb_root()
        .map_err(|err| {
            ImgQualityError::AnalysisError(format!("resolve MFB lock root failed: {err}"))
        })?
        .join("locks");
    std::fs::create_dir_all(&lock_dir).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "create Photos import lock directory {} failed: {err}",
            lock_dir.display()
        ))
    })?;
    Ok(lock_dir.join(FAST_IMG_PHOTOS_IMPORT_LOCK_FILE))
}

fn acquire_photos_import_lock() -> Result<PhotosImportLock> {
    let lock_path = photos_import_lock_path()?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "open Photos import lock {} failed: {err}",
                lock_path.display()
            ))
        })?;
    #[cfg(unix)]
    {
        // SAFETY: `file` is a valid open descriptor for the process-local Photos import
        // lock. `flock` is advisory and does not outlive the file descriptor
        // held by `PhotosImportLock`.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Err(ImgQualityError::AnalysisError(format!(
                    "another Photos/iCloud import is already running; lock={}",
                    lock_path.display()
                )));
            }
            return Err(ImgQualityError::AnalysisError(format!(
                "acquire Photos import lock {} failed: {err}",
                lock_path.display()
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = &lock_path;
    }
    Ok(PhotosImportLock { file })
}

#[cfg(target_os = "macos")]
fn clear_quarantine_xattr(path: &Path) -> Result<()> {
    let mut command = std::process::Command::new(MACOS_XATTR_PATH);
    command.arg("-d").arg("com.apple.quarantine").arg(path);
    let output = run_fast_img_command_with_timeout(
        &mut command,
        FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
        "fast-img quarantine clear",
    )
    .map_err(|err| {
        ImgQualityError::AnalysisError(format!("xattr quarantine clear command failed: {err}"))
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such xattr") || stderr.contains("No such file") {
        return Ok(());
    }
    Err(ImgQualityError::AnalysisError(format!(
        "xattr quarantine clear failed for {}: {}",
        path.display(),
        stderr.trim()
    )))
}

#[cfg(not(target_os = "macos"))]
const fn clear_quarantine_xattr(path: &Path) {
    let _ = path;
}

#[cfg(target_os = "macos")]
fn path_has_quarantine_xattr(path: &Path) -> Result<bool> {
    let mut command = std::process::Command::new(MACOS_XATTR_PATH);
    command.arg("-p").arg("com.apple.quarantine").arg(path);
    let output = run_fast_img_command_with_timeout(
        &mut command,
        FAST_IMG_SYSTEM_COMMAND_TIMEOUT,
        "fast-img quarantine probe",
    )
    .map_err(|e| ImgQualityError::AnalysisError(format!("xattr probe failed: {e}")))?;
    Ok(output.status.success())
}

#[cfg(not(target_os = "macos"))]
const fn path_has_quarantine_xattr(path: &Path) -> bool {
    let _ = path;
    false
}

fn marker_entry_out_rel(source_rel: &str, entry: &Blake3Entry) -> String {
    entry.out_rel.clone().unwrap_or_else(|| {
        // Legacy entry without out_rel: the .JXL guess fails closed at hash verify,
        // but audit it so a wrong guess reads as "missing out_rel", not corruption.
        crate::media_conversion_gate::delivery_pipeline_batch_audit(
            "marker_out_rel_guess",
            format!("Blake3 entry for {source_rel} lacks out_rel; guessing .JXL sibling"),
        );
        PathBuf::from(source_rel)
            .with_extension("JXL")
            .to_string_lossy()
            .to_string()
    })
}

fn validate_fast_img_marker_path_contract(marker: &WorkingCopyMarker) -> Result<()> {
    marker
        .validate_checkpoint_path_contract(&marker.working_copy)
        .map_err(|err| {
            ImgQualityError::AnalysisError(format!("fast-img marker path validation failed: {err}"))
        })
}

fn fast_img_marker_output_paths(marker: &WorkingCopyMarker) -> Result<Vec<(String, PathBuf)>> {
    validate_fast_img_marker_path_contract(marker)?;
    let mut outputs = Vec::new();
    for (source_rel, entry) in &marker.blake3_log {
        let out_rel = marker_entry_out_rel(source_rel, entry);
        outputs.push((out_rel.clone(), marker.working_copy.join(out_rel)));
    }
    outputs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(outputs)
}

/// Prompt the user for a yes/no confirmation (§Import confirm gate, GAP-3).
///
/// Reads from stdin. Returns `true` for "y"/"Y", `false` otherwise.
///
/// # Errors
/// Returns an error on I/O failure.
pub fn prompt_user_confirm(message: &str) -> Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        tracing::warn!(
            target: "fast_img_confirm",
            "confirmation required without an interactive terminal; treating as explicit no"
        );
        return Ok(false);
    }
    print!("{message}");
    std::io::stdout()
        .flush()
        .map_err(ImgQualityError::IoError)?;
    let stdin = std::io::stdin();
    let stdin_line = stdin
        .lock()
        .lines()
        .next()
        .transpose()
        .map_err(ImgQualityError::IoError)?;
    let line = if let Some(line) = stdin_line {
        line
    } else {
        tracing::warn!(
            target: "fast_img_confirm",
            "confirmation prompt reached EOF; treating as explicit no"
        );
        String::new()
    };
    Ok(matches!(line.trim(), "y" | "Y"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // ── §Detection (synthesized fixtures — no project assets) ────────────────

    fn write_valid_test_jpeg(path: &Path, rgb: [u8; 3]) {
        let image = image::RgbImage::from_pixel(1, 1, image::Rgb(rgb));
        image
            .save_with_format(path, image::ImageFormat::Jpeg)
            .unwrap();
    }

    #[test]
    fn jpeg_magic_detector_accepts_minimal_jfif() {
        use crate::image::format_detect::{FormatKind, detect_true_format};

        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).unwrap();
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jpeg);
    }

    #[test]
    fn jpeg_magic_detector_accepts_exif_header() {
        use crate::image::format_detect::{FormatKind, detect_true_format};

        // FF D8 FF E1 — most camera JPEGs (EXIF APP1)
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x10]).unwrap();
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jpeg);
    }

    #[test]
    fn jpeg_magic_detector_accepts_icc_header() {
        use crate::image::format_detect::{FormatKind, detect_true_format};

        // FF D8 FF E2 — ICC profile APP2
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xFF, 0xD8, 0xFF, 0xE2, 0x00, 0x10]).unwrap();
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jpeg);
    }

    #[test]
    fn jpeg_magic_detector_accepts_progressive_jpeg_marker() {
        use crate::image::format_detect::{FormatKind, detect_true_format};

        // FF D8 FF C2 — SOF2 progressive
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xFF, 0xD8, 0xFF, 0xC2, 0x00, 0x10]).unwrap();
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jpeg);
    }

    #[test]
    fn jpeg_magic_detector_accepts_cmyk_jpeg_marker() {
        use crate::image::format_detect::{FormatKind, detect_true_format};

        // CMYK JPEG still starts FF D8 FF (Adobe APP14 is deeper in the stream)
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xFF, 0xD8, 0xFF, 0xEE, 0x00, 0x0E]).unwrap();
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jpeg);
    }

    #[test]
    fn true_jpeg_accepts_arbitrary_filename_extensions() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        for rel in [
            "camera.mp4",
            "camera.png",
            "camera.heic",
            "camera.txt",
            "camera",
        ] {
            let path = temp_dir.path().join(rel);
            write_valid_test_jpeg(&path, [20, 40, 60]);
            assert!(
                is_true_jpeg(&path).unwrap(),
                "true JPEG rejected because filename looked like {rel}"
            );
        }
    }

    #[test]
    fn osxphotos_candidates_include_user_local_bin_for_gui_launches() {
        let candidates = osxphotos_candidate_paths(Some(Path::new("/Users/example")));

        assert!(candidates.contains(&PathBuf::from("/Users/example/.local/bin/osxphotos")));
        assert!(candidates.contains(&PathBuf::from("/opt/homebrew/bin/osxphotos")));
        assert!(candidates.contains(&PathBuf::from("/usr/local/bin/osxphotos")));
    }

    #[test]
    fn photos_import_ids_fail_closed_when_photos_returns_zero_jxl_ids() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("src_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: "out".to_string(),
                library_asset: None,
            },
        );
        let output_paths = fast_img_marker_output_paths(&marker).unwrap();

        let err =
            fast_img_pairs_from_photos_import_ids(&output_paths, b"\n", marker.src_jpeg_count)
                .unwrap_err();

        assert!(
            err.to_string()
                .contains("Photos AppleScript import returned 0 IDs for 1 JXL outputs"),
            "unexpected: {err}"
        );
        assert!(
            err.to_string()
                .contains("osxphotos import filters .JXL before Photos sees it"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn photos_zero_import_session_error_names_library_session_and_preserves_sources() {
        let stderr = "1359:1452: execution error: Photos returned 0 imported items for \
                      /tmp/IMG_7564.JXL (batch 294) (-2700)";

        let err = photos_applescript_import_chunk_error("JXL", 49, 531, 10, stderr);
        let message = err.to_string();

        assert!(
            message.contains("Photos AppleScript JXL import chunk 49/531"),
            "unexpected: {message}"
        );
        assert!(
            message.contains("zero imported items: /tmp/IMG_7564.JXL"),
            "unexpected: {message}"
        );
        assert!(
            message.contains("preserves sources until destructive cleanup gates pass"),
            "unexpected: {message}"
        );
    }

    #[test]
    fn photos_zero_import_session_error_handles_multi_file_batch() {
        let stderr = "1359:1452: execution error: Photos returned 0 imported items for batch \
                      starting /tmp/a.JXL (expected 50, batch 63) (-2700)";

        let err = photos_applescript_import_chunk_error("JXL", 1, 64, 100, stderr);
        let message = err.to_string();

        assert!(
            message.contains("zero imported items: /tmp/a.JXL"),
            "unexpected: {message}"
        );
        assert!(
            message.contains("preserves sources until destructive cleanup gates pass"),
            "unexpected: {message}"
        );
    }

    #[test]
    fn photos_import_ids_reject_ambiguous_multi_file_order() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("src_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let mut marker = WorkingCopyMarker::new(src_root, wc, 2);
        marker.blake3_log.insert(
            "b.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("nested/b.JXL".to_string()),
                src: "src-b".to_string(),
                out: "out-b".to_string(),
                library_asset: None,
            },
        );
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src-a".to_string(),
                out: "out-a".to_string(),
                library_asset: None,
            },
        );
        let output_paths = fast_img_marker_output_paths(&marker).unwrap();

        let err = fast_img_pairs_from_photos_import_ids(
            &output_paths,
            b"UUID-B\nUUID-A\n",
            marker.src_jpeg_count,
        )
        .expect_err("Photos UUID order is undefined for multi-file import output");

        assert!(
            err.to_string().contains("exactly one JXL output"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn photos_import_preflight_rejects_missing_jxl_output() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let missing = temp_dir.path().join("missing.JXL");

        let err = validate_photos_import_output_paths(&[("missing.JXL".to_string(), missing)])
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("Photos import preflight missing JXL output"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn optimized_import_album_names_match_icloud_import_default() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        let mut marker = WorkingCopyMarker::new(src_root, wc, 3);
        let cases = [
            ("root.JXL", "✨/✨Batch"),
            ("微信/a.JXL", "✨/✨Batch/微信"),
            ("foo/bar/b.JXL", "✨/✨Batch/foo/bar"),
        ];

        for (rel_path, expected_album) in cases {
            assert_eq!(
                fast_img_optimized_import_album_name(&marker, rel_path),
                expected_album
            );
        }

        marker.working_copy = temp_dir.path().join("Batch_collected_optimized");
        assert_eq!(
            fast_img_optimized_import_album_name(&marker, "root.JXL"),
            "✨/✨Batch"
        );
    }

    #[test]
    fn photos_import_args_include_optimized_album_pairs() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let mut marker = WorkingCopyMarker::new(src_root, wc.clone(), 1);
        marker.blake3_log.insert(
            "微信/a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("微信/a.JXL".to_string()),
                src: "src".to_string(),
                out: "out".to_string(),
                library_asset: None,
            },
        );
        let output_paths = fast_img_marker_output_paths(&marker).unwrap();

        let entries = fast_img_photos_import_manifest_entries(&marker, &output_paths);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, wc.join("微信/a.JXL"));
        assert_eq!(entries[0].1, "✨/✨Batch/微信");
    }

    #[test]
    fn fast_img_output_import_candidates_preserve_recorded_avif_extension() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Meme_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let mut marker = WorkingCopyMarker::new(src_root, wc.clone(), 1);
        marker.blake3_log.insert(
            "nested/source.png".to_string(),
            Blake3Entry {
                out_rel: Some("nested/source.avif".to_string()),
                src: "source-hash".to_string(),
                out: "avif-hash".to_string(),
                library_asset: None,
            },
        );

        let candidates = build_fast_img_output_import_candidates(&marker).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].rel_path, "nested/source.avif");
        assert_eq!(candidates[0].path, wc.join("nested/source.avif"));
        assert_eq!(candidates[0].blake3, "avif-hash");
    }

    #[test]
    fn photos_import_manifest_text_pairs_paths_with_albums() {
        let entries = vec![
            (PathBuf::from("/tmp/a.JXL"), "✨A".to_string()),
            (PathBuf::from("/tmp/微信/b.JXL"), "✨微信".to_string()),
        ];

        let manifest = photos_import_manifest_text(&entries).unwrap();

        assert_eq!(manifest, "/tmp/a.JXL\n✨A\n/tmp/微信/b.JXL\n✨微信");
    }

    #[test]
    fn photos_import_manifest_text_rejects_line_breaks() {
        let entries = vec![(PathBuf::from("/tmp/a\nb.JXL"), "✨A".to_string())];

        let err = photos_import_manifest_text(&entries).unwrap_err();

        assert!(
            err.to_string().contains("must not contain line breaks"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn library_handle_quarantine_check_uses_controlled_output_path() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("src_optimized");
        let out = wc.join("a.JXL");
        let photos_asset = temp_dir.path().join("Photos Library.photoslibrary/a.JXL");
        std::fs::create_dir_all(&wc).unwrap();
        std::fs::create_dir_all(
            photos_asset
                .parent()
                .expect("test Photos asset path has a parent"),
        )
        .unwrap();
        std::fs::write(&out, b"jxl").unwrap();
        std::fs::write(&photos_asset, b"jxl").unwrap();
        let out_hash = crate::common_utils::calculate_blake3_hash(&out)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: out_hash,
                library_asset: None,
            },
        );

        let handle = library_handle_from_probes(
            &marker,
            &[("a.JXL".to_string(), "UUID-A".to_string())],
            |_uuid| {
                Ok(FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: photos_asset.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                })
            },
            |path| Ok(path != out),
        )?;

        assert!(!handle.imported_assets[0].quarantined);
        Ok(())
    }

    #[test]
    fn generic_media_import_handle_verifies_gif_output_blake3() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let out = temp_dir.path().join("clip_gif/nested/a.GIF");
        let photos_asset = temp_dir.path().join("Photos Library.photoslibrary/a.GIF");
        std::fs::create_dir_all(out.parent().expect("test output has parent")).unwrap();
        std::fs::create_dir_all(
            photos_asset
                .parent()
                .expect("test Photos asset path has parent"),
        )
        .unwrap();
        std::fs::write(&out, b"gif-bytes").unwrap();
        std::fs::write(&photos_asset, b"gif-bytes").unwrap();
        let out_hash = crate::common_utils::calculate_blake3_hash(&out)?;
        let candidates = vec![PhotosImportCandidate {
            rel_path: "nested/a.GIF".to_string(),
            path: out.clone(),
            blake3: out_hash.clone(),
            album_name: "✨clip_gif".to_string(),
        }];

        let handle = library_handle_from_media_output_probes(
            &candidates,
            &[("nested/a.GIF".to_string(), "UUID-A".to_string())],
            |uuids| {
                assert_eq!(uuids, ["UUID-A".to_string()]);
                Ok(vec![FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: photos_asset.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                }])
            },
            |path| Ok(path != out),
        )?;

        assert_eq!(handle.imported_assets.len(), 1);
        assert_eq!(handle.imported_assets[0].rel_path, "nested/a.GIF");
        assert_eq!(handle.imported_assets[0].blake3, out_hash);
        assert_eq!(
            handle.imported_assets[0].photos_uuid.as_deref(),
            Some("UUID-A")
        );
        assert_eq!(handle.imported_assets[0].sync_status, "photos_local");
        assert!(!handle.imported_assets[0].quarantined);
        Ok(())
    }

    #[test]
    fn photos_import_preflight_rejects_candidate_hash_drift() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("candidate.webp");
        std::fs::write(&path, b"original").unwrap();
        let original_hash = crate::common_utils::calculate_blake3_hash(&path).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        let candidate = PhotosImportCandidate {
            rel_path: "candidate.webp".to_string(),
            path,
            blake3: original_hash,
            album_name: "tier2".to_string(),
        };

        let err = validate_photos_import_candidates(&[candidate])
            .expect_err("changed candidate must not reach Photos");
        assert!(
            err.to_string()
                .contains("Photos import preflight BLAKE3 mismatch"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn checkpointed_import_preflight_rejects_marker_output_hash_drift() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let working_copy = temp_dir.path().join("src_optimized");
        std::fs::create_dir_all(&working_copy).unwrap();
        let output = working_copy.join("a.AVIF");
        std::fs::write(&output, b"original").unwrap();
        let original_hash = crate::common_utils::calculate_blake3_hash(&output).unwrap();
        let mut marker =
            WorkingCopyMarker::new(src_root, working_copy, 1).with_strategy("avif".to_string());
        marker.blake3_log.insert(
            "a.png".to_string(),
            Blake3Entry {
                out_rel: Some("a.AVIF".to_string()),
                src: "source-hash".to_string(),
                out: original_hash,
                library_asset: None,
            },
        );
        std::fs::write(&output, b"replacement").unwrap();

        let err = validate_fast_img_marker_output_hashes(&marker)
            .expect_err("changed output must not reach Photos");
        assert!(
            err.to_string()
                .contains("Photos import preflight BLAKE3 mismatch"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn tier2_marker_proofs_merge_across_cleanup_resume() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let mut marker = WorkingCopyMarker::new(
            temp_dir.path().join("src"),
            temp_dir.path().join("src_optimized"),
            0,
        );
        let asset = |rel_path: &str, blake3: &str, uuid: &str| LibraryAssetRecord {
            rel_path: rel_path.to_string(),
            blake3: blake3.to_string(),
            sync_status: "photos_local".to_string(),
            quarantined: false,
            photos_uuid: Some(uuid.to_string()),
            library_blake3: None,
        };
        apply_tier2_library_assets_to_marker(
            &mut marker,
            &LibraryHandle {
                imported_assets: vec![asset("a.webp", "hash-a", "UUID-A")],
                import_error_count: 0,
            },
        )?;
        apply_tier2_library_assets_to_marker(
            &mut marker,
            &LibraryHandle {
                imported_assets: vec![
                    asset("a.webp", "hash-a-new", "UUID-A2"),
                    asset("b.jxl", "hash-b", "UUID-B"),
                ],
                import_error_count: 0,
            },
        )?;

        assert_eq!(marker.tier2_imported_assets.len(), 2);
        assert_eq!(marker.tier2_imported_assets[0].rel_path, "a.webp");
        assert_eq!(marker.tier2_imported_assets[0].blake3, "hash-a-new");
        assert_eq!(
            marker.tier2_imported_assets[0].photos_uuid.as_deref(),
            Some("UUID-A2")
        );
        assert_eq!(marker.tier2_imported_assets[1].rel_path, "b.jxl");
        Ok(())
    }

    #[test]
    fn generic_media_import_handle_records_library_blake3_after_pixel_proof() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let out = temp_dir.path().join("tier2/a.webp");
        let photos_asset = temp_dir.path().join("Photos Library.photoslibrary/a.webp");
        std::fs::create_dir_all(out.parent().expect("test output has parent")).unwrap();
        std::fs::create_dir_all(
            photos_asset
                .parent()
                .expect("test Photos asset path has parent"),
        )
        .unwrap();
        std::fs::write(&out, b"RIFF\x08\x00\x00\x00WEBPsource")?;
        std::fs::write(&photos_asset, b"RIFF\x08\x00\x00\x00WEBPlibrary")?;
        let out_hash = crate::common_utils::calculate_blake3_hash(&out)?;
        let library_hash = crate::common_utils::calculate_blake3_hash(&photos_asset)?;
        let candidates = vec![PhotosImportCandidate {
            rel_path: "a.webp".to_string(),
            path: out.clone(),
            blake3: out_hash.clone(),
            album_name: "tier2".to_string(),
        }];

        let mut verifier_called = false;
        let handle = library_handle_from_media_output_probes_with_pixel_verifier(
            &candidates,
            &[("a.webp".to_string(), "UUID-A".to_string())],
            |uuids| {
                assert_eq!(uuids, ["UUID-A".to_string()]);
                Ok(vec![FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: photos_asset.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                }])
            },
            |path| Ok(path != out),
            |source, library, fmt, tolerance| {
                verifier_called = true;
                assert_eq!(source, out.as_path());
                assert_eq!(library, photos_asset.as_path());
                assert_eq!(fmt, crate::image::format_detect::FormatKind::WebP);
                assert_eq!(tolerance, crate::image::orientation::DiffTolerance::LsbAvif);
                Ok(crate::image::orientation::PixelDiffResult::Match)
            },
        )?;

        assert!(verifier_called);
        assert_eq!(handle.imported_assets.len(), 1);
        assert_eq!(handle.imported_assets[0].blake3, out_hash);
        assert_eq!(
            handle.imported_assets[0].library_blake3.as_deref(),
            Some(library_hash.as_str())
        );
        assert_eq!(
            handle.imported_assets[0].photos_uuid.as_deref(),
            Some("UUID-A")
        );
        Ok(())
    }

    #[test]
    fn tier2_reconciliation_keeps_all_filename_matches_for_content_proof() -> Result<()> {
        let candidates = vec![
            PhotosImportCandidate {
                rel_path: "a.webp".to_string(),
                path: PathBuf::from("a.webp"),
                blake3: "a".to_string(),
                album_name: "album".to_string(),
            },
            PhotosImportCandidate {
                rel_path: "b.jxl".to_string(),
                path: PathBuf::from("b.jxl"),
                blake3: "b".to_string(),
                album_name: "album".to_string(),
            },
        ];
        assert_eq!(
            photos_reconciled_candidate_ids(&candidates, "UUID-WRONG|UUID-A\nMFB_NOT_FOUND\n")?,
            vec![
                vec!["UUID-WRONG".to_string(), "UUID-A".to_string()],
                Vec::<String>::new(),
            ]
        );
        assert!(photos_reconciled_candidate_ids(&candidates, "UUID-A\n").is_err());
        Ok(())
    }

    #[test]
    fn generic_media_import_handle_accepts_out_of_order_osxphotos_rows() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let out_a = temp_dir.path().join("a.webp");
        let out_b = temp_dir.path().join("b.webp");
        let lib_a = temp_dir.path().join("library/a.webp");
        let lib_b = temp_dir.path().join("library/b.webp");
        std::fs::create_dir_all(lib_a.parent().expect("parent")).unwrap();
        std::fs::write(&out_a, b"webp-a").unwrap();
        std::fs::write(&out_b, b"webp-b").unwrap();
        std::fs::write(&lib_a, b"webp-a").unwrap();
        std::fs::write(&lib_b, b"webp-b").unwrap();
        let hash_a = crate::common_utils::calculate_blake3_hash(&out_a)?;
        let hash_b = crate::common_utils::calculate_blake3_hash(&out_b)?;
        let candidates = vec![
            PhotosImportCandidate {
                rel_path: "a.webp".to_string(),
                path: out_a.clone(),
                blake3: hash_a,
                album_name: "✨tier2".to_string(),
            },
            PhotosImportCandidate {
                rel_path: "b.webp".to_string(),
                path: out_b.clone(),
                blake3: hash_b,
                album_name: "✨tier2".to_string(),
            },
        ];

        let handle = library_handle_from_media_output_probes(
            &candidates,
            &[
                ("a.webp".to_string(), "UUID-A".to_string()),
                ("b.webp".to_string(), "UUID-B".to_string()),
            ],
            |uuids| {
                assert_eq!(uuids, ["UUID-A".to_string(), "UUID-B".to_string()]);
                Ok(vec![
                    FastImgLibraryAssetProbe {
                        uuid: "UUID-B".to_string(),
                        path: lib_b.clone(),
                        iscloudasset: false,
                        incloud: Some(false),
                        ismissing: false,
                    },
                    FastImgLibraryAssetProbe {
                        uuid: "UUID-A".to_string(),
                        path: lib_a.clone(),
                        iscloudasset: false,
                        incloud: Some(false),
                        ismissing: false,
                    },
                ])
            },
            |path| Ok(path != out_a && path != out_b),
        )?;

        assert_eq!(handle.imported_assets.len(), 2);
        assert_eq!(
            handle.imported_assets[0].photos_uuid.as_deref(),
            Some("UUID-A")
        );
        assert_eq!(
            handle.imported_assets[1].photos_uuid.as_deref(),
            Some("UUID-B")
        );
        Ok(())
    }

    #[test]
    fn generic_media_import_batches_candidates_and_fails_closed() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let mut candidates = Vec::new();
        for index in 0..11 {
            let name = format!("f{index:02}");
            let path = temp_dir.path().join(format!("{name}.AVIF"));
            std::fs::write(&path, name.as_bytes()).unwrap();
            candidates.push(PhotosImportCandidate {
                rel_path: format!("{name}.AVIF"),
                blake3: crate::common_utils::calculate_blake3_hash(&path)?,
                path,
                album_name: "✨media".to_string(),
            });
        }

        let mut normal_batch_sizes = Vec::new();
        let report = import_media_outputs_with_photos_applescript_with(
            &candidates,
            false,
            &mut |manifest_entries: &[(PathBuf, String)]| {
                normal_batch_sizes.push(manifest_entries.len());
                Ok(manifest_entries
                    .iter()
                    .map(|(path, _)| {
                        format!(
                            "UUID-{}",
                            path.file_stem()
                                .and_then(|stem| stem.to_str())
                                .expect("test path has UTF-8 stem")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            },
        )?;
        assert_eq!(normal_batch_sizes, [10, 1]);
        assert_eq!(report.failed_count, 0);
        assert_eq!(
            report.report_pairs.first(),
            Some(&("f00.AVIF".to_string(), "UUID-f00".to_string()))
        );
        assert_eq!(
            report.report_pairs.last(),
            Some(&("f10.AVIF".to_string(), "UUID-f10".to_string()))
        );

        let mut normal_failure_batch_sizes = Vec::new();
        let report = import_media_outputs_with_photos_applescript_with(
            &candidates,
            false,
            &mut |manifest_entries: &[(PathBuf, String)]| {
                normal_failure_batch_sizes.push(manifest_entries.len());
                Ok(String::new())
            },
        )?;
        assert_eq!(
            normal_failure_batch_sizes,
            [10, 10, 10, 10, 10, 1, 1, 1, 1, 1]
        );
        assert_eq!(report.failed_count, 11);
        assert_eq!(report.report_pairs.len(), 0);

        let mut fail_fast_calls = 0usize;
        let err = import_media_outputs_with_photos_applescript_with(
            &candidates,
            true,
            &mut |manifest_entries: &[(PathBuf, String)]| {
                fail_fast_calls += 1;
                assert_eq!(manifest_entries.len(), 10);
                Err(ImgQualityError::AnalysisError(
                    "Photos returned 0 imported items".to_string(),
                ))
            },
        )
        .unwrap_err();
        assert_eq!(fail_fast_calls, 1);
        assert!(err.to_string().contains("Photos returned 0 imported items"));
        Ok(())
    }

    #[test]
    fn generic_media_import_recovers_poisoned_photos_session_once() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("a.AVIF");
        std::fs::write(&path, b"avif-a").unwrap();
        let candidates = [PhotosImportCandidate {
            rel_path: "a.AVIF".to_string(),
            blake3: crate::common_utils::calculate_blake3_hash(&path)?,
            path,
            album_name: "✨media".to_string(),
        }];
        let mut attempts = 0usize;

        let report = import_media_outputs_with_photos_applescript_with(
            &candidates,
            false,
            &mut |_| {
                attempts += 1;
                if attempts == 1 {
                    Err(ImgQualityError::AnalysisError(
                        "Photos AppleScript media import chunk 1/1 failed: timed out at hard timeout after 120s"
                            .to_string(),
                    ))
                } else {
                    Ok("UUID-a\n".to_string())
                }
            },
        )?;

        assert_eq!(attempts, 2);
        assert_eq!(report.failed_count, 0);
        assert_eq!(
            report.report_pairs,
            [("a.AVIF".to_string(), "UUID-a".to_string())]
        );
        Ok(())
    }

    #[test]
    fn generic_media_import_aggressively_retries_unknown_photos_errors() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("a.AVIF");
        std::fs::write(&path, b"avif-a").unwrap();
        let candidates = [PhotosImportCandidate {
            rel_path: "a.AVIF".to_string(),
            blake3: crate::common_utils::calculate_blake3_hash(&path)?,
            path,
            album_name: "✨media".to_string(),
        }];
        let mut attempts = 0usize;

        let report =
            import_media_outputs_with_photos_applescript_with(&candidates, false, &mut |_| {
                attempts += 1;
                if attempts < 5 {
                    Err(ImgQualityError::AnalysisError(
                        "Photos import failed: unknown error".to_string(),
                    ))
                } else {
                    Ok("UUID-a\n".to_string())
                }
            })?;

        assert_eq!(attempts, 5);
        assert_eq!(report.failed_count, 0);
        assert_eq!(
            report.report_pairs,
            [("a.AVIF".to_string(), "UUID-a".to_string())]
        );
        Ok(())
    }

    #[test]
    fn generic_media_import_does_not_retry_permanent_photos_errors() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("a.AVIF");
        std::fs::write(&path, b"avif-a").unwrap();
        let candidates = [PhotosImportCandidate {
            rel_path: "a.AVIF".to_string(),
            blake3: crate::common_utils::calculate_blake3_hash(&path)?,
            path,
            album_name: "✨media".to_string(),
        }];
        let mut attempts = 0usize;

        let err =
            import_media_outputs_with_photos_applescript_with(&candidates, false, &mut |_| {
                attempts += 1;
                Err(ImgQualityError::AnalysisError(
                    "Photos import failed: not authorized (-1743)".to_string(),
                ))
            })
            .unwrap_err();

        assert_eq!(attempts, 1);
        assert!(err.to_string().contains("not authorized"));
        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[serial_test::serial]
    fn photos_system_commands_ignore_path_override() {
        let _path_guard = crate::common_utils::EnvGuard::set("PATH", "/tmp/untrusted");
        assert_eq!(
            resolve_osascript_command(),
            PathBuf::from(MACOS_OSASCRIPT_PATH)
        );
        for path in [
            MACOS_OSASCRIPT_PATH,
            MACOS_PS_PATH,
            MACOS_VM_STAT_PATH,
            MACOS_PGREP_PATH,
            MACOS_OPEN_PATH,
            MACOS_KILLALL_PATH,
            MACOS_XATTR_PATH,
        ] {
            assert!(
                Path::new(path).is_absolute(),
                "system tool is not absolute: {path}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn fast_img_command_timeout_terminates_hung_child() {
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("30");
        let started = std::time::Instant::now();
        let err = run_fast_img_command_with_timeout(
            &mut command,
            Duration::from_millis(20),
            "fast-img timeout regression",
        )
        .unwrap_err();
        assert!(err.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    #[serial_test::serial]
    fn photos_import_canonical_error_mode_is_explicit_and_fail_closed() {
        {
            let _guard =
                crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_ERROR_MODE, "debug");
            assert!(photos_import_fail_fast_enabled());
        }
        {
            let _guard = crate::common_utils::EnvGuard::set(
                crate::constants::ENV_MFB_ERROR_MODE,
                "log-and-continue",
            );
            assert!(!photos_import_fail_fast_enabled());
        }
        {
            let _guard = crate::common_utils::EnvGuard::set(
                crate::constants::ENV_MFB_ERROR_MODE,
                "unknown-mode",
            );
            assert!(photos_import_fail_fast_enabled());
        }
        {
            let _guard = crate::common_utils::EnvGuard::set(
                crate::constants::ENV_MFB_DRAG_DROP_ERROR_MODE,
                "debug",
            );
            assert!(photos_import_fail_fast_enabled());
        }
    }

    #[test]
    fn index_photos_probes_by_uuid_fails_when_query_incomplete() {
        let err = index_photos_probes_by_uuid(
            &["UUID-A".to_string(), "UUID-B".to_string()],
            vec![FastImgLibraryAssetProbe {
                uuid: "UUID-A".to_string(),
                path: PathBuf::from("/tmp/a"),
                iscloudasset: false,
                incloud: None,
                ismissing: false,
            }],
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn photos_import_script_batches_inside_single_session() {
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("is greater than or equal to batchSize"),
            "Photos import script must flush batches by size inside one osascript session"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("read (POSIX file manifestPath)"),
            "Photos import script must read path/album pairs from a manifest file"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("delay (batchDelayMs / 1000)"),
            "Photos import script must pace batches inside the session, not between processes"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("digestPauseInterval"),
            "Photos import script must accept digest pause interval parameter"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("if (batchNumber mod digestPauseInterval)"),
            "Photos import script must insert digest pauses every N batches"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT
                .contains("first album whose name is targetAlbumName"),
            "large Photos libraries require filtered album lookup"
        );
        assert!(
            !FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("repeat with candidateAlbum in albums"),
            "per-file imports must not rescan every Photos album"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("with timeout of hardTimeoutSecs seconds")
        );
        assert!(FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("operationMode is \"reconcile\""));
        assert!(FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("mfbFindExistingImportId"));
        assert!(FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("whose filename is expectedFilename"));
        assert!(!FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("86400"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn photos_import_applescript_compiles() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let source = temp_dir.path().join("photos-import.applescript");
        let output = temp_dir.path().join("photos-import.scpt");
        std::fs::write(&source, FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT).unwrap();
        let compiled = std::process::Command::new("/usr/bin/osacompile")
            .args(["-o"])
            .arg(&output)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "Photos AppleScript syntax error: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
    }

    #[test]
    fn photos_import_soft_estimate_scales_but_stays_below_hard_deadline() {
        let one_batch = photos_import_session_timeout(1).unwrap();
        let ten_batches = photos_import_session_timeout(10).unwrap();

        assert!(ten_batches > one_batch);
        assert!(ten_batches < crate::process_runner::image_process_hard_timeout());
    }

    #[test]
    fn photos_import_script_coerces_posix_file_outside_photos_tell() {
        let posix_index = FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT
            .find("set importPath to POSIX file")
            .unwrap();
        let import_tell_index = FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT
            .find("tell application \"Photos\"\n                set importedItems")
            .unwrap();
        let append_index = FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT
            .find("set end of fileList to importPath")
            .unwrap();

        assert!(posix_index < append_index);
        assert!(append_index < import_tell_index);
    }

    #[test]
    fn photos_import_script_batches_file_list_per_album() {
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("set fileList to {}"),
            "Photos import must build a multi-file list per batch"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("set end of fileList to importPath"),
            "Photos import must append each batch path before one import transaction"
        );
        assert!(
            !FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("set fileList to {importPath}"),
            "Photos import must not issue one import transaction per file"
        );
    }

    #[test]
    fn photos_import_script_preserves_nested_album_paths() {
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT.contains("mfbEnsureChildFolder"),
            "Photos import must create nested folders instead of flattening album paths"
        );
        assert!(
            FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT
                .contains("repeat with pathIndex from 2 to ((count of pathItems) - 1)"),
            "Photos import must walk all intermediate path components"
        );
        assert!(
            !FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT
                .contains("set targetAlbumName to item 2 of pathItems"),
            "Photos import must not truncate album paths after the second component"
        );
    }

    #[test]
    fn photos_import_windows_force_periodic_relaunch_before_poison_threshold() {
        let windows = photos_import_windows(100, 10, 25).unwrap();

        assert_eq!(windows.len(), 10);
        assert!(
            windows.iter().all(|window| window.len <= 10),
            "each osascript window must stay short"
        );
        assert_eq!(
            windows
                .iter()
                .filter(|window| window.relaunch_photos_before)
                .map(|window| window.start)
                .collect::<Vec<_>>(),
            vec![20, 40, 60, 80]
        );
    }

    #[test]
    fn photos_import_pending_entries_skip_valid_checkpointed_marker_proofs() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let proven = wc.join("a.JXL");
        let pending = wc.join("b.JXL");
        std::fs::write(&proven, b"proven-jxl").unwrap();
        std::fs::write(&pending, b"pending-jxl").unwrap();
        let proven_hash = crate::common_utils::calculate_blake3_hash(&proven)?;
        let pending_hash = crate::common_utils::calculate_blake3_hash(&pending)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 2);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src-a".to_string(),
                out: proven_hash.clone(),
                library_asset: Some(proven_hash.clone()),
            },
        );
        marker.photos_imported_assets.push(LibraryAssetRecord {
            rel_path: "a.JXL".to_string(),
            blake3: proven_hash,
            sync_status: "photos_local".to_string(),
            quarantined: false,
            photos_uuid: Some("UUID-A/L0/001".to_string()),
            library_blake3: None,
        });
        marker.blake3_log.insert(
            "b.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("b.JXL".to_string()),
                src: "src-b".to_string(),
                out: pending_hash,
                library_asset: None,
            },
        );

        let plan = photos_import_checkpoint_plan(&marker, |_| Ok(false))?;

        assert_eq!(plan.proven_assets.len(), 1);
        assert_eq!(plan.proven_assets[0].rel_path, "a.JXL");
        assert_eq!(plan.pending_entries.len(), 1);
        assert_eq!(plan.pending_entries[0].source_rel, "b.jpg");
        assert_eq!(plan.pending_entries[0].rel_path, "b.JXL");
        assert_eq!(plan.pending_entries[0].path, pending);
        assert_eq!(plan.pending_entries[0].album_name, "✨/✨Batch");
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_resume_reconciles_uncheckpointed_asset_before_reimport() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().to_str().unwrap(),
        );
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let output = wc.join("a.AVIF");
        std::fs::write(&output, b"avif-a").unwrap();
        let hash = crate::common_utils::calculate_blake3_hash(&output)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.AVIF".to_string()),
                src: "src-a".to_string(),
                out: hash.clone(),
                library_asset: None,
            },
        );
        let mut is_quarantined = |_path: &Path| Ok(false);
        let pending = photos_import_checkpoint_plan(&marker, &mut is_quarantined)?.pending_entries;
        let mut import_called = false;
        let recovered = reconcile_uncheckpointed_photos_assets(
            &mut marker,
            &pending,
            &mut |uuids: &[String]| {
                assert_eq!(uuids, ["UUID-A"]);
                Ok(vec![FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: output.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                }])
            },
            &mut is_quarantined,
            &mut |manifest_entries: &[(PathBuf, String)]| {
                import_called = true;
                assert_eq!(manifest_entries.len(), 1);
                Ok("UUID-A/L0/001\n".to_string())
            },
        )?;

        assert!(import_called);
        assert_eq!(recovered, 1);
        assert!(
            photos_import_checkpoint_plan(&marker, &mut is_quarantined)?
                .pending_entries
                .is_empty()
        );
        assert_eq!(marker.photos_imported_assets.len(), 1);
        assert_eq!(
            marker
                .blake3_log
                .get("a.jpg")
                .and_then(|entry| entry.library_asset.as_deref()),
            Some(hash.as_str())
        );
        Ok(())
    }

    #[test]
    fn photos_resume_reverifies_persisted_uuid_before_skipping() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let output = wc.join("a.AVIF");
        std::fs::write(&output, b"avif-a").unwrap();
        let hash = crate::common_utils::calculate_blake3_hash(&output)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.AVIF".to_string()),
                src: "src-a".to_string(),
                out: hash.clone(),
                library_asset: Some(hash.clone()),
            },
        );
        marker.photos_imported_assets.push(LibraryAssetRecord {
            rel_path: "a.AVIF".to_string(),
            blake3: hash,
            sync_status: "photos_local".to_string(),
            quarantined: false,
            photos_uuid: Some("UUID-A/L0/001".to_string()),
            library_blake3: None,
        });
        let mut is_quarantined = |_path: &Path| Ok(false);
        let plan = photos_import_checkpoint_plan(&marker, &mut is_quarantined)?;
        let mut query_calls = 0usize;

        reverify_checkpointed_photos_assets(
            &marker,
            &plan.proven_assets,
            &mut |uuids: &[String]| {
                query_calls += 1;
                assert_eq!(uuids, ["UUID-A"]);
                Ok(vec![FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: output.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                }])
            },
            &mut is_quarantined,
        )?;

        assert_eq!(query_calls, 1);
        Ok(())
    }

    #[test]
    fn persisted_photos_resume_proof_requires_uuid() {
        let mut asset = LibraryAssetRecord {
            rel_path: "a.JXL".to_string(),
            blake3: "hash".to_string(),
            sync_status: "photos_local".to_string(),
            quarantined: false,
            photos_uuid: None,
            library_blake3: None,
        };
        assert!(
            photos_import_report_pairs_from_persisted_assets(std::slice::from_ref(&asset)).is_err()
        );

        asset.photos_uuid = Some("UUID-A/L0/001".to_string());
        assert_eq!(
            photos_import_report_pairs_from_persisted_assets(&[asset]).unwrap(),
            [("a.JXL".to_string(), "UUID-A/L0/001".to_string())]
        );
    }

    #[test]
    #[serial_test::serial]
    fn photos_import_failed_window_leaves_entries_pending_without_partial_checkpoint() -> Result<()>
    {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().to_str().unwrap(),
        );
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let total = FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP + 1;
        let mut marker = WorkingCopyMarker::new(src_root, wc.clone(), total);
        for idx in 0..total {
            let name = format!("f{idx:02}");
            let source_rel = format!("{name}.jpg");
            let rel_path = format!("{name}.JXL");
            let path = wc.join(&rel_path);
            std::fs::write(&path, name.as_bytes()).unwrap();
            let out = crate::common_utils::calculate_blake3_hash(&path)?;
            marker.blake3_log.insert(
                source_rel,
                Blake3Entry {
                    out_rel: Some(rel_path.clone()),
                    src: "src".to_string(),
                    out: out.clone(),
                    library_asset: None,
                },
            );
        }

        let mut import_calls = 0usize;
        let mut run_import_batch = |batch_entries: &[(PathBuf, String)]| -> Result<String> {
            if batch_entries.len() != 1 {
                return Err(ImgQualityError::AnalysisError(format!(
                    "expected one-file Photos import transaction, got {}",
                    batch_entries.len()
                )));
            }
            import_calls = import_calls.checked_add(1).ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "Photos import regression-test counter overflowed".to_string(),
                )
            })?;
            if import_calls == 1 {
                Ok("UUID-f00\n".to_string())
            } else {
                Ok(String::new())
            }
        };
        let mut query_assets = |uuids: &[String]| {
            uuids
                .iter()
                .map(|uuid| {
                    let path = wc.join(format!("{}.JXL", uuid.trim_start_matches("UUID-")));
                    Ok(FastImgLibraryAssetProbe {
                        uuid: uuid.clone(),
                        path,
                        iscloudasset: false,
                        incloud: Some(false),
                        ismissing: false,
                    })
                })
                .collect::<Result<Vec<_>>>()
        };
        let mut is_quarantined = |_path: &Path| Ok(false);
        let mut checkpoint_marker = marker.clone();
        let pending =
            photos_import_checkpoint_plan(&checkpoint_marker, &mut is_quarantined)?.pending_entries;
        let err = import_pending_media_entries_with_checkpoint(
            &mut checkpoint_marker,
            &pending,
            true,
            &mut query_assets,
            &mut is_quarantined,
            &mut |_reason: &str| Ok(()),
            &mut run_import_batch,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("Photos AppleScript import returned 0 IDs for 1 JXL outputs"),
            "unexpected err: {err}"
        );
        assert!(
            checkpoint_marker
                .blake3_log
                .get("f00.jpg")
                .and_then(|entry| entry.library_asset.as_ref())
                .is_some(),
            "the completed one-file transaction must be checkpointed"
        );
        for idx in 1..total {
            let key = format!("f{idx:02}.jpg");
            assert!(
                checkpoint_marker
                    .blake3_log
                    .get(&key)
                    .and_then(|entry| entry.library_asset.as_ref())
                    .is_none(),
                "{key} should remain pending after the next one-file transaction failed"
            );
        }
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_import_normal_mode_continues_after_one_unverified_file() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().to_str().unwrap(),
        );
        let _verify_delay_guard =
            crate::common_utils::EnvGuard::set(FAST_IMG_ICLOUD_VERIFY_DELAY_MS_ENV, "0");
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let mut marker = WorkingCopyMarker::new(src_root, wc.clone(), 3);
        for name in ["a", "b", "c"] {
            let rel_path = format!("{name}.JXL");
            let path = wc.join(&rel_path);
            std::fs::write(&path, name.as_bytes()).unwrap();
            marker.blake3_log.insert(
                format!("{name}.jpg"),
                Blake3Entry {
                    out_rel: Some(rel_path),
                    src: format!("src-{name}"),
                    out: crate::common_utils::calculate_blake3_hash(&path)?,
                    library_asset: None,
                },
            );
        }

        let mut import_calls = Vec::new();
        let mut run_import_batch = |batch_entries: &[(PathBuf, String)]| -> Result<String> {
            let stem = batch_entries[0]
                .0
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError("missing test file stem".to_string())
                })?
                .to_string();
            import_calls.push(stem.clone());
            Ok(format!("UUID-{stem}\n"))
        };
        let mut query_assets = |uuids: &[String]| {
            uuids
                .iter()
                .filter(|uuid| uuid.as_str() != "UUID-b")
                .map(|uuid| {
                    let stem = uuid.trim_start_matches("UUID-");
                    Ok(FastImgLibraryAssetProbe {
                        uuid: uuid.clone(),
                        path: wc.join(format!("{stem}.JXL")),
                        iscloudasset: false,
                        incloud: Some(false),
                        ismissing: false,
                    })
                })
                .collect::<Result<Vec<_>>>()
        };
        let mut is_quarantined = |_path: &Path| Ok(false);
        let pending = photos_import_checkpoint_plan(&marker, &mut is_quarantined)?.pending_entries;
        let report = import_pending_media_entries_with_checkpoint(
            &mut marker,
            &pending,
            false,
            &mut query_assets,
            &mut is_quarantined,
            &mut |_reason: &str| Ok(()),
            &mut run_import_batch,
        )?;

        assert_eq!(import_calls, ["a", "b", "b", "b", "b", "b", "c"]);
        assert_eq!(report.imported_assets.len(), 2);
        assert_eq!(report.failed_count, 1);
        assert!(
            marker
                .blake3_log
                .get("a.jpg")
                .and_then(|entry| entry.library_asset.as_ref())
                .is_some()
        );
        assert!(
            marker
                .blake3_log
                .get("b.jpg")
                .and_then(|entry| entry.library_asset.as_ref())
                .is_none()
        );
        assert!(
            marker
                .blake3_log
                .get("c.jpg")
                .and_then(|entry| entry.library_asset.as_ref())
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn photos_import_rejects_scrambled_library_bytes_before_checkpoint() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().to_str().unwrap(),
        );
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        let library_dir = temp_dir.path().join("Photos Library.photoslibrary");
        std::fs::create_dir_all(&wc).unwrap();
        std::fs::create_dir_all(&library_dir).unwrap();
        let output = wc.join("a.JXL");
        let library_asset = library_dir.join("a.jxl");
        std::fs::write(&output, b"expected-jxl-bytes").unwrap();
        std::fs::write(&library_asset, b"different-library-bytes").unwrap();
        let output_hash = crate::common_utils::calculate_blake3_hash(&output)?;
        let library_hash = crate::common_utils::calculate_blake3_hash(&library_asset)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: output_hash,
                library_asset: None,
            },
        );

        let mut query_assets = |uuids: &[String]| {
            assert_eq!(uuids, ["UUID-A".to_string()]);
            Ok(vec![FastImgLibraryAssetProbe {
                uuid: "UUID-A".to_string(),
                path: library_asset.clone(),
                iscloudasset: false,
                incloud: Some(false),
                ismissing: false,
            }])
        };
        let mut is_quarantined = |_path: &Path| Ok(false);
        let pending = photos_import_checkpoint_plan(&marker, &mut is_quarantined)?.pending_entries;
        let err = import_pending_media_entries_with_checkpoint(
            &mut marker,
            &pending,
            false,
            &mut query_assets,
            &mut is_quarantined,
            &mut |_reason: &str| Ok(()),
            &mut |_batch_entries: &[(PathBuf, String)]| Ok("UUID-A\n".to_string()),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("Photos verifier BLAKE3 mismatch for a.JXL"),
            "unexpected err: {err}"
        );
        assert!(
            err.to_string().contains(&library_hash),
            "mismatch error must include library hash: {err}"
        );
        assert!(
            marker
                .blake3_log
                .get("a.jpg")
                .and_then(|entry| entry.library_asset.as_ref())
                .is_none(),
            "scrambled library bytes must not be checkpointed as proof"
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_import_one_file_transactions_bind_each_checkpoint_to_its_identifier() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().to_str().unwrap(),
        );
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        let library_dir = temp_dir.path().join("Photos Library.photoslibrary");
        std::fs::create_dir_all(&wc).unwrap();
        std::fs::create_dir_all(&library_dir).unwrap();
        let out_a = wc.join("a.JXL");
        let out_b = wc.join("b.JXL");
        let library_a = library_dir.join("A.jxl");
        let library_b = library_dir.join("B.jxl");
        std::fs::write(&out_a, b"asset-a").unwrap();
        std::fs::write(&out_b, b"asset-b").unwrap();
        std::fs::write(&library_a, b"asset-a").unwrap();
        std::fs::write(&library_b, b"asset-b").unwrap();
        let hash_a = crate::common_utils::calculate_blake3_hash(&out_a)?;
        let hash_b = crate::common_utils::calculate_blake3_hash(&out_b)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 2);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src-a".to_string(),
                out: hash_a.clone(),
                library_asset: None,
            },
        );
        marker.blake3_log.insert(
            "b.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("b.JXL".to_string()),
                src: "src-b".to_string(),
                out: hash_b.clone(),
                library_asset: None,
            },
        );

        let mut query_assets = |uuids: &[String]| {
            assert_eq!(uuids.len(), 1);
            let (uuid, path) = match uuids[0].as_str() {
                "UUID-A" => ("UUID-A", library_a.clone()),
                "UUID-B" => ("UUID-B", library_b.clone()),
                other => {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "unexpected test UUID: {other}"
                    )));
                }
            };
            Ok(vec![FastImgLibraryAssetProbe {
                uuid: uuid.to_string(),
                path,
                iscloudasset: false,
                incloud: Some(false),
                ismissing: false,
            }])
        };
        let mut is_quarantined = |_path: &Path| Ok(false);
        let pending = photos_import_checkpoint_plan(&marker, &mut is_quarantined)?.pending_entries;
        let report = import_pending_media_entries_with_checkpoint(
            &mut marker,
            &pending,
            false,
            &mut query_assets,
            &mut is_quarantined,
            &mut |_reason: &str| Ok(()),
            &mut |batch_entries: &[(PathBuf, String)]| {
                assert_eq!(batch_entries.len(), 1);
                let stem = batch_entries[0]
                    .0
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "test import path is missing a UTF-8 stem".to_string(),
                        )
                    })?;
                Ok(format!("UUID-{}\n", stem.to_ascii_uppercase()))
            },
        )?;
        let records = &report.imported_assets;

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].rel_path, "a.JXL");
        assert_eq!(records[0].blake3, hash_a);
        assert_eq!(records[0].photos_uuid.as_deref(), Some("UUID-A"));
        assert_eq!(records[1].rel_path, "b.JXL");
        assert_eq!(records[1].blake3, hash_b);
        assert_eq!(records[1].photos_uuid.as_deref(), Some("UUID-B"));
        assert_eq!(
            marker
                .blake3_log
                .get("a.jpg")
                .and_then(|entry| entry.library_asset.as_ref()),
            Some(&records[0].blake3)
        );
        assert_eq!(
            marker
                .blake3_log
                .get("b.jpg")
                .and_then(|entry| entry.library_asset.as_ref()),
            Some(&records[1].blake3)
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_import_fast_path_skips_initial_warmup() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            temp_dir.path().to_str().unwrap(),
        );
        let src_root = temp_dir.path().join("src");
        let wc = temp_dir.path().join("Batch_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let total = FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP;
        let mut marker = WorkingCopyMarker::new(src_root, wc.clone(), total);
        for idx in 0..total {
            let rel_path = format!("f{idx:03}.JXL");
            let path = wc.join(&rel_path);
            std::fs::write(&path, rel_path.as_bytes()).unwrap();
            let out = crate::common_utils::calculate_blake3_hash(&path)?;
            marker.blake3_log.insert(
                format!("f{idx:03}.jpg"),
                Blake3Entry {
                    out_rel: Some(rel_path),
                    src: "src".to_string(),
                    out,
                    library_asset: None,
                },
            );
        }

        let mut prepare_calls = Vec::new();
        let mut prepare_import_session = |reason: &str| -> Result<()> {
            prepare_calls.push(reason.to_string());
            Ok(())
        };
        let mut run_batch_sizes = Vec::new();
        let mut run_import_batch = |batch_entries: &[(PathBuf, String)]| -> Result<String> {
            run_batch_sizes.push(batch_entries.len());
            Ok(batch_entries
                .iter()
                .map(|(path, _)| {
                    format!(
                        "UUID-{}",
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .expect("test path has UTF-8 stem")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        };
        let mut query_assets = |uuids: &[String]| {
            uuids
                .iter()
                .map(|uuid| {
                    let stem = uuid.trim_start_matches("UUID-");
                    Ok(FastImgLibraryAssetProbe {
                        uuid: uuid.clone(),
                        path: wc.join(format!("{stem}.JXL")),
                        iscloudasset: false,
                        incloud: Some(false),
                        ismissing: false,
                    })
                })
                .collect::<Result<Vec<_>>>()
        };
        let mut is_quarantined = |_path: &Path| Ok(false);
        let pending = photos_import_checkpoint_plan(&marker, &mut is_quarantined)?.pending_entries;
        let report = import_pending_media_entries_with_checkpoint(
            &mut marker,
            &pending,
            false,
            &mut query_assets,
            &mut is_quarantined,
            &mut prepare_import_session,
            &mut run_import_batch,
        )?;

        assert!(
            prepare_calls.is_empty(),
            "small pending set must avoid relaunch warmup overhead"
        );
        assert_eq!(run_batch_sizes.len(), total);
        assert!(run_batch_sizes.iter().all(|batch_size| *batch_size == 1));
        assert_eq!(report.imported_assets.len(), total);
        assert_eq!(report.failed_count, 0);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_import_stable_path_keeps_warmup_and_windowed_batches() -> Result<()> {
        let strategy = photos_import_strategy(FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP + 1);
        let windows = photos_import_windows(
            FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP + 1,
            photos_import_strategy_window_file_cap(strategy),
            FAST_IMG_PHOTOS_IMPORT_RELAUNCH_INTERVAL_FILES,
        )?;

        assert_eq!(strategy, PhotosImportStrategy::StableCheckpointed);
        assert!(photos_import_strategy_requires_initial_warmup(strategy));
        assert_eq!(windows.len(), 2);
        assert_eq!(
            windows.iter().map(|window| window.len).collect::<Vec<_>>(),
            vec![FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP, 51]
        );
        let batch_sizes = photos_import_batch_sizes(windows[0].len);
        assert_eq!(batch_sizes.len(), FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP);
        assert!(batch_sizes.iter().all(|batch_size| *batch_size == 1));
        Ok(())
    }

    #[test]
    fn photos_import_relaunches_session_before_first_batch_even_when_running() -> Result<()> {
        let mut pid_checks = 0usize;
        let mut relaunch_reasons = Vec::new();

        ensure_photos_import_session_ready(
            "initial_import_warmup",
            || {
                pid_checks = pid_checks.checked_add(1).ok_or_else(|| {
                    ImgQualityError::AnalysisError(
                        "Photos import test pid check counter overflowed".to_string(),
                    )
                })?;
                Ok(Some("12345".to_string()))
            },
            |reason| {
                relaunch_reasons.push(reason.to_string());
                Ok(())
            },
        )?;

        assert_eq!(pid_checks, 1);
        assert_eq!(relaunch_reasons, vec!["initial_import_warmup"]);
        Ok(())
    }

    #[test]
    fn periodic_photos_recovery_continues_only_after_functional_health_proof() {
        let mut recovery_calls = Vec::new();
        let mut health_probe_calls = 0usize;
        let result = handle_photos_import_recovery(
            "periodic_window_boundary",
            &mut |reason: &str| {
                recovery_calls.push(reason.to_string());
                Err(ImgQualityError::AnalysisError(
                    "timed out waiting for Photos process quit state".to_string(),
                ))
            },
            &mut || {
                health_probe_calls += 1;
                Ok(())
            },
        );

        assert!(
            result.is_ok(),
            "a functional Photos health proof may preserve the current import window"
        );
        assert_eq!(recovery_calls, vec!["periodic_window_boundary"]);
        assert_eq!(health_probe_calls, 1);
    }

    #[test]
    fn periodic_photos_recovery_fails_when_functional_health_probe_fails() {
        let result = handle_photos_import_recovery(
            "periodic_window_boundary",
            &mut |_reason: &str| {
                Err(ImgQualityError::AnalysisError(
                    "timed out waiting for Photos process quit state".to_string(),
                ))
            },
            &mut || {
                Err(ImgQualityError::AnalysisError(
                    "AppleEvent probe timed out".to_string(),
                ))
            },
        );

        let err = result.expect_err("unproven Photos session health must fail closed");
        let detail = err.to_string();
        assert!(
            detail.contains("timed out waiting for Photos process quit state")
                && detail.contains("AppleEvent probe timed out"),
            "both recovery and health-probe failures must remain visible: {err}"
        );
    }

    #[test]
    fn failed_graceful_quit_still_reaches_force_kill_recovery() {
        let mut waited_for_quit = false;
        let result = complete_photos_quit_recovery(
            "poisoned_session",
            Err(ImgQualityError::AnalysisError(
                "Photos recovery quit command timed out".to_string(),
            )),
            &mut || {
                waited_for_quit = true;
                Ok(())
            },
        );

        assert!(
            result.is_ok(),
            "quit timeout must fall through to process recovery"
        );
        assert!(
            waited_for_quit,
            "process recovery must get a chance to force-quit Photos"
        );
    }

    #[test]
    fn poisoned_photos_recovery_timeout_remains_fatal() {
        let mut health_probe_called = false;
        let result = handle_photos_import_recovery(
            "poisoned_session",
            &mut |_reason: &str| {
                Err(ImgQualityError::AnalysisError(
                    "timed out waiting for Photos process quit state".to_string(),
                ))
            },
            &mut || {
                health_probe_called = true;
                Ok(())
            },
        );

        let err = result.expect_err("poisoned Photos session recovery must fail closed");
        assert!(
            err.to_string()
                .contains("timed out waiting for Photos process quit state"),
            "unexpected err: {err}"
        );
        assert!(
            !health_probe_called,
            "poisoned sessions must not be rescued by the periodic-boundary health probe"
        );
    }

    #[test]
    fn photos_import_batch_sizes_use_one_file_transactions() {
        for total in [
            0,
            1,
            2,
            11,
            21,
            FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP,
            FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP + 1,
        ] {
            assert_eq!(
                photos_import_batch_sizes(total),
                vec![1; total],
                "every Photos import path must checkpoint one file at a time"
            );
        }
    }

    #[test]
    #[ignore = "writes to the macOS System Photo Library; explicit consent required"]
    #[cfg(target_os = "macos")]
    #[serial_test::serial]
    fn photos_import_live_smoke_system_library_requires_explicit_consent() -> Result<()> {
        if std::env::var("MFB_LIVE_PHOTOS_SMOKE_ALLOW_SYSTEM_LIBRARY").as_deref() != Ok("1") {
            return Err(ImgQualityError::AnalysisError(
                "live Photos smoke writes to the System Photo Library; set \
                 MFB_LIVE_PHOTOS_SMOKE_ALLOW_SYSTEM_LIBRARY=1 to confirm"
                    .to_string(),
            ));
        }
        let input = std::env::var_os("MFB_LIVE_PHOTOS_SMOKE_INPUT")
            .map(PathBuf::from)
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "MFB_LIVE_PHOTOS_SMOKE_INPUT must name an explicit synthetic fixture"
                        .to_string(),
                )
            })?;

        if !input.exists() {
            return Err(ImgQualityError::AnalysisError(format!(
                "live Photos smoke input missing: {}",
                input.display()
            )));
        }
        let input_name = input
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "live Photos smoke input has no UTF-8 file name: {}",
                    input.display()
                ))
            })?;
        if !input_name.starts_with("mfb-photos-smoke-") {
            return Err(ImgQualityError::AnalysisError(format!(
                "live Photos smoke input must use the mfb-photos-smoke-* synthetic-fixture prefix: {}",
                input.display()
            )));
        }
        prepare_photos_import_session("live_system_library_smoke")?;

        let output = run_photos_import_applescript_session(
            "live system-library smoke",
            &[(input.clone(), "✨system-library-smoke".to_string())],
        )?;
        let rel_path = input_name.to_string();
        let report_pairs = fast_img_pairs_from_photos_import_ids(
            &[(rel_path.clone(), input.clone())],
            output.as_bytes(),
            1,
        )?;
        let candidates = [PhotosImportCandidate {
            rel_path: rel_path.clone(),
            path: input.clone(),
            blake3: crate::common_utils::calculate_blake3_hash(&input)?,
            album_name: "✨system-library-smoke".to_string(),
        }];
        let handle = library_handle_from_media_output_probes(
            &candidates,
            &report_pairs,
            query_osxphotos_asset_probes,
            path_has_quarantine_xattr,
        )?;

        assert_eq!(handle.imported_assets.len(), 1);
        assert!(
            handle.imported_assets[0]
                .photos_uuid
                .as_deref()
                .is_some_and(|uuid| !uuid.is_empty()),
            "live Photos smoke must preserve the verified UUID"
        );
        let reconciled_output = run_photos_import_applescript_session_mode(
            "media reconciliation",
            &[(input.clone(), "✨system-library-smoke".to_string())],
            "reconcile",
        )?;
        let reconciled_identifier = reconciled_output.trim();
        assert_ne!(reconciled_identifier, "MFB_NOT_FOUND");
        let reconciled_handle = library_handle_from_media_output_probes(
            &candidates,
            &[(rel_path, reconciled_identifier.to_string())],
            query_osxphotos_asset_probes,
            path_has_quarantine_xattr,
        )?;
        assert_eq!(reconciled_handle.imported_assets.len(), 1);
        Ok(())
    }

    #[test]
    fn photos_import_retry_detection_covers_zero_import_invalid_connection_and_timeout() {
        assert!(
            photos_import_retry_reason(
                "execution error: Photos returned 0 imported items for /tmp/a.JXL (-2700)"
            )
            .is_some()
        );
        assert!(
            photos_import_retry_reason("execution error: “Photos”遇到一个错误：连接无效。 (-609)")
                .is_some()
        );
        assert!(
            photos_import_retry_reason(
                "execution error: “Photos”遇到一个错误：AppleEvent已超时。 (-1712)"
            )
            .is_some()
        );
        assert_eq!(
            photos_import_retry_reason(
                "execution error: “Photos”遇到一个错误：AppleEvent已超时。 (-1712)"
            ),
            Some("appleevent_timeout")
        );
        assert_eq!(
            photos_import_retry_reason(
                "Photos AppleScript import chunk timed out at hard timeout after 120s"
            ),
            Some("appleevent_timeout")
        );
        assert_eq!(
            photos_import_retry_reason(
                "Photos AppleScript import chunk timed out after 1800.045210458s / 1800s +                 (subprocess killed, exit_code=-1)"
            ),
            Some("appleevent_timeout"),
            "the historical Photos timeout wording must trigger the bounded recovery path"
        );
        assert_eq!(
            photos_import_retry_reason("Photos returned 4 imported items for 10 files"),
            None,
            "a partial import must not retry already imported files"
        );
    }

    #[test]
    fn photos_verifier_live_queries_pending_uuids_when_library_changes() -> Result<()> {
        let requested = ["UUID-OLD".to_string(), "UUID-NEW".to_string()];
        let previous_library = PathBuf::from("previous.photoslibrary");
        let current_library = PathBuf::from("current.photoslibrary");
        let libraries = [previous_library.clone(), current_library.clone()];
        let mut queries = Vec::new();

        let (probes, resolved_library) = query_osxphotos_asset_probes_in_libraries_with(
            &requested,
            &libraries,
            |library, uuids| {
                queries.push((library.to_path_buf(), uuids.to_vec()));
                if library == previous_library {
                    Ok(vec![FastImgLibraryAssetProbe {
                        uuid: "UUID-OLD".to_string(),
                        path: PathBuf::from("previous.photoslibrary/originals/old.AVIF"),
                        iscloudasset: true,
                        incloud: Some(true),
                        ismissing: false,
                    }])
                } else {
                    Ok(vec![FastImgLibraryAssetProbe {
                        uuid: "UUID-NEW".to_string(),
                        path: PathBuf::from("current.photoslibrary/originals/new.AVIF"),
                        iscloudasset: true,
                        incloud: Some(true),
                        ismissing: false,
                    }])
                }
            },
        )?;

        assert_eq!(
            queries,
            [
                (previous_library, requested.to_vec()),
                (current_library.clone(), vec!["UUID-NEW".to_string()]),
            ]
        );
        assert_eq!(resolved_library.as_ref(), Some(&current_library));
        assert_eq!(
            probes
                .iter()
                .map(|probe| probe.uuid.as_str())
                .collect::<Vec<_>>(),
            ["UUID-OLD", "UUID-NEW"]
        );
        Ok(())
    }

    #[test]
    fn photos_import_probe_targets_reject_duplicate_report_paths() {
        let err = fast_img_import_probe_targets(&[
            ("a.AVIF".to_string(), "UUID-A/L0/001".to_string()),
            ("a.AVIF".to_string(), "UUID-A/L0/001".to_string()),
        ])
        .expect_err("one Photos asset must not prove the same relative path twice");

        assert!(
            err.to_string().contains("duplicated relative path a.AVIF"),
            "unexpected duplicate-report error: {err}"
        );
    }

    #[test]
    fn photos_upload_verifier_batches_pending_without_head_of_line_blocking() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let library_asset_a = temp_dir.path().join("a.JXL");
        let library_asset_b = temp_dir.path().join("b.JXL");
        std::fs::write(&library_asset_a, b"a").unwrap();
        std::fs::write(&library_asset_b, b"b").unwrap();
        let attempts_a = Cell::new(0);
        let attempts_b = Cell::new(0);
        let query_processes = Cell::new(0);
        let targets = vec![
            FastImgImportProbeTarget {
                rel_path: "a.JXL".to_string(),
                import_identifier: "UUID-A/L0/001".to_string(),
                osxphotos_uuid: "UUID-A".to_string(),
            },
            FastImgImportProbeTarget {
                rel_path: "b.JXL".to_string(),
                import_identifier: "UUID-B/L0/001".to_string(),
                osxphotos_uuid: "UUID-B".to_string(),
            },
        ];
        let mut query_assets = |uuids: &[String]| {
            query_processes.set(query_processes.get() + 1);
            uuids
                .iter()
                .map(|uuid| {
                    let (path, uploaded) = match uuid.as_str() {
                        "UUID-A" => {
                            attempts_a.set(attempts_a.get() + 1);
                            (library_asset_a.clone(), attempts_a.get() >= 3)
                        }
                        "UUID-B" => {
                            attempts_b.set(attempts_b.get() + 1);
                            (library_asset_b.clone(), true)
                        }
                        other => panic!("unexpected uuid {other}"),
                    };
                    Ok(FastImgLibraryAssetProbe {
                        uuid: uuid.clone(),
                        path,
                        iscloudasset: uploaded,
                        incloud: Some(uploaded),
                        ismissing: false,
                    })
                })
                .collect::<Result<Vec<_>>>()
        };

        let probes = query_uploaded_asset_probes_batch_with_retry(
            &targets,
            4,
            1,
            Duration::ZERO,
            true,
            &mut query_assets,
            |_| {},
        )?;

        assert_eq!(query_processes.get(), 4);
        assert_eq!(attempts_a.get(), 3);
        assert_eq!(attempts_b.get(), 1);
        assert_eq!(probes.len(), 2);
        assert!(probes.contains_key("a.JXL"));
        assert!(probes.contains_key("b.JXL"));
        Ok(())
    }

    #[test]
    fn photos_upload_verifier_uses_bounded_batches_for_many_uploaded_assets() -> Result<()> {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let targets = (0..4)
            .map(|idx| FastImgImportProbeTarget {
                rel_path: format!("{idx}.JXL"),
                import_identifier: format!("UUID-{idx}/L0/001"),
                osxphotos_uuid: format!("UUID-{idx}"),
            })
            .collect::<Vec<_>>();
        for target in &targets {
            std::fs::write(temp_dir.path().join(&target.rel_path), b"jxl").unwrap();
        }
        let query_processes = Cell::new(0);
        let mut query_assets = |uuids: &[String]| {
            query_processes.set(query_processes.get() + 1);
            uuids
                .iter()
                .map(|uuid| {
                    let idx = uuid.strip_prefix("UUID-").unwrap();
                    Ok(FastImgLibraryAssetProbe {
                        uuid: uuid.clone(),
                        path: temp_dir.path().join(format!("{idx}.JXL")),
                        iscloudasset: true,
                        incloud: Some(true),
                        ismissing: false,
                    })
                })
                .collect::<Result<Vec<_>>>()
        };

        let probes = query_uploaded_asset_probes_batch_with_retry(
            &targets,
            1,
            2,
            Duration::ZERO,
            true,
            &mut query_assets,
            |_| {},
        )?;

        assert_eq!(query_processes.get(), 2);
        assert_eq!(probes.len(), 4);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_local_verifier_retries_bounded_visibility_lag() -> Result<()> {
        let targets = vec![FastImgImportProbeTarget {
            rel_path: "a.JXL".to_string(),
            import_identifier: "UUID-A/L0/001".to_string(),
            osxphotos_uuid: "UUID-A".to_string(),
        }];
        let temp_dir = tempfile::TempDir::new().unwrap();
        let library_asset = temp_dir.path().join("a.JXL");
        std::fs::write(&library_asset, b"jxl").unwrap();
        let query_processes = Cell::new(0);
        let mut query_assets = |uuids: &[String]| {
            query_processes.set(query_processes.get() + 1);
            uuids
                .iter()
                .map(|uuid| {
                    Ok(FastImgLibraryAssetProbe {
                        uuid: uuid.clone(),
                        path: library_asset.clone(),
                        iscloudasset: false,
                        incloud: Some(false),
                        ismissing: query_processes.get() < 2,
                    })
                })
                .collect::<Result<Vec<_>>>()
        };

        let probes = query_uploaded_asset_probes_batch_with_retry(
            &targets,
            8,
            1,
            Duration::ZERO,
            false,
            &mut query_assets,
            |_| {},
        )?;

        assert_eq!(query_processes.get(), 2);
        assert_eq!(probes.len(), 1);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_local_verifier_retries_transient_query_errors() -> Result<()> {
        let targets = vec![FastImgImportProbeTarget {
            rel_path: "a.AVIF".to_string(),
            import_identifier: "UUID-A/L0/001".to_string(),
            osxphotos_uuid: "UUID-A".to_string(),
        }];
        let temp_dir = tempfile::TempDir::new().unwrap();
        let library_asset = temp_dir.path().join("a.AVIF");
        std::fs::write(&library_asset, b"avif").unwrap();
        let query_processes = Cell::new(0);
        let mut query_assets = |uuids: &[String]| {
            query_processes.set(query_processes.get() + 1);
            if query_processes.get() < 3 {
                return Err(ImgQualityError::AnalysisError(
                    "osxphotos query failed: database is temporarily busy".to_string(),
                ));
            }
            Ok(vec![FastImgLibraryAssetProbe {
                uuid: uuids[0].clone(),
                path: library_asset.clone(),
                iscloudasset: false,
                incloud: Some(false),
                ismissing: false,
            }])
        };

        let probes = query_uploaded_asset_probes_batch_with_retry(
            &targets,
            5,
            1,
            Duration::ZERO,
            false,
            &mut query_assets,
            |_| {},
        )?;

        assert_eq!(query_processes.get(), 3);
        assert_eq!(probes.len(), 1);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn photos_upload_verifier_defaults_are_low_process_pressure() {
        assert_eq!(fast_img_icloud_upload_verify_attempts(), 5);
        assert!(fast_img_icloud_upload_verify_batch_size() <= 64);
        assert!(fast_img_icloud_upload_verify_delay() >= Duration::from_secs(2));
        assert!(fast_img_photos_import_timeout() <= Duration::from_secs(120));
        assert_eq!(fast_img_photos_import_batch_size(), 50);
        assert!(!fast_img_require_icloud_upload_proof());
    }

    #[test]
    #[serial_test::serial]
    fn photos_upload_verifier_attempts_are_capped_even_when_env_is_high() {
        let _guard = crate::common_utils::EnvGuard::set(FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_ENV, "60");

        assert_eq!(
            fast_img_icloud_upload_verify_attempts(),
            FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_MAX
        );
    }

    #[test]
    fn true_jpeg_rejects_png_magic() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
            .unwrap();
        assert!(!is_true_jpeg(f.path()).unwrap());
    }

    #[test]
    fn true_jpeg_rejects_truncated_header() {
        // 2 bytes — detect_format_from_bytes returns Unknown
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xFF, 0xD8]).unwrap();
        assert!(!is_true_jpeg(f.path()).unwrap());
    }

    #[test]
    fn true_jpeg_rejects_wrong_ext_disguise() {
        // GIF content pretending to be JPEG
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"GIF89a\x01\x00\x01\x00").unwrap();
        assert!(!is_true_jpeg(f.path()).unwrap());
    }

    #[test]
    fn true_jpeg_rejects_corrupt_multi_app_no_ff_third_byte() {
        // FF D8 but third byte ≠ 0xFF — not a valid JPEG marker sequence
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xFF, 0xD8, 0x00]).unwrap();
        assert!(!is_true_jpeg(f.path()).unwrap());
    }

    // ── §Integrity ────────────────────────────────────────────────────────────

    #[test]
    fn integrity_decode_probe_fails_on_empty_output() {
        let src = NamedTempFile::new().unwrap();
        let out = NamedTempFile::new().unwrap(); // 0 bytes
        let err = verify_jxl_roundtrip_integrity(src.path(), out.path()).unwrap_err();
        assert!(err.to_string().contains("empty"), "unexpected: {err}");
    }

    #[test]
    fn integrity_fails_closed_when_djxl_unavailable() {
        let src = NamedTempFile::new().unwrap();
        let mut out = NamedTempFile::new().unwrap();
        out.write_all(b"not-a-real-jxl-but-nonzero").unwrap();

        if crate::DjxlBuilder::check_available() {
            return;
        }
        let err = verify_jxl_roundtrip_integrity(src.path(), out.path()).unwrap_err();
        assert!(
            err.to_string().contains("djxl unavailable"),
            "unexpected: {err}"
        );
    }

    // ── §Delete Safety ────────────────────────────────────────────────────────

    #[test]
    fn delete_gate_fails_when_output_missing() {
        let src = NamedTempFile::new().unwrap();
        let missing = std::path::PathBuf::from("/tmp/__mfb_nonexistent_xyz.jxl");
        let integrity = IntegrityResult::RoundtripMatch {
            source_hash: "src".to_string(),
            output_hash: "out".to_string(),
        };
        let err = safe_delete_jpeg_source(src.path(), &missing, &integrity).unwrap_err();
        assert!(
            err.to_string().contains("does not exist"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn delete_gate_fails_when_output_empty() {
        let src = NamedTempFile::new().unwrap();
        let out = NamedTempFile::new().unwrap(); // 0 bytes
        let integrity = IntegrityResult::RoundtripMatch {
            source_hash: "src".to_string(),
            output_hash: "out".to_string(),
        };
        let err = safe_delete_jpeg_source(src.path(), out.path(), &integrity).unwrap_err();
        assert!(err.to_string().contains("empty"), "unexpected: {err}");
    }

    #[test]
    fn delete_gate_rejects_decode_probe_only_integrity() {
        let mut src = NamedTempFile::new().unwrap();
        src.write_all(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        let mut out = NamedTempFile::new().unwrap();
        out.write_all(b"output-bytes").unwrap();
        let integrity = IntegrityResult::DecodeProbePassed {
            output_hash: "out".to_string(),
        };

        let err = safe_delete_jpeg_source(src.path(), out.path(), &integrity).unwrap_err();

        assert!(
            err.to_string()
                .contains("exact JXL roundtrip or final non-JXL delivery proof is required"),
            "unexpected: {err}"
        );
        assert!(src.path().exists());
    }

    #[test]
    fn delete_gate_rejects_forged_roundtrip_hashes() {
        let mut src = NamedTempFile::new().unwrap();
        src.write_all(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        let mut out = NamedTempFile::new().unwrap();
        out.write_all(b"output-bytes").unwrap();
        let integrity = IntegrityResult::RoundtripMatch {
            source_hash: "forged-src".to_string(),
            output_hash: "forged-out".to_string(),
        };

        let err = safe_delete_jpeg_source(src.path(), out.path(), &integrity).unwrap_err();

        assert!(
            err.to_string().contains("stale or forged"),
            "unexpected: {err}"
        );
        assert!(src.path().exists());
    }

    #[test]
    fn delete_gate_removes_source_when_all_pass() {
        let mut src = NamedTempFile::new().unwrap();
        src.write_all(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        let mut out = NamedTempFile::new().unwrap();
        out.write_all(b"output-bytes").unwrap();

        let source_hash = crate::common_utils::calculate_blake3_hash(src.path()).unwrap();
        let output_hash = crate::common_utils::calculate_blake3_hash(out.path()).unwrap();
        let src_path = src.path().to_path_buf();
        let integrity = IntegrityResult::RoundtripMatch {
            source_hash,
            output_hash,
        };
        safe_delete_jpeg_source(&src_path, out.path(), &integrity).unwrap();
        assert!(!src_path.exists());
    }

    #[test]
    fn delete_gate_accepts_final_jxl_delivery_integrity() {
        let mut src = NamedTempFile::new().unwrap();
        src.write_all(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        let mut out = NamedTempFile::new().unwrap();
        out.write_all(b"metadata-rewritten-final-jxl").unwrap();

        let source_hash = crate::common_utils::calculate_blake3_hash(src.path()).unwrap();
        let output_hash = crate::common_utils::calculate_blake3_hash(out.path()).unwrap();
        let src_path = src.path().to_path_buf();
        let integrity = IntegrityResult::FinalModernDelivery {
            source_hash,
            output_hash,
        };
        safe_delete_jpeg_source(&src_path, out.path(), &integrity).unwrap();
        assert!(!src_path.exists());
    }

    #[test]
    fn delete_gate_rejects_jxl_pixel_equivalence_integrity() {
        let mut src = NamedTempFile::new().unwrap();
        src.write_all(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        let mut out = NamedTempFile::new().unwrap();
        out.write_all(b"no-jbrd-pixel-equivalent-jxl").unwrap();

        let source_hash = crate::common_utils::calculate_blake3_hash(src.path()).unwrap();
        let output_hash = crate::common_utils::calculate_blake3_hash(out.path()).unwrap();
        let src_path = src.path().to_path_buf();
        let integrity = IntegrityResult::JxlPixelEquivalent {
            source_hash,
            output_hash,
        };
        let error = safe_delete_jpeg_source(&src_path, out.path(), &integrity)
            .expect_err("pixel equivalence must never authorize source deletion");
        assert!(error.to_string().contains("non-exact integrity"));
        assert!(src_path.exists());
    }

    #[test]
    fn tier2_delete_gate_fails_when_source_missing() {
        let missing = std::path::PathBuf::from("/tmp/__mfb_nonexistent_tier2.webp");
        let proof = crate::pipeline::verification::LibraryAssetRecord {
            rel_path: "photo.webp".to_string(),
            blake3: "abc".to_string(),
            sync_status: "uploaded".to_string(),
            quarantined: false,
            photos_uuid: Some("uuid".to_string()),
            library_blake3: None,
        };
        let err = safe_delete_modern_lossy_static_source(&missing, &proof).unwrap_err();
        assert!(
            err.to_string().contains("does not exist"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn tier2_batch_delete_rejects_path_traversal_before_photos_query() {
        let src_dir = tempfile::tempdir().unwrap();
        let handle = crate::pipeline::verification::LibraryHandle {
            imported_assets: vec![crate::pipeline::verification::LibraryAssetRecord {
                rel_path: "../escape.webp".to_string(),
                blake3: "claimed".to_string(),
                sync_status: "local".to_string(),
                quarantined: false,
                photos_uuid: Some("uuid".to_string()),
                library_blake3: None,
            }],
            import_error_count: 0,
        };
        let error = delete_verified_modern_lossy_static_sources(src_dir.path(), &handle)
            .expect_err("unsafe relative path must fail before external verification");
        assert!(error.to_string().contains("unsafe or duplicate"));
    }

    #[test]
    fn tier2_delete_gate_fails_when_source_empty() {
        let src = NamedTempFile::new().unwrap();
        let proof = crate::pipeline::verification::LibraryAssetRecord {
            rel_path: "photo.webp".to_string(),
            blake3: "abc".to_string(),
            sync_status: "uploaded".to_string(),
            quarantined: false,
            photos_uuid: Some("uuid".to_string()),
            library_blake3: None,
        };
        let err = safe_delete_modern_lossy_static_source(src.path(), &proof).unwrap_err();
        assert!(err.to_string().contains("empty"), "unexpected: {err}");
    }

    #[test]
    fn tier2_delete_gate_rejects_forged_source_hash() {
        let mut src = NamedTempFile::new().unwrap();
        src.write_all(b"lossy-webp-bytes").unwrap();
        let proof = crate::pipeline::verification::LibraryAssetRecord {
            rel_path: "photo.webp".to_string(),
            blake3: "forged-hash".to_string(),
            sync_status: "uploaded".to_string(),
            quarantined: false,
            photos_uuid: Some("uuid".to_string()),
            library_blake3: None,
        };
        let err = safe_delete_modern_lossy_static_source(src.path(), &proof).unwrap_err();
        assert!(
            err.to_string().contains("stale or forged"),
            "unexpected: {err}"
        );
        assert!(src.path().exists());
    }

    #[test]
    fn tier2_delete_gate_removes_source_when_all_pass() {
        let mut src = NamedTempFile::new().unwrap();
        src.write_all(b"lossy-webp-bytes").unwrap();
        let source_hash = crate::common_utils::calculate_blake3_hash(src.path()).unwrap();
        let src_path = src.path().to_path_buf();
        let proof = crate::pipeline::verification::LibraryAssetRecord {
            rel_path: "photo.webp".to_string(),
            blake3: source_hash,
            sync_status: "uploaded".to_string(),
            quarantined: false,
            photos_uuid: Some("uuid".to_string()),
            library_blake3: None,
        };
        safe_delete_modern_lossy_static_source(&src_path, &proof).unwrap();
        assert!(!src_path.exists());
    }

    #[test]
    #[serial_test::serial]
    fn adaptive_timeout_records_startup_time() {
        use std::sync::atomic::Ordering;

        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(120, Ordering::SeqCst);

        record_osxphotos_query_startup_time(30);
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            120
        );

        record_osxphotos_query_startup_time(200);
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            230
        );

        record_osxphotos_query_startup_time(100);
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            230
        );

        record_osxphotos_query_startup_time(500);
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            480
        );
    }

    #[test]
    #[serial_test::serial]
    fn adaptive_timeout_uses_cached_base() {
        use std::sync::atomic::Ordering;

        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(120, Ordering::SeqCst);
        OSXPHOTOS_WARMED_UP.store(true, Ordering::SeqCst); // Pretend already warmed up

        let timeout1 = fast_img_osxphotos_query_timeout(1);
        assert_eq!(timeout1, Duration::from_mins(2));

        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(240, Ordering::SeqCst);

        let timeout2 = fast_img_osxphotos_query_timeout(1);
        assert_eq!(timeout2, Duration::from_mins(4));

        // 100 items: 240 base + 60 scaling (1 min) = 300s
        let timeout3 = fast_img_osxphotos_query_timeout(100);
        assert_eq!(timeout3, Duration::from_mins(5));
    }

    #[test]
    #[serial_test::serial]
    fn adaptive_timeout_cold_start_buffer() {
        use std::sync::atomic::Ordering;

        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(120, Ordering::SeqCst);
        OSXPHOTOS_WARMED_UP.store(false, Ordering::SeqCst);

        // First query (cold): 120 base + 180 cold buffer = 300s
        let timeout_cold = fast_img_osxphotos_query_timeout(1);
        assert_eq!(timeout_cold, Duration::from_mins(5));

        // Mark as warmed up
        OSXPHOTOS_WARMED_UP.store(true, Ordering::SeqCst);

        // Subsequent query (warm): 120 base + 0 buffer = 120s
        let timeout_warm = fast_img_osxphotos_query_timeout(1);
        assert_eq!(timeout_warm, Duration::from_mins(2));
    }

    #[test]
    #[serial_test::serial]
    fn adaptive_timeout_decay_on_fast_queries() {
        use std::sync::atomic::Ordering;

        // Start with high base (from previous slow query)
        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(400, Ordering::SeqCst);
        OSXPHOTOS_WARMED_UP.store(true, Ordering::SeqCst);

        // Record fast query (30s) - should trigger decay
        // new_base = 30+30 = 60, old = 400, 60 < 200 (old/2) → decay
        record_osxphotos_query_startup_time(30);

        // Base should decay: 400 * 3/4 = 300
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            300
        );

        // Another fast query - continue decay
        record_osxphotos_query_startup_time(30);
        // 300 * 3/4 = 225
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            225
        );

        // Keep decaying (225 > 180, 60 < 112.5)
        record_osxphotos_query_startup_time(30);
        // 225 * 3/4 = 168
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            168
        );

        // Now 168 < 180, decay stops (condition: old > 180)
        record_osxphotos_query_startup_time(30);
        // No decay, stays at 168
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            168
        );
    }
}
