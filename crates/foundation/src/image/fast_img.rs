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
//!   JXL lossless transcode
//! - \[I1\] chose: shortest-path import uses Photos `AppleScript` UUID import
//!   plus osxphotos query verifier and fails closed
//! - \[I2\] chose: default verifier proves Photos local custody; iCloud upload
//!   completion polling is explicit opt-in to avoid pressuring Photos/cloud
//!   daemons

use crate::pipeline::verification::{
    Blake3Entry, LibraryAssetRecord, LibraryHandle, WorkingCopyMarker, write_marker_atomic,
};
use crate::unified_error::{ImgQualityError, Result};
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

/// Roundtrip BLAKE3 integrity check for a raw JXL lossless transcode
/// (§Integrity).
///
/// Decodes `jxl_output` back to a JPEG via `djxl`, then compares
/// `BLAKE3(decoded.jpg) == BLAKE3(source_jpeg)`.
///
/// This is bit-exact proof the raw JXL container faithfully preserves the JPEG
/// bitstream before delivery metadata edits. Final fast-img delivery uses
/// [`verify_final_jxl_delivery_integrity`] because EXIF/XMP rewrites and
/// upstream JXL Orientation exclusion can legitimately rewrite container
/// metadata after the raw transcode proof.
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

    let decode_output = DjxlBuilder::new()
        .input(jxl_output)
        .output(temp_path)
        .build()
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!("integrity: djxl decode failed: {e}"))
        })?;

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

/// Pixel-equivalence integrity proof for JPEG→JXL transcodes without JBRD.
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
    use crate::common_utils::calculate_blake3_hash;
    use crate::image::format_detect::FormatKind;
    use crate::image::orientation::{
        PixelDiffResult, orientation_diff_tolerance_for_format, verify_orientation_pixel_diff,
    };

    let out_meta = std::fs::metadata(jxl_output).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "pixel-equivalence: cannot stat output {}: {e}",
            jxl_output.display()
        ))
    })?;
    if out_meta.len() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel-equivalence: output is empty: {}",
            jxl_output.display()
        )));
    }

    let tolerance = orientation_diff_tolerance_for_format(FormatKind::Jxl).ok_or_else(|| {
        ImgQualityError::AnalysisError(
            "pixel-equivalence: missing JXL orientation policy".to_string(),
        )
    })?;
    match verify_orientation_pixel_diff(source_jpeg, jxl_output, FormatKind::Jxl, tolerance)? {
        PixelDiffResult::Match => {}
        PixelDiffResult::SkippedToolAbsent { tool } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "pixel-equivalence: proof unavailable for {}: missing {tool}",
                jxl_output.display()
            )));
        }
        PixelDiffResult::Mismatch { max_delta, channel } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "pixel-equivalence: proof failed for {}: max_delta={max_delta} channel={channel}",
                jxl_output.display()
            )));
        }
    }

    let source_hash = calculate_blake3_hash(source_jpeg).map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-equivalence: BLAKE3(source) failed: {e}"))
    })?;
    let output_hash = calculate_blake3_hash(jxl_output).map_err(|e| {
        ImgQualityError::AnalysisError(format!("pixel-equivalence: BLAKE3(output) failed: {e}"))
    })?;

    tracing::info!(
        target: "fast_img_integrity",
        source = %source_jpeg.display(),
        source_blake3 = %source_hash,
        output = %jxl_output.display(),
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
/// The final JXL may no longer decode back to a byte-identical JPEG after
/// metadata preservation and Orientation tag cleanup. This check therefore
/// proves the mechanically verifiable post-delivery state: current source hash,
/// current output hash, non-empty output, decoder readability,
/// orientation-correct pixels, and no residual output Orientation tag.
///
/// # Errors
/// Returns an error if any proof step fails.
pub fn verify_final_jxl_delivery_integrity(
    source_jpeg: &Path,
    jxl_output: &Path,
) -> Result<IntegrityResult> {
    use crate::common_utils::calculate_blake3_hash;
    use crate::image::format_detect::FormatKind;
    use crate::image::orientation::{
        PixelDiffResult, orientation_diff_tolerance_for_format, verify_orientation_pixel_diff,
    };

    let out_meta = std::fs::metadata(jxl_output).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "final-integrity: cannot stat output {}: {e}",
            jxl_output.display()
        ))
    })?;
    if out_meta.len() == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "final-integrity: output is empty: {}",
            jxl_output.display()
        )));
    }

    if !crate::DjxlBuilder::check_available() {
        return Err(ImgQualityError::AnalysisError(format!(
            "final-integrity: djxl unavailable; cannot verify final JXL {}",
            jxl_output.display()
        )));
    }

    let tolerance = orientation_diff_tolerance_for_format(FormatKind::Jxl).ok_or_else(|| {
        ImgQualityError::AnalysisError(
            "final-integrity: missing JXL orientation policy".to_string(),
        )
    })?;
    match verify_orientation_pixel_diff(source_jpeg, jxl_output, FormatKind::Jxl, tolerance)? {
        PixelDiffResult::Match => {}
        PixelDiffResult::SkippedToolAbsent { tool } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "final-integrity: orientation proof unavailable for {}: missing {tool}",
                jxl_output.display()
            )));
        }
        PixelDiffResult::Mismatch { max_delta, channel } => {
            return Err(ImgQualityError::AnalysisError(format!(
                "final-integrity: orientation proof failed for {}: max_delta={max_delta} \
                 channel={channel}",
                jxl_output.display()
            )));
        }
    }

    ensure_no_residual_orientation_tag(jxl_output)?;

    let source_hash = calculate_blake3_hash(source_jpeg).map_err(|e| {
        ImgQualityError::AnalysisError(format!("final-integrity: BLAKE3(source) failed: {e}"))
    })?;
    let output_hash = calculate_blake3_hash(jxl_output).map_err(|e| {
        ImgQualityError::AnalysisError(format!("final-integrity: BLAKE3(output) failed: {e}"))
    })?;

    tracing::info!(
        target: "fast_img_integrity",
        source = %source_jpeg.display(),
        source_blake3 = %source_hash,
        output = %jxl_output.display(),
        output_blake3 = %output_hash,
        "final JXL delivery integrity check"
    );

    Ok(IntegrityResult::FinalJxlDelivery {
        source_hash,
        output_hash,
    })
}

fn ensure_no_residual_orientation_tag(path: &Path) -> Result<()> {
    use crate::ToolBuilder;
    use crate::image_builders::ExiftoolBuilder;

    if !ExiftoolBuilder::check_available() {
        return Err(ImgQualityError::AnalysisError(format!(
            "final-integrity: exiftool unavailable; cannot verify Orientation tag absence for {}",
            path.display()
        )));
    }
    let output = ExiftoolBuilder::new()
        .arg("-s3")
        .arg("-Orientation")
        .input(path)
        .build()
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "final-integrity: Orientation probe failed for {}: {e}",
                path.display()
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ImgQualityError::AnalysisError(format!(
            "final-integrity: Orientation probe exited non-zero for {}: {stderr}",
            path.display()
        )));
    }
    if !output.stdout.is_empty() {
        return Err(ImgQualityError::AnalysisError(format!(
            "final-integrity: residual Orientation tag present in {}",
            path.display()
        )));
    }
    Ok(())
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
    /// JPEG→JXL no-JBRD fallback passed decoded pixel equivalence proof.
    JxlPixelEquivalent {
        source_hash: String,
        output_hash: String,
    },
    /// Final delivery JXL passed source/output hash, decode, metadata, and
    /// orientation proofs.
    FinalJxlDelivery {
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
        IntegrityResult::FinalJxlDelivery {
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
        IntegrityResult::JxlPixelEquivalent {
            source_hash,
            output_hash,
        } => {
            tracing::info!(
                target: "fast_img_delete",
                %source_hash,
                %output_hash,
                "delete-gate 1: JXL pixel-equivalence proof confirmed"
            );
            (source_hash, output_hash)
        }
        IntegrityResult::DecodeProbePassed { output_hash } => {
            tracing::error!(
                target: "fast_img_delete",
                %output_hash,
                "delete-gate 1 FAIL: decode-probe-only integrity is not sufficient for deletion"
            );
            return Err(ImgQualityError::AnalysisError(
                "delete-gate 1 FAIL: final JXL delivery proof or raw roundtrip proof is required \
                 before deleting source JPEG"
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

/// Delete the XMP sidecar that matches a source JPEG after the caller has
/// already verified the source JPEG is gone and the JXL output proof is still
/// current.
///
/// # Errors
/// Returns an error if a matching sidecar exists but cannot be removed.
pub fn safe_delete_matching_xmp_sidecar(source: &Path, output: &Path) -> Result<bool> {
    delete_matching_xmp_sidecar_path(
        source,
        output,
        crate::metadata::find_xmp_sidecar(source).as_deref(),
    )
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
        "delete-gate PASS: removing merged XMP sidecar"
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
        if library_blake3 != asset.blake3 {
            return Err(ImgQualityError::AnalysisError(format!(
                "tier-2 delete gate Photos BLAKE3 drift for {} (uuid={uuid}): expected={} library={library_blake3}",
                asset.rel_path, asset.blake3
            )));
        }
    }
    Ok(())
}

fn preflight_modern_lossy_static_source_deletion(
    library_handle: &crate::pipeline::verification::LibraryHandle,
    gate3_passed: bool,
) -> Result<()> {
    if !gate3_passed {
        return Err(ImgQualityError::AnalysisError(
            "tier-2 source delete gate requires Gate 3 passed".to_string(),
        ));
    }
    if library_handle.import_error_count != 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "tier-2 source delete gate refuses import errors: {}",
            library_handle.import_error_count
        )));
    }
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
    }
    Ok(())
}

/// Delete tier-2 lossy modern static sources after Gate 3 and Photos custody re-verification.
pub fn delete_verified_modern_lossy_static_sources(
    src_dir: &Path,
    library_handle: &crate::pipeline::verification::LibraryHandle,
    gate3_passed: bool,
) -> Result<(usize, usize)> {
    if library_handle.imported_assets.is_empty() {
        return Ok((0, 0));
    }
    preflight_modern_lossy_static_source_deletion(library_handle, gate3_passed)?;
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
    marker
        .tier2_imported_assets
        .clone_from(&library.imported_assets);
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
) -> Result<usize> {
    if !src_dir.is_dir() {
        return Ok(0);
    }
    let mut dirs = Vec::new();
    for asset in imported_assets {
        let source = src_dir.join(&asset.rel_path);
        let mut current = source.parent();
        while let Some(dir) = current {
            if dir == src_dir {
                break;
            }
            dirs.push(dir.to_path_buf());
            current = dir.parent();
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut pruned = 0usize;
    for dir in dirs {
        let mut entries = std::fs::read_dir(&dir).map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "read tier-2 source dir {}: {err}",
                dir.display()
            ))
        })?;
        if entries
            .next()
            .transpose()
            .map_err(|err| {
                ImgQualityError::AnalysisError(format!(
                    "read tier-2 source dir entry {}: {err}",
                    dir.display()
                ))
            })?
            .is_some()
        {
            continue;
        }
        std::fs::remove_dir(&dir).map_err(|err| {
            ImgQualityError::AnalysisError(format!(
                "delete empty tier-2 source directory {}: {err}",
                dir.display()
            ))
        })?;
        pruned += 1;
        tracing::info!(
            target: "fast_img_delete",
            path = %dir.display(),
            "delete-gate PASS: removing empty tier-2 source directory"
        );
    }
    Ok(pruned)
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
    report_rel_path: String,
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

/// Import JXL outputs into Photos and return a concrete verifier handle.
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
/// shortest-path mode fails closed while preserving the JXL-only output
/// directory and source JPEGs.
pub fn import_jxl_outputs_with_library_verifier(
    marker: &WorkingCopyMarker,
) -> Result<LibraryHandle> {
    let _photos_import_lock = acquire_photos_import_lock()?;
    let output_paths = fast_img_marker_output_paths(marker);
    #[cfg(target_os = "macos")]
    let quarantine_probe = path_has_quarantine_xattr;
    #[cfg(not(target_os = "macos"))]
    let quarantine_probe = |path: &Path| Ok(path_has_quarantine_xattr(path));
    import_jxl_outputs_with_photos_checkpoint(
        marker,
        &output_paths,
        query_osxphotos_asset_probes,
        quarantine_probe,
    )
}

pub fn import_media_outputs_with_library_verifier(
    candidates: &[PhotosImportCandidate],
) -> Result<LibraryHandle> {
    let _photos_import_lock = acquire_photos_import_lock()?;
    let report_pairs = import_media_outputs_with_photos_applescript(candidates)?;
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
    import_media_outputs_with_library_verifier(&import_candidates)
}

const FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT: &str = r#"
on run argv
    if (count of argv) is not 5 then
        error "Photos import expected manifest path, batch size, batch delay, digest pause interval, and digest pause seconds arguments"
    end if
    set manifestPath to item 1 of argv
    set batchSize to (item 2 of argv) as integer
    set batchDelayMs to (item 3 of argv) as integer
    set digestPauseInterval to (item 4 of argv) as integer
    set digestPauseSecs to (item 5 of argv) as integer
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
    set importedIds to {}
    set batchNumber to 0
    with timeout of 86400 seconds
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

on mfbEnsureTopLevelFolder(folderName)
    tell application "Photos"
        repeat with candidateFolder in folders
            if name of candidateFolder is folderName then
                return candidateFolder
            end if
        end repeat
        return make new folder named folderName
    end tell
end mfbEnsureTopLevelFolder

on mfbEnsureChildFolder(parentFolder, folderName)
    tell application "Photos"
        repeat with candidateFolder in folders of parentFolder
            if name of candidateFolder is folderName then
                return candidateFolder
            end if
        end repeat
        return make new folder named folderName at parentFolder
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
            repeat with candidateAlbum in albums of targetFolder
                if name of candidateAlbum is targetAlbumName then
                    return (id of candidateAlbum as text)
                end if
            end repeat
            set createdAlbum to make new album named targetAlbumName at targetFolder
            return (id of createdAlbum as text)
        end tell
    else
        set targetAlbumName to item 1 of pathItems
        tell application "Photos"
            repeat with candidateAlbum in albums
                if name of candidateAlbum is targetAlbumName then
                    return (id of candidateAlbum as text)
                end if
            end repeat
            set createdAlbum to make new album named targetAlbumName
            return (id of createdAlbum as text)
        end tell
    end if
end mfbEnsureAlbumIdForPath
"#;

fn import_jxl_outputs_with_photos_checkpoint<Q, P>(
    marker: &WorkingCopyMarker,
    output_paths: &[(String, PathBuf)],
    mut query_assets: Q,
    mut is_quarantined: P,
) -> Result<LibraryHandle>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
{
    prepare_photos_import_output_paths(output_paths)?;
    let plan = photos_import_checkpoint_plan(marker, &mut is_quarantined)?;
    let expected_output_count = marker.expected_output_count();
    let mut checkpoint_marker = marker.clone();
    let PhotosImportCheckpointPlan {
        pending_entries,
        proven_assets,
    } = plan;
    let mut imported_assets = proven_assets;
    let mut prepare_import_session = prepare_photos_import_session;
    let mut run_import_batch = |batch_entries: &[(PathBuf, String)]| {
        run_photos_import_applescript_session("JXL", batch_entries)
    };
    let mut pending_imported = import_pending_jxl_entries_with_checkpoint(
        &mut checkpoint_marker,
        &pending_entries,
        &mut query_assets,
        &mut is_quarantined,
        &mut prepare_import_session,
        &mut run_import_batch,
    )?;
    imported_assets.append(&mut pending_imported);
    imported_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    if imported_assets.len() != expected_output_count {
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos AppleScript import established {} verified assets for {} JXL outputs (marker \
             expected {}). The importer checkpoints each verified window and resumes pending \
             assets on rerun.",
            imported_assets.len(),
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
        import_error_count: 0,
    })
}

fn import_media_outputs_with_photos_applescript(
    candidates: &[PhotosImportCandidate],
) -> Result<Vec<(String, String)>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    validate_photos_import_candidates(candidates)?;
    let manifest_entries = photos_import_candidate_manifest_entries(candidates);
    let stdout = run_photos_import_applescript_session("media", &manifest_entries)?;
    let report_pairs = photos_import_pairs_from_candidates(candidates, stdout.as_bytes())?;
    tracing::info!(
        target: "photos_import",
        imported = report_pairs.len(),
        batch_size = fast_img_photos_import_batch_size(),
        "Photos AppleScript media import complete"
    );
    Ok(report_pairs)
}

fn photos_import_checkpoint_plan<P>(
    marker: &WorkingCopyMarker,
    mut is_quarantined: P,
) -> Result<PhotosImportCheckpointPlan>
where
    P: FnMut(&Path) -> Result<bool>,
{
    let mut pending_entries = Vec::new();
    let mut proven_assets = Vec::new();
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
        proven_assets.push(LibraryAssetRecord {
            rel_path,
            blake3: library_asset.clone(),
            sync_status: "photos_local".to_string(),
            quarantined: is_quarantined(&path)?,
            photos_uuid: None,
        });
    }
    pending_entries.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    proven_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(PhotosImportCheckpointPlan {
        pending_entries,
        proven_assets,
    })
}

fn import_pending_jxl_entries_with_checkpoint<Q, P, R>(
    marker: &mut WorkingCopyMarker,
    pending_entries: &[PhotosImportPendingEntry],
    query_assets: &mut Q,
    is_quarantined: &mut P,
    prepare_import_session: &mut impl FnMut(&str) -> Result<()>,
    run_import_batch: &mut R,
) -> Result<Vec<LibraryAssetRecord>>
where
    Q: FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    P: FnMut(&Path) -> Result<bool>,
    R: FnMut(&[(PathBuf, String)]) -> Result<String>,
{
    if pending_entries.is_empty() {
        return Ok(Vec::new());
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
            )?;
        }
        let batch_sizes = photos_import_batch_sizes_for_strategy(strategy, entries.len());
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
            let manifest_entries = batch_entries
                .iter()
                .map(|entry| (entry.path.clone(), entry.album_name.clone()))
                .collect::<Vec<_>>();
            tracing::info!(
                target: "photos_import",
                window_start = window.start,
                window_len = window.len,
                batch_number,
                batch_count,
                batch_files = batch_entries.len(),
                "Starting Photos import batch"
            );
            let mut poisoned_attempts = 0usize;
            let stdout = loop {
                match run_import_batch(&manifest_entries) {
                    Ok(stdout) => break stdout,
                    Err(err) => {
                        let detail = err.to_string();
                        let Some(poison_reason) = photos_import_poison_reason(&detail) else {
                            return Err(err);
                        };
                        tracing::warn!(
                            target: "photos_import",
                            window_start = window.start,
                            batch_number,
                            batch_count,
                            poisoned_attempts,
                            poison_reason,
                            detail = %detail,
                            "Photos import batch hit a recoverable session failure; relaunching Photos and retrying"
                        );
                        if poisoned_attempts >= FAST_IMG_PHOTOS_IMPORT_POISON_RETRY_LIMIT {
                            return Err(err);
                        }
                        handle_photos_import_recovery(
                            "poisoned_session",
                            &mut relaunch_photos_for_import_recovery,
                        )?;
                        poisoned_attempts = poisoned_attempts.checked_add(1).ok_or_else(|| {
                            ImgQualityError::AnalysisError(
                                "Photos import poison retry counter overflowed".to_string(),
                            )
                        })?;
                        tracing::info!(
                            target: "photos_import",
                            window_start = window.start,
                            batch_number,
                            batch_count,
                            poisoned_attempts,
                            poison_reason,
                            "Photos import recovery complete; automatically retrying current batch"
                        );
                    }
                }
            };
            let output_paths = batch_entries
                .iter()
                .map(|entry| (entry.rel_path.clone(), entry.path.clone()))
                .collect::<Vec<_>>();
            let report_pairs = fast_img_pairs_from_photos_import_ids(
                &output_paths,
                stdout.as_bytes(),
                batch_entries.len(),
            )?;
            let mut batch_assets = library_records_from_pending_import(
                batch_entries,
                &report_pairs,
                query_assets,
                is_quarantined,
            )?;
            checkpoint_photos_import_window(marker, batch_entries, &batch_assets)?;
            imported_assets.append(&mut batch_assets);
            offset = end;
            if batch_index + 1 < batch_count {
                std::thread::sleep(Duration::from_millis(FAST_IMG_PHOTOS_IMPORT_BATCH_DELAY_MS));
            }
            if FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL > 0
                && (batch_index + 1) % FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL == 0
            {
                std::thread::sleep(Duration::from_secs(
                    FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_SECS,
                ));
            }
        }
        if window.start + window.len < pending_entries.len() {
            std::thread::sleep(Duration::from_secs(
                FAST_IMG_PHOTOS_IMPORT_WINDOW_PAUSE_SECS,
            ));
        }
    }
    Ok(imported_assets)
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
        if !report_index.contains_key(&entry.rel_path) {
            return Err(ImgQualityError::AnalysisError(format!(
                "Photos verifier missing import identifier for {}",
                entry.rel_path
            )));
        }
        let verified_probe = remove_matching_library_probe_by_hash(
            &mut verified_probes,
            &entry.rel_path,
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
            photos_uuid: None,
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
            Ok(VerifiedLibraryProbe {
                report_rel_path,
                probe,
                blake3,
            })
        })
        .collect()
}

fn remove_matching_library_probe_by_hash(
    verified_probes: &mut Vec<VerifiedLibraryProbe>,
    rel_path: &str,
    expected_hash: &str,
    output_path: &Path,
    error_suffix: &str,
) -> Result<VerifiedLibraryProbe> {
    if let Some(index) = verified_probes
        .iter()
        .position(|verified| verified.blake3 == expected_hash)
    {
        return Ok(verified_probes.remove(index));
    }

    let candidate = verified_probes
        .iter()
        .find(|verified| verified.report_rel_path == rel_path)
        .or_else(|| verified_probes.first());
    if let Some(candidate) = candidate {
        tracing::error!(
            target: "photos_import",
            rel_path = %rel_path,
            output = %expected_hash,
            library = %candidate.blake3,
            output_path = %output_path.display(),
            library_path = %candidate.probe.path.display(),
            "Photos imported bytes diverged from working copy"
        );
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos verifier BLAKE3 mismatch for {rel_path}: output={expected_hash} library={} \
             output_path={} library_path={}. {error_suffix}",
            candidate.blake3,
            output_path.display(),
            candidate.probe.path.display()
        )));
    }

    Err(ImgQualityError::AnalysisError(format!(
        "Photos verifier missing library probe for {rel_path}: output={expected_hash} \
         output_path={}",
        output_path.display()
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

        let osascript = resolve_osascript_command();
        let mut command = std::process::Command::new(&osascript);
        command
            .arg("-e")
            .arg(FAST_IMG_PHOTOS_IMPORT_APPLESCRIPT)
            .arg(manifest_file.path())
            .arg(FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE.to_string())
            .arg(FAST_IMG_PHOTOS_IMPORT_BATCH_DELAY_MS.to_string())
            .arg(FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL.to_string())
            .arg(FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_SECS.to_string());

        let timeout = photos_import_session_timeout(batch_count)?;

        let output = crate::process_runner::ManagedProcess::spawn(&mut command)
            .and_then(|process| process.wait_timeout(timeout, "Photos AppleScript import chunk"))
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
            let output = std::process::Command::new("ps")
                .args(["-p", &pid, "-o", "rss=,vsz="])
                .output();

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
                        "Photos process memory probe spawn failed: {err}"
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

    match std::process::Command::new("vm_stat").output() {
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
                "vm_stat probe spawn failed: {err}"
            );
        }
    }
}

fn get_photos_pid() -> Result<Option<String>> {
    let output = std::process::Command::new("pgrep")
        .args(["-x", "Photos"])
        .output()
        .map_err(|err| {
            tracing::warn!(
                target: "photos_import",
                "Photos process lookup spawn failed: {err}"
            );
            ImgQualityError::AnalysisError(format!("Photos process lookup spawn failed: {err}"))
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

fn photos_import_poison_reason(detail: &str) -> Option<&'static str> {
    let lower = detail.to_ascii_lowercase();
    if photos_zero_import_context(detail).is_some()
        || lower.contains("photos returned 0 imported items")
    {
        Some("zero_import_items")
    } else if lower.contains("invalid connection")
        || lower.contains("connection is invalid")
        || lower.contains("(-609)")
        || detail.contains("连接无效")
    {
        Some("invalid_connection")
    } else if lower.contains("(-1712)")
        || detail.contains("超时")
        || detail.contains("AppleEvent已超时")
    {
        Some("appleevent_timeout")
    } else {
        None
    }
}

fn handle_photos_import_recovery(
    reason: &str,
    relaunch: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    match relaunch(reason) {
        Ok(()) => Ok(()),
        Err(err) if reason == "periodic_window_boundary" => {
            tracing::warn!(
                target: "photos_import",
                reason,
                error = %err,
                "Periodic Photos recovery failed; checking if Photos is still responsive"
            );

            #[cfg(all(target_os = "macos", not(test)))]
            match get_photos_pid() {
                Ok(Some(_pid)) => {
                    tracing::warn!(
                        target: "photos_import",
                        reason,
                        "Photos still running after recovery failure; session may be degraded"
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        target: "photos_import",
                        reason,
                        error = %e,
                        "Failed to check if Photos is running after recovery failure"
                    );
                }
            }

            tracing::warn!(
                target: "photos_import",
                reason,
                "Continuing with best-effort session; import verifier remains authoritative"
            );
            Ok(())
        }
        Err(err) => Err(err),
    }
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
    let quit_output = crate::process_runner::ManagedProcess::spawn(&mut quit_command)
        .and_then(|process| {
            process.wait_timeout(
                Duration::from_secs(FAST_IMG_PHOTOS_IMPORT_RELAUNCH_COMMAND_TIMEOUT_SECS),
                "Photos import recovery quit",
            )
        })
        .map_err(|err| {
            tracing::error!(
                target: "photos_import",
                reason,
                "Photos recovery quit command failed via {}: {err}",
                osascript.display()
            );
            ImgQualityError::AnalysisError(format!(
                "Photos recovery quit command failed via {}: {err}",
                osascript.display()
            ))
        })?;
    if !quit_output.status.success() {
        let stderr = quit_output.stderr.trim();
        tracing::error!(
            target: "photos_import",
            reason,
            status = ?quit_output.status.code(),
            stderr = %stderr,
            "Photos recovery quit command exited unsuccessfully"
        );
        return Err(ImgQualityError::AnalysisError(format!(
            "Photos recovery quit command exited unsuccessfully: {stderr}"
        )));
    }

    wait_for_photos_process_state(false, "quit")?;

    let mut last_launch_error = None;
    for attempt in 1..=FAST_IMG_PHOTOS_IMPORT_RELAUNCH_OPEN_ATTEMPTS {
        let mut open_command = std::process::Command::new("open");
        open_command.args(["-a", "Photos"]);
        let open_output = crate::process_runner::ManagedProcess::spawn(&mut open_command)
            .and_then(|process| {
                process.wait_timeout(
                    Duration::from_secs(FAST_IMG_PHOTOS_IMPORT_RELAUNCH_COMMAND_TIMEOUT_SECS),
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
        let _ = std::process::Command::new("killall")
            .args(["-9", "Photos"])
            .status();
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
        let _ = std::process::Command::new("killall")
            .args(["-KILL", "Photos"])
            .status();
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

#[cfg(test)]
fn photos_import_batch_sizes(total: usize) -> Vec<usize> {
    photos_import_batch_sizes_for_strategy(photos_import_strategy(total), total)
}

fn photos_import_batch_sizes_for_strategy(
    _strategy: PhotosImportStrategy,
    total: usize,
) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    vec![total]
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
    mut query_assets: impl FnMut(&[String]) -> Result<Vec<FastImgLibraryAssetProbe>>,
    mut is_quarantined: impl FnMut(&Path) -> Result<bool>,
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
        if library_blake3 != candidate.blake3 {
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
                 Import aborted because Photos library bytes do not match the source file.",
                target.rel_path, probe.uuid, candidate.blake3
            )));
        }
        let quarantined = is_quarantined(&candidate.path)?;
        imported_assets.push(LibraryAssetRecord {
            rel_path: target.rel_path,
            blake3: candidate.blake3.clone(),
            sync_status: photos_sync_status(probe).to_string(),
            quarantined,
            photos_uuid: Some(probe.uuid.clone()),
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
    Ok(())
}

pub fn library_handle_from_marker_import_proof(
    marker: &WorkingCopyMarker,
) -> Result<Option<LibraryHandle>> {
    #[cfg(target_os = "macos")]
    {
        library_handle_from_marker_import_proof_with(marker, path_has_quarantine_xattr)
    }
    #[cfg(not(target_os = "macos"))]
    {
        library_handle_from_marker_import_proof_with(marker, |path| {
            Ok(path_has_quarantine_xattr(path))
        })
    }
}

fn library_handle_from_marker_import_proof_with(
    marker: &WorkingCopyMarker,
    mut is_quarantined: impl FnMut(&Path) -> Result<bool>,
) -> Result<Option<LibraryHandle>> {
    let proof_count = marker
        .blake3_log
        .values()
        .filter(|entry| entry.library_asset.is_some())
        .count();
    if proof_count == 0 {
        return Ok(None);
    }
    let expected_output_count = marker.expected_output_count();
    if proof_count != marker.blake3_log.len() || proof_count != expected_output_count {
        return Err(ImgQualityError::AnalysisError(format!(
            "fast-img marker has partial Photos import proof: \
             {proof_count}/{expected_output_count} entries"
        )));
    }

    let mut imported_assets = Vec::new();
    for (source_rel, entry) in &marker.blake3_log {
        let rel_path = marker_entry_out_rel(source_rel, entry);
        let Some(library_asset) = entry.library_asset.as_ref() else {
            return Err(ImgQualityError::AnalysisError(format!(
                "fast-img marker import proof missing for {source_rel}"
            )));
        };
        if entry.out != *library_asset {
            return Err(ImgQualityError::AnalysisError(format!(
                "fast-img marker import proof hash drift for {source_rel}: output={} \
                 library={library_asset}",
                entry.out
            )));
        }
        let output_path = marker.working_copy.join(&rel_path);
        imported_assets.push(LibraryAssetRecord {
            rel_path,
            blake3: library_asset.clone(),
            sync_status: "photos_local".to_string(),
            quarantined: is_quarantined(&output_path)?,
            photos_uuid: None,
        });
    }
    imported_assets.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
    Ok(Some(LibraryHandle {
        imported_assets,
        import_error_count: 0,
    }))
}

fn query_osxphotos_asset_probes(uuids: &[String]) -> Result<Vec<FastImgLibraryAssetProbe>> {
    const MAX_RETRIES: usize = 3;
    const BASE_RETRY_DELAY_SECS: u64 = 10; // Faster initial retry
    const MAX_RETRY_DELAY_SECS: u64 = 60; // Lower max delay
    if uuids.is_empty() {
        return Ok(Vec::new());
    }

    let mut last_error: Option<String> = None;
    for attempt in 1..=MAX_RETRIES {
        match query_osxphotos_asset_probes_once(uuids) {
            Ok(probes) => return Ok(probes),
            Err(e) => {
                let err_str = e.to_string();
                let is_timeout =
                    err_str.contains("timed out") || err_str.contains("subprocess killed");
                let is_fatal_auth = err_str.contains("OperationalError")
                    || err_str.contains("unable to open database file")
                    || err_str.contains("TCC")
                    || err_str.contains("Operation not permitted");
                if is_fatal_auth {
                    return Err(e);
                }
                if !is_timeout || attempt == MAX_RETRIES {
                    return Err(e);
                }
                last_error = Some(err_str);

                // Cleanup: try to kill any lingering osxphotos processes
                tracing::warn!(
                    target: "photos_import",
                    attempt,
                    "osxphotos query timeout; attempting cleanup of stale processes"
                );
                let _ = std::process::Command::new("pkill")
                    .args(["-9", "osxphotos"])
                    .status();
                std::thread::sleep(Duration::from_secs(1));

                // Extend timeout for next attempt (adaptive)
                extend_osxphotos_query_timeout();

                // Faster exponential backoff: 10s, 20s, 40s
                let delay_secs =
                    (BASE_RETRY_DELAY_SECS * (1 << (attempt - 1))).min(MAX_RETRY_DELAY_SECS);
                tracing::warn!(
                    target: "photos_import",
                    attempt,
                    max_retries = MAX_RETRIES,
                    delay_secs,
                    "osxphotos query timeout, extending timeout and retrying"
                );
                std::thread::sleep(Duration::from_secs(delay_secs));
            }
        }
    }
    Err(ImgQualityError::AnalysisError(format!(
        "osxphotos query failed after {} retries: {}",
        MAX_RETRIES,
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )))
}

fn query_osxphotos_asset_probes_once(uuids: &[String]) -> Result<Vec<FastImgLibraryAssetProbe>> {
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
        .arg(crate::common_utils::photos_library_path()?)
        .arg("--uuid-from-file")
        .arg(uuid_file.path())
        .arg("--mute")
        .arg("--json");
    let timeout = fast_img_osxphotos_query_timeout(uuids.len());
    tracing::info!(
        target: "photos_import",
        uuid_count = uuids.len(),
        timeout_secs = timeout.as_secs(),
        "Starting osxphotos query"
    );
    let start = std::time::Instant::now();
    let output = crate::process_runner::ManagedProcess::spawn(&mut command)
        .and_then(|process| process.wait_timeout(timeout, "fast-img osxphotos batch query"))
        .map_err(|e| ImgQualityError::AnalysisError(format!("osxphotos query failed: {e}")))?;
    let elapsed = start.elapsed();

    // Record actual startup time for adaptive timeout (success path)
    record_osxphotos_query_startup_time(elapsed.as_secs());

    tracing::info!(
        target: "photos_import",
        uuid_count = uuids.len(),
        elapsed_secs = elapsed.as_secs(),
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

const FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_ENV: &str = "MFB_FAST_IMG_ICLOUD_VERIFY_ATTEMPTS";
const FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE_ENV: &str = "MFB_FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE";
const FAST_IMG_ICLOUD_VERIFY_DELAY_MS_ENV: &str = "MFB_FAST_IMG_ICLOUD_VERIFY_DELAY_MS";
const FAST_IMG_OSXPHOTOS_QUERY_TIMEOUT_SECS_ENV: &str = "MFB_FAST_IMG_OSXPHOTOS_QUERY_TIMEOUT_SECS";
const FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF_ENV: &str = "MFB_FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF";
const FAST_IMG_PHOTOS_IMPORT_TIMEOUT_SECS_ENV: &str = "MFB_FAST_IMG_PHOTOS_IMPORT_TIMEOUT_SECS";
const FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE_ENV: &str = "MFB_FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE";
const FAST_IMG_ICLOUD_VERIFY_ATTEMPTS_DEFAULT: usize = 3;
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

/// Session-level flag tracking whether osxphotos has been proven responsive.
/// First successful query sets this to true, enabling faster subsequent
/// queries.
static OSXPHOTOS_WARMED_UP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
const FAST_IMG_PHOTOS_IMPORT_BATCH_DELAY_MS: u64 = 2_000;
const FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_INTERVAL: usize = 20;
const FAST_IMG_PHOTOS_IMPORT_DIGEST_PAUSE_SECS: u64 = 30;
const FAST_IMG_PHOTOS_IMPORT_WINDOW_PAUSE_SECS: u64 = 60;
const FAST_IMG_PHOTOS_IMPORT_POISON_RETRY_LIMIT: usize = 1;
#[cfg(all(target_os = "macos", not(test)))]
const FAST_IMG_PHOTOS_IMPORT_RELAUNCH_COMMAND_TIMEOUT_SECS: u64 = 60; // Increased from 30s
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
    report_pairs
        .iter()
        .map(|(rel_path, import_identifier)| {
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

/// Called on timeout to extend the next query's timeout.
fn extend_osxphotos_query_timeout() {
    use std::sync::atomic::Ordering;

    let old = OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::Relaxed);
    // Double the timeout, up to 8 min max
    let new_base = (old * 2).min(480);
    OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(new_base, Ordering::Relaxed);
    tracing::warn!(
        target: "photos_import",
        old_base_secs = old,
        new_base_secs = new_base,
        "osxphotos query timeout, extending adaptive base"
    );
}

fn fast_img_photos_import_timeout() -> Duration {
    fast_img_positive_secs_env(
        FAST_IMG_PHOTOS_IMPORT_TIMEOUT_SECS_ENV,
        Duration::from_mins(30),
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
            let mut probes_by_uuid = BTreeMap::new();
            for probe in query_assets(&query_uuids)? {
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
    crate::common_utils::resolve_tool_path("osascript")
        .unwrap_or_else(|| PathBuf::from("/usr/bin/osascript"))
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
    let output = std::process::Command::new("xattr")
        .arg("-d")
        .arg("com.apple.quarantine")
        .arg(path)
        .output()
        .map_err(|err| {
            ImgQualityError::AnalysisError(format!("xattr quarantine clear spawn failed: {err}"))
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
    let output = std::process::Command::new("xattr")
        .arg("-p")
        .arg("com.apple.quarantine")
        .arg(path)
        .output()
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

fn fast_img_marker_output_paths(marker: &WorkingCopyMarker) -> Vec<(String, PathBuf)> {
    let mut outputs = Vec::new();
    for (source_rel, entry) in &marker.blake3_log {
        let out_rel = marker_entry_out_rel(source_rel, entry);
        outputs.push((out_rel.clone(), marker.working_copy.join(out_rel)));
    }
    outputs.sort_by(|left, right| left.0.cmp(&right.0));
    outputs
}

/// Prompt the user for a yes/no confirmation (§Import confirm gate, GAP-3).
///
/// Reads from stdin. Returns `true` for "y"/"Y", `false` otherwise.
///
/// # Errors
/// Returns an error on I/O failure.
pub fn prompt_user_confirm(message: &str) -> Result<bool> {
    use std::io::{BufRead, Write};
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
        let output_paths = fast_img_marker_output_paths(&marker);

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
    fn photos_import_ids_pair_with_sorted_marker_outputs() -> Result<()> {
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
        let output_paths = fast_img_marker_output_paths(&marker);

        let pairs = fast_img_pairs_from_photos_import_ids(
            &output_paths,
            b"UUID-A\nUUID-B\n",
            marker.src_jpeg_count,
        )?;

        assert_eq!(
            pairs,
            vec![
                ("a.JXL".to_string(), "UUID-A".to_string()),
                ("nested/b.JXL".to_string(), "UUID-B".to_string()),
            ]
        );
        Ok(())
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
        let output_paths = fast_img_marker_output_paths(&marker);

        let entries = fast_img_photos_import_manifest_entries(&marker, &output_paths);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, wc.join("微信/a.JXL"));
        assert_eq!(entries[0].1, "✨/✨Batch/微信");
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
                library_asset: Some(proven_hash),
            },
        );
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

        let mut run_import_batch = |batch_entries: &[(PathBuf, String)]| -> Result<String> {
            assert_eq!(batch_entries.len(), FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP);
            Ok("UUID-f00\n".to_string())
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
        let err = import_pending_jxl_entries_with_checkpoint(
            &mut checkpoint_marker,
            &pending,
            &mut query_assets,
            &mut is_quarantined,
            &mut |_reason: &str| Ok(()),
            &mut run_import_batch,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("Photos AppleScript import returned 1 IDs for 100 JXL outputs"),
            "unexpected err: {err}"
        );
        for idx in 0..FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP {
            let key = format!("f{idx:02}.jpg");
            assert!(
                checkpoint_marker
                    .blake3_log
                    .get(&key)
                    .and_then(|entry| entry.library_asset.as_ref())
                    .is_none(),
                "{key} should remain pending after failed window import"
            );
        }
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
        let err = import_pending_jxl_entries_with_checkpoint(
            &mut marker,
            &pending,
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
    fn photos_import_accepts_out_of_order_import_identifiers_when_hashes_match() -> Result<()> {
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
            assert_eq!(uuids, ["UUID-B".to_string(), "UUID-A".to_string()]);
            Ok(vec![
                FastImgLibraryAssetProbe {
                    uuid: "UUID-B".to_string(),
                    path: library_b.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                },
                FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: library_a.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                },
            ])
        };
        let mut is_quarantined = |_path: &Path| Ok(false);
        let pending = photos_import_checkpoint_plan(&marker, &mut is_quarantined)?.pending_entries;
        let records = import_pending_jxl_entries_with_checkpoint(
            &mut marker,
            &pending,
            &mut query_assets,
            &mut is_quarantined,
            &mut |_reason: &str| Ok(()),
            &mut |_batch_entries: &[(PathBuf, String)]| Ok("UUID-B\nUUID-A\n".to_string()),
        )?;

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].rel_path, "a.JXL");
        assert_eq!(records[0].blake3, hash_a);
        assert_eq!(records[1].rel_path, "b.JXL");
        assert_eq!(records[1].blake3, hash_b);
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
        let records = import_pending_jxl_entries_with_checkpoint(
            &mut marker,
            &pending,
            &mut query_assets,
            &mut is_quarantined,
            &mut prepare_import_session,
            &mut run_import_batch,
        )?;

        assert!(
            prepare_calls.is_empty(),
            "small pending set must avoid relaunch warmup overhead"
        );
        assert_eq!(
            run_batch_sizes,
            vec![FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP]
        );
        assert_eq!(records.len(), total);
        Ok(())
    }

    #[test]
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
        assert_eq!(
            photos_import_batch_sizes_for_strategy(strategy, windows[0].len),
            vec![FAST_IMG_PHOTOS_IMPORT_WINDOW_FILE_CAP]
        );
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
    fn periodic_photos_recovery_timeout_does_not_abort_import_window() {
        let mut recovery_calls = Vec::new();
        let result =
            handle_photos_import_recovery("periodic_window_boundary", &mut |reason: &str| {
                recovery_calls.push(reason.to_string());
                Err(ImgQualityError::AnalysisError(
                    "timed out waiting for Photos process quit state".to_string(),
                ))
            });

        assert!(
            result.is_ok(),
            "periodic relaunch recovery is best-effort because import proof still gates success"
        );
        assert_eq!(recovery_calls, vec!["periodic_window_boundary"]);
    }

    #[test]
    fn poisoned_photos_recovery_timeout_remains_fatal() {
        let result = handle_photos_import_recovery("poisoned_session", &mut |_reason: &str| {
            Err(ImgQualityError::AnalysisError(
                "timed out waiting for Photos process quit state".to_string(),
            ))
        });

        let err = result.expect_err("poisoned Photos session recovery must fail closed");
        assert!(
            err.to_string()
                .contains("timed out waiting for Photos process quit state"),
            "unexpected err: {err}"
        );
    }

    #[test]
    fn photos_import_batch_sizes_use_one_process_per_window() {
        assert_eq!(
            photos_import_batch_sizes_for_strategy(PhotosImportStrategy::StableCheckpointed, 1),
            vec![1]
        );
        assert_eq!(
            photos_import_batch_sizes_for_strategy(PhotosImportStrategy::StableCheckpointed, 11),
            vec![11]
        );
        assert_eq!(
            photos_import_batch_sizes_for_strategy(PhotosImportStrategy::StableCheckpointed, 21),
            vec![21]
        );
    }

    #[test]
    fn photos_import_batch_sizes_use_fast_path_for_small_pending_sets() {
        assert_eq!(photos_import_batch_sizes(2), vec![2]);
        assert_eq!(
            photos_import_batch_sizes(FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP),
            vec![FAST_IMG_PHOTOS_IMPORT_FAST_PATH_FILE_CAP]
        );
        assert_eq!(
            photos_import_batch_sizes(150),
            vec![150],
            "FastSmallSet must process <=150 files in one batch"
        );
    }

    #[test]
    fn photos_import_batch_sizes_keep_stable_path_for_large_pending_sets() {
        assert_eq!(
            photos_import_batch_sizes(151),
            vec![151],
            "StableCheckpointed must use one AppleScript process per import window"
        );
    }

    #[test]
    #[ignore = "macOS-only live Photos smoke test"]
    #[cfg(target_os = "macos")]
    #[serial_test::serial]
    fn photos_import_live_smoke_debug_library() -> Result<()> {
        use std::process::Command;

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| ImgQualityError::AnalysisError("HOME is unset".to_string()))?;
        let input = std::env::var_os("MFB_LIVE_PHOTOS_SMOKE_INPUT").map_or_else(
            || home.join("Downloads/Final 2/Telegram_optimized/IMG_9644.JXL"),
            PathBuf::from,
        );
        let library = std::env::var_os("MFB_LIVE_PHOTOS_SMOKE_LIBRARY")
            .map_or_else(|| home.join("Pictures/debug.photoslibrary"), PathBuf::from);

        if !input.exists() {
            return Err(ImgQualityError::AnalysisError(format!(
                "live Photos smoke input missing: {}",
                input.display()
            )));
        }
        if !library.exists() {
            return Err(ImgQualityError::AnalysisError(format!(
                "live Photos smoke library missing: {}",
                library.display()
            )));
        }

        let open_status = Command::new("open")
            .arg("-a")
            .arg("Photos")
            .arg(&library)
            .status()
            .map_err(|e| {
                ImgQualityError::AnalysisError(format!("open Photos for live smoke failed: {e}"))
            })?;
        if !open_status.success() {
            return Err(ImgQualityError::AnalysisError(
                "open Photos for live smoke returned non-zero".to_string(),
            ));
        }

        std::thread::sleep(Duration::from_secs(3));

        let output =
            run_photos_import_applescript_session("JXL", &[(input, "✨debug-smoke".to_string())])?;
        let ids = output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .count();

        assert_eq!(ids, 1, "unexpected import id output: {output}");
        Ok(())
    }

    #[test]
    fn photos_import_poison_detection_covers_zero_import_invalid_connection_and_timeout() {
        assert!(
            photos_import_poison_reason(
                "execution error: Photos returned 0 imported items for /tmp/a.JXL (-2700)"
            )
            .is_some()
        );
        assert!(
            photos_import_poison_reason("execution error: “Photos”遇到一个错误：连接无效。 (-609)")
                .is_some()
        );
        assert!(
            photos_import_poison_reason(
                "execution error: “Photos”遇到一个错误：AppleEvent已超时。 (-1712)"
            )
            .is_some()
        );
        assert_eq!(
            photos_import_poison_reason(
                "execution error: “Photos”遇到一个错误：AppleEvent已超时。 (-1712)"
            ),
            Some("appleevent_timeout")
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
    fn photos_upload_verifier_defaults_are_low_process_pressure() {
        assert_eq!(fast_img_icloud_upload_verify_attempts(), 3);
        assert!(fast_img_icloud_upload_verify_batch_size() <= 64);
        assert!(fast_img_icloud_upload_verify_delay() >= Duration::from_secs(2));
        assert!(fast_img_photos_import_timeout() <= Duration::from_mins(30));
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
                .contains("final JXL delivery proof or raw roundtrip proof is required"),
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
        let integrity = IntegrityResult::FinalJxlDelivery {
            source_hash,
            output_hash,
        };
        safe_delete_jpeg_source(&src_path, out.path(), &integrity).unwrap();
        assert!(!src_path.exists());
    }

    #[test]
    fn delete_gate_accepts_jxl_pixel_equivalence_integrity() {
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
        safe_delete_jpeg_source(&src_path, out.path(), &integrity).unwrap();
        assert!(!src_path.exists());
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
        };
        let err = safe_delete_modern_lossy_static_source(&missing, &proof).unwrap_err();
        assert!(
            err.to_string().contains("does not exist"),
            "unexpected: {err}"
        );
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
        };
        safe_delete_modern_lossy_static_source(&src_path, &proof).unwrap();
        assert!(!src_path.exists());
    }

    #[test]
    #[serial_test::serial]
    fn adaptive_timeout_extends_on_timeout_call() {
        use std::sync::atomic::Ordering;

        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(120, Ordering::SeqCst);
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            120
        );

        extend_osxphotos_query_timeout();
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            240
        );

        extend_osxphotos_query_timeout();
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            480
        );

        extend_osxphotos_query_timeout();
        assert_eq!(
            OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.load(Ordering::SeqCst),
            480
        );
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

        extend_osxphotos_query_timeout();

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

    #[test]
    #[serial_test::serial]
    fn adaptive_timeout_extreme_library_scenario() {
        use std::sync::atomic::Ordering;

        // Simulate extreme library: 200k+ assets, osxphotos takes 6min to start
        OSXPHOTOS_QUERY_TIMEOUT_BASE_SECS.store(120, Ordering::SeqCst);
        OSXPHOTOS_WARMED_UP.store(false, Ordering::SeqCst);

        // First query with cold start buffer: 120 + 180 = 300s (5min)
        let timeout1 = fast_img_osxphotos_query_timeout(64);
        assert_eq!(timeout1, Duration::from_mins(5)); // Still not enough

        // First attempt times out, extend
        extend_osxphotos_query_timeout();
        // Base now: 240

        // Second query: 240 + 180 = 420s (7min)
        let timeout2 = fast_img_osxphotos_query_timeout(64);
        assert_eq!(timeout2, Duration::from_mins(7));

        // Still times out, extend again
        extend_osxphotos_query_timeout();
        // Base now: 480 (capped)

        // Third query: 480 + 180 = 660s (11min) - exceeds 8min base but cold buffer
        // adds more
        let timeout3 = fast_img_osxphotos_query_timeout(64);
        assert_eq!(timeout3, Duration::from_mins(11));

        // This succeeds after 6min, record it
        record_osxphotos_query_startup_time(360); // 6 minutes
        // new_base = 360+30 = 390, but capped at 480
        // Warmed up now set to true

        // Subsequent queries are much faster without cold buffer
        let timeout4 = fast_img_osxphotos_query_timeout(64);
        assert_eq!(timeout4, Duration::from_mins(8)); // Just base, no cold buffer
    }
}
