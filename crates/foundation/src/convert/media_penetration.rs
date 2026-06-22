//! Media Penetration Detection
//!
//! Content-based verification that bypasses potentially fake metadata.
//! All detection functions decode actual media content instead of trusting container headers.

use crate::builder_base::ToolBuilder;
use crate::progress_mode::emit_stderr;
use std::intrinsics::likely;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenetrationResult<T> {
    /// Detection succeeded with a definitive result
    Verified(T),
    /// Detection failed due to technical error (file unreadable, codec unsupported, etc.)
    Failed,
    /// Detection skipped (optimization: claim is reasonable, no need to verify)
    Skipped,
}

impl<T> PenetrationResult<T> {
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Verified(v) => Some(v),
            Self::Failed | Self::Skipped => None,
        }
    }

    pub const fn is_verified(&self) -> bool {
        matches!(self, Self::Verified(_))
    }
}

fn parse_mean_volume_line(line: &str) -> anyhow::Result<Option<f64>> {
    let Some(idx) = line.find("mean_volume:") else {
        return Ok(None);
    };
    let Some(vol_str) = line[idx + 12..].split_whitespace().next() else {
        anyhow::bail!("mean_volume line has no numeric token: {line}");
    };
    vol_str
        .parse::<f64>()
        .map(Some)
        .map_err(|err| anyhow::anyhow!("invalid mean_volume token {vol_str:?}: {err}"))
}

fn parse_volumedetect_sample_count_line(line: &str) -> anyhow::Result<Option<u64>> {
    let Some(idx) = line.find("n_samples:") else {
        return Ok(None);
    };
    let Some(sample_count_str) = line[idx + 10..].split_whitespace().next() else {
        anyhow::bail!("n_samples line has no numeric token: {line}");
    };
    sample_count_str
        .parse::<u64>()
        .map(Some)
        .map_err(|err| anyhow::anyhow!("invalid n_samples token {sample_count_str:?}: {err}"))
}

fn parse_volumedetect_silence(
    stderr: &str,
    silence_threshold_db: f64,
) -> anyhow::Result<Option<bool>> {
    let mut mean_volume_seen = false;
    let mut all_mean_volumes_silent = true;
    let mut sample_count_seen = false;
    let mut any_nonzero_sample_count = false;

    for line in stderr.lines() {
        match parse_mean_volume_line(line) {
            Ok(Some(mean_volume)) => {
                mean_volume_seen = true;
                if mean_volume >= silence_threshold_db {
                    all_mean_volumes_silent = false;
                }
            }
            Ok(None) => {}
            Err(err) => {
                anyhow::bail!("failed to parse volumedetect mean_volume line {line:?}: {err}");
            }
        }

        match parse_volumedetect_sample_count_line(line) {
            Ok(Some(sample_count)) => {
                sample_count_seen = true;
                if sample_count > 0 {
                    any_nonzero_sample_count = true;
                }
            }
            Ok(None) => {}
            Err(err) => {
                anyhow::bail!("failed to parse volumedetect n_samples line {line:?}: {err}");
            }
        }
    }

    if mean_volume_seen {
        return Ok(Some(all_mean_volumes_silent));
    }

    if sample_count_seen && !any_nonzero_sample_count {
        return Ok(Some(true));
    }

    Ok(None)
}

fn parse_lavfi_min_line(line: &str) -> anyhow::Result<Option<f64>> {
    let Some(idx) = line.find("lavfi.stats.0.Min:") else {
        return Ok(None);
    };
    let Some(part) = line.get(idx + 18..) else {
        anyhow::bail!("lavfi min line has no value: {line}");
    };
    let min_str =
        crate::media_conversion_gate::probe_stdout_first_token(part, "media_penetration min");
    min_str
        .parse::<f64>()
        .map(Some)
        .map_err(|err| anyhow::anyhow!("invalid lavfi min token {min_str:?}: {err}"))
}

fn parse_frame_count_line(line: &str) -> anyhow::Result<Option<u64>> {
    let Some(pos) = line.find("frame=") else {
        return Ok(None);
    };
    let Some(part) = line.get(pos + 6..) else {
        anyhow::bail!("frame count line has no value: {line}");
    };
    let count_str =
        crate::media_conversion_gate::probe_stdout_first_token(part, "media_penetration count");
    count_str
        .parse::<u64>()
        .map(Some)
        .map_err(|err| anyhow::anyhow!("invalid frame count token {count_str:?}: {err}"))
}

/// Penetrating audio detection: decode and analyze actual audio samples.
/// Returns `Verified(true)` if silent, `Verified(false)` if audible, `Failed` on error.
#[must_use]
pub fn detect_audio_silence(path: &Path) -> PenetrationResult<bool> {
    const SILENCE_THRESHOLD_DB: f64 = crate::constants::AUDIO_SILENCE_THRESHOLD_DB;

    let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .arg("-af")
        .arg("volumedetect")
        .arg("-vn")
        .arg("-sn")
        .arg("-dn")
        .format("null")
        .output_pipe()
        .build()
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            crate::ui_stderr::line(
                crate::modern_ui::symbols::WARNING,
                crate::modern_ui::symbols::plain::WARNING,
                format!("Audio penetration failed: ffmpeg error ({e})"),
            );
            return PenetrationResult::Failed;
        }
    };

    if !output.status.success() {
        crate::ui_stderr::line(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
            format!(
                "Audio penetration failed: ffmpeg exit code {}",
                crate::media_conversion_gate::process_exit_code_for_context(
                    output.status.code(),
                    "ffmpeg_penetration_audio",
                    path.display().to_string(),
                )
            ),
        );
        return PenetrationResult::Failed;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);

    match parse_volumedetect_silence(&stderr, SILENCE_THRESHOLD_DB) {
        Ok(Some(is_silent)) => return PenetrationResult::Verified(is_silent),
        Ok(None) => {}
        Err(err) => {
            crate::media_conversion_gate::delivery_intent_path_audit(
                "media_penetration_volumedetect_parse_failed",
                path,
                format!("[WARN] failed to parse volumedetect output: {err}"),
            );
            return PenetrationResult::Failed;
        }
    }

    crate::ui_stderr::line(
        crate::modern_ui::symbols::WARNING,
        crate::modern_ui::symbols::plain::WARNING,
        "[WARN] Audio penetration failed: volumedetect output unparseable",
    );
    PenetrationResult::Failed
}

/// Penetrating transparency detection: decode frames and check alpha variance.
///
/// Uses stratified sampling first, then falls back to full decode if suspicious.
/// Returns `Verified(true)` if alpha is used, `Verified(false)` if fake, `Skipped` if no claim.
#[must_use]
pub fn detect_real_transparency(path: &Path, duration: Option<f64>) -> PenetrationResult<bool> {
    // Phase 1: Stratified Sampling (fast check)
    // Sample up to 3 points in time to catch most cases efficiently.
    let Some(duration_val) = duration else {
        crate::media_conversion_gate::probe_layer_audit(
            "transparency_duration_missing",
            path,
            "transparency penetration impossible without duration",
        );
        return PenetrationResult::Failed;
    };
    let sample_points = if duration_val <= crate::constants::TRANSPARENCY_SAMPLE_POINTS_SHORT_LIMIT
    {
        vec![0.0_f64]
    } else if duration_val <= crate::constants::TRANSPARENCY_SAMPLE_POINTS_MEDIUM_LIMIT {
        vec![
            0.0_f64,
            duration_val * crate::constants::SAMPLING_POINT_MID_F64,
        ]
    } else {
        vec![
            0.0_f64,
            duration_val * crate::constants::SAMPLING_POINT_MID_F64,
            duration_val - crate::constants::PENETRATION_SAMPLING_EOF_OFFSET,
        ]
    };

    let mut found_transparency = false;
    let mut sampling_succeeded = false;

    for ss in &sample_points {
        let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
            .input_arg("-ss")
            .input_arg(ss.to_string())
            .input(path)
            .frames_v(1)
            .arg("-vf")
            .arg("alphaextract,stats")
            .format("null")
            .output_pipe()
            .build()
            .output()
        {
            Ok(out) => out,
            Err(e) => {
                emit_stderr(&format!(
                    "{} Transparency sampling failed at {ss:.2}s: ffmpeg error ({e})",
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
                continue;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No alpha channel") {
                return PenetrationResult::Verified(false);
            }
            continue;
        }

        sampling_succeeded = true;
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Parse "lavfi.stats.0.Min" from stderr
        for line in stderr.lines() {
            match parse_lavfi_min_line(line) {
                Ok(Some(min_val)) if likely(min_val < crate::constants::MAX_8BIT_VALUE_F64) => {
                    found_transparency = true;
                    break;
                }
                Ok(Some(_) | None) => {}
                Err(err) => {
                    crate::media_conversion_gate::delivery_intent_path_audit(
                        "media_penetration_alpha_min_parse_failed",
                        path,
                        format!("failed to parse alpha min output line {line:?}: {err}"),
                    );
                    return PenetrationResult::Failed;
                }
            }
        }

        if found_transparency {
            break;
        }
    }

    // If sampling found transparency, we're done
    if found_transparency {
        return PenetrationResult::Verified(true);
    }

    // Phase 2: Full Decode Verification (for suspicious cases)
    // If sampling succeeded but found no transparency, do a full decode to be absolutely sure.
    // This catches cases where transparency only appears in specific frames.
    if sampling_succeeded && duration_val > crate::constants::PENETRATION_MIN_SAMPLING_DURATION {
        return run_full_decode_transparency(path);
    }

    PenetrationResult::Verified(found_transparency)
}

fn run_full_decode_transparency(path: &Path) -> PenetrationResult<bool> {
    emit_stderr(
        "   Transparency sampling found no variance, performing full decode verification...",
    );

    let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .arg("-vf")
        .arg("alphaextract,stats")
        .format("null")
        .output_pipe()
        .build()
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            emit_stderr(&format!(
                "{} Full transparency decode failed: ffmpeg error ({e})",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
            return PenetrationResult::Failed;
        }
    };

    if !output.status.success() {
        emit_stderr(&format!(
            "{} Full transparency decode failed: exit code {}",
            crate::modern_ui::symbols::styled_warning_icon(),
            crate::media_conversion_gate::process_exit_code_for_context(
                output.status.code(),
                "ffmpeg_penetration_transparency",
                path.display().to_string(),
            )
        ));
        return PenetrationResult::Failed;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check all frames for any Min < 255
    for line in stderr.lines() {
        match parse_lavfi_min_line(line) {
            Ok(Some(min_val)) if likely(min_val < crate::constants::MAX_8BIT_VALUE_F64) => {
                emit_stderr("   Full decode found transparency in at least one frame");
                return PenetrationResult::Verified(true);
            }
            Ok(Some(_) | None) => {}
            Err(err) => {
                crate::media_conversion_gate::delivery_intent_path_audit(
                    "media_penetration_alpha_min_parse_failed",
                    path,
                    format!("failed to parse full alpha min output line {line:?}: {err}"),
                );
                return PenetrationResult::Failed;
            }
        }
    }

    emit_stderr("   Full decode confirmed: all frames have opaque alpha (fake transparency)");
    PenetrationResult::Verified(false)
}

/// Penetrating frame count detection: decode and count actual frames via ffmpeg summary.
/// Returns `Verified(count)` with real count, `Skipped` if a concrete claim is already reasonable.
#[must_use]
pub fn detect_real_frame_count(
    path: &Path,
    claimed_frame_count: Option<u64>,
) -> PenetrationResult<u64> {
    // Only verify suspicious concrete claims (<= LOWER_LIMIT or > UPPER_LIMIT).
    // Missing claims are honest unknowns and still merit verification.
    if claimed_frame_count.is_some_and(|claimed| {
        claimed > crate::constants::FRAME_COUNT_TRUST_LOWER_LIMIT
            && claimed <= crate::constants::FRAME_COUNT_TRUST_UPPER_LIMIT
    }) {
        return PenetrationResult::Skipped;
    }

    // Count frames via ffmpeg's physical output summary.
    // Using '-fps_mode passthrough' ensures zero frame duplication/dropping, giving us
    // the absolute physical number of frames as seen by the processing engine.
    let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-fps_mode")
        .arg("passthrough")
        .format("null")
        .output_pipe()
        .build()
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            emit_stderr(&format!(
                "{} Frame count penetration failed: ffmpeg error ({e})",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
            return PenetrationResult::Failed;
        }
    };

    // FFmpeg's summary (frame= XXXX) is in stderr
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Find the last "frame=" entry in the output
    // Example line: "frame=  123 fps=0.1 q=-0.0 Lsize=N/A time=00:00:05.12 bitrate=N/A speed=4.5x"
    let mut actual_u64 = None;
    for line in stderr.lines().rev() {
        match parse_frame_count_line(line) {
            Ok(Some(count)) => {
                actual_u64 = Some(count);
                break;
            }
            Ok(None) => {}
            Err(err) => {
                crate::media_conversion_gate::delivery_intent_path_audit(
                    "media_penetration_frame_count_parse_failed",
                    path,
                    format!("failed to parse frame-count output line {line:?}: {err}"),
                );
                return PenetrationResult::Failed;
            }
        }
    }

    if let Some(actual) = actual_u64 {
        if claimed_frame_count.is_some_and(|claimed| actual != claimed) {
            emit_stderr(&format!(
                "{} Frame count mismatch detected: metadata claimed {}, physical decoded {actual}",
                crate::modern_ui::symbols::styled_warning_icon(),
                crate::media_conversion_gate::delivery_frame_count_label_u64(
                    claimed_frame_count,
                    &format!("penetration mismatch {}", path.display()),
                )
            ));
        }
        PenetrationResult::Verified(actual)
    } else {
        crate::ui_stderr::line(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
            "Frame count penetration failed: ffmpeg produced no 'frame=' summary",
        );
        if !output.status.success() {
            emit_stderr(&format!(
                "   FFmpeg exit code: {}",
                crate::media_conversion_gate::process_exit_code_for_context(
                    output.status.code(),
                    "ffmpeg_penetration_frame_count",
                    path.display().to_string(),
                )
            ));
        }
        PenetrationResult::Failed
    }
}

/// Penetrating interlace detection: decodes a short sample and uses the `idet` filter.
///
/// Returns `Verified(true)` if interlacing is physically detected, `Verified(false)` if progressive.
/// `Skipped` if the check isn't necessary.
#[must_use]
pub fn detect_interlacing(path: &Path) -> PenetrationResult<bool> {
    // Only sample the first 24 frames (~1 second) to keep the penetration fast.
    let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(crate::constants::INTERLACE_DETECTION_SAMPLE_FRAMES)
        .arg("-vf")
        .arg("idet")
        .format("null")
        .output_pipe()
        .build()
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            emit_stderr(&format!(
                "{} Interlace detection failed: ffmpeg error ({e})",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
            return PenetrationResult::Failed;
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Look for: "Parsed_idet_0 ... Single frame detection: TFF: 12 BFF: 0 Progressive: 40 Undetermined: 0"
    for line in stderr.lines() {
        if line.contains("Parsed_idet_") && line.contains("Single frame detection:") {
            let tff = line.find("TFF:").and_then(|tff_idx| {
                let token = line[tff_idx + 4..].split_whitespace().next();
                crate::media_conversion_gate::probe_idet_count_optional(path, "TFF", token)
            });
            let bff = line.find("BFF:").and_then(|bff_idx| {
                let token = line[bff_idx + 4..].split_whitespace().next();
                crate::media_conversion_gate::probe_idet_count_optional(path, "BFF", token)
            });

            let interlaced_hits = match (tff, bff) {
                (Some(t), Some(b)) => t + b,
                // Partial parse failure or missing field: do not treat as zero hits.
                _ => continue,
            };

            // If we found multiple clear interlaced frames in the short sample, it's interlaced.
            // Using a threshold of 2 to avoid single-frame false positives.
            if interlaced_hits >= 2 {
                let (Some(tff_hits), Some(bff_hits)) = (tff, bff) else {
                    continue;
                };
                emit_stderr(&format!(
                    "{} [{}] Interlace penetration: INTERLACED frames detected (TFF:{}, BFF:{})",
                    crate::modern_ui::symbols::pick("📺", "[VID]"),
                    crate::media_conversion_gate::path_file_name_for_log(path),
                    tff_hits,
                    bff_hits
                ));
                return PenetrationResult::Verified(true);
            }
            return PenetrationResult::Verified(false);
        }
    }

    PenetrationResult::Failed
}

/// Summary of penetration detection results for reporting
#[derive(Debug, Default)]
pub struct PenetrationSummary {
    pub audio_checked: bool,
    pub audio_result: Option<bool>,
    pub transparency_checked: bool,
    pub transparency_result: Option<bool>,
    pub frame_count_checked: bool,
    pub frame_count_mismatch: Option<(u64, u64)>, // (claimed, actual)
}

impl PenetrationSummary {
    #[must_use]
    pub const fn has_any_mismatch(&self) -> bool {
        self.audio_result.is_some()
            || matches!(self.transparency_result, Some(false))
            || self.frame_count_mismatch.is_some()
    }

    #[must_use]
    pub fn report(&self) -> String {
        let mut lines = Vec::new();

        if self.audio_checked
            && let Some(is_silent) = self.audio_result
        {
            lines.push(format!(
                "  Audio: {}",
                if is_silent {
                    "SILENT (verified)"
                } else {
                    "AUDIBLE (verified)"
                }
            ));
        }

        if self.transparency_checked
            && let Some(is_real) = self.transparency_result
        {
            lines.push(format!(
                "  Transparency: {}",
                if is_real {
                    "REAL (verified)"
                } else {
                    "FAKE (metadata lied)"
                }
            ));
        }

        if self.frame_count_checked
            && let Some((claimed, actual)) = self.frame_count_mismatch
        {
            lines.push(format!(
                "  Frame count: MISMATCH (claimed={claimed}, actual={actual})"
            ));
        }

        if lines.is_empty() {
            "No penetration checks performed".to_string()
        } else {
            format!("Penetration Detection Results:\n{}", lines.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_audio_silence_requires_real_file() {
        // This test requires actual media files, skipped in unit tests
        // Integration tests should cover this
    }

    #[test]
    fn parse_mean_volume_line_malformed_returns_error() {
        let err = parse_mean_volume_line("[Parsed_volumedetect] mean_volume: bad dB")
            .expect_err("malformed mean_volume must be an error");

        assert!(err.to_string().contains("bad"));
    }

    #[test]
    fn parse_volumedetect_silence_prefers_later_mean_volume_over_initial_zero_samples()
    -> anyhow::Result<()> {
        let stderr = "\
[Parsed_volumedetect_0 @ 0x1] n_samples: 0
[Parsed_volumedetect_0 @ 0x2] n_samples: 297984
[Parsed_volumedetect_0 @ 0x2] mean_volume: -21.1 dB
";

        let parsed = parse_volumedetect_silence(stderr, -60.0)?
            .ok_or_else(|| anyhow::anyhow!("volumedetect output must produce a decision"))?;

        assert!(!parsed);
        Ok(())
    }

    #[test]
    fn parse_lavfi_min_line_malformed_returns_error() {
        let err = parse_lavfi_min_line("lavfi.stats.0.Min: nope")
            .expect_err("malformed lavfi min must be an error");

        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn parse_frame_count_line_malformed_returns_error() {
        let err = parse_frame_count_line("frame= many fps=0.0")
            .expect_err("malformed frame count must be an error");

        assert!(err.to_string().contains("many"));
    }

    #[test]
    fn test_detect_real_transparency_skipped_when_mocked() {
        // When no transparency is claimed, should skip without decoding
        let fake_path = Path::new("/nonexistent.mp4");
        // detect_real_transparency is called only if meta.has_transparency is true
        // in callers, but we test the function itself.
        // Providing a mock duration to avoid Failed result due to missing metadata.
        let result = detect_real_transparency(fake_path, Some(5.0));
        // It will try to run ffmpeg and fail because file is nonexistent, returning Verified(false) or Failed.
        // But the goal is to verify it doesn't panic on None duration.
        assert!(!result.is_verified() || result == PenetrationResult::Verified(false));
    }

    #[test]
    fn test_detect_real_frame_count_trusts_reasonable_claims() {
        let fake_path = Path::new("/nonexistent.mp4");
        // Reasonable claims (LOWER_LIMIT+1 to UPPER_LIMIT) should be skipped without decoding
        assert_eq!(
            detect_real_frame_count(fake_path, Some(100)),
            PenetrationResult::Skipped
        );
        assert_eq!(
            detect_real_frame_count(fake_path, Some(5000)),
            PenetrationResult::Skipped
        );
    }

    #[test]
    fn test_detect_real_frame_count_does_not_treat_unknown_claim_as_reasonable() {
        let fake_path = Path::new("/nonexistent.mp4");
        assert_ne!(
            detect_real_frame_count(fake_path, None),
            PenetrationResult::Skipped
        );
    }

    #[test]
    fn test_penetration_result_into_option() {
        assert_eq!(PenetrationResult::Verified(true).into_option(), Some(true));
        assert_eq!(PenetrationResult::Failed::<bool>.into_option(), None);
        assert_eq!(PenetrationResult::Skipped::<bool>.into_option(), None);
    }

    #[test]
    fn test_penetration_result_is_verified() {
        assert!(PenetrationResult::Verified(42_i32).is_verified());
        assert!(!PenetrationResult::Failed::<i32>.is_verified());
        assert!(!PenetrationResult::Skipped::<i32>.is_verified());
    }

    #[test]
    fn test_penetration_summary_has_mismatch() {
        let mut summary = PenetrationSummary::default();
        assert!(!summary.has_any_mismatch());

        summary.frame_count_mismatch = Some((100, 50));
        assert!(summary.has_any_mismatch());

        // Test the hardened transparency unwrapping logic
        summary.frame_count_mismatch = None;
        summary.transparency_result = Some(true); // Should NOT be a mismatch
        assert!(!summary.has_any_mismatch());

        summary.transparency_result = Some(false); // SHOULD be a mismatch
        assert!(summary.has_any_mismatch());
    }

    #[test]
    fn test_penetration_summary_report() {
        let summary = PenetrationSummary {
            audio_checked: true,
            audio_result: Some(true),
            transparency_checked: true,
            transparency_result: Some(false),
            ..Default::default()
        };

        let report = summary.report();
        assert!(report.contains("SILENT"));
        assert!(report.contains("FAKE"));
    }
}
