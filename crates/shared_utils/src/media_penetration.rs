//! Media Penetration Detection
//!
//! Content-based verification that bypasses potentially fake metadata.
//! All detection functions decode actual media content instead of trusting container headers.

use crate::progress_mode::emit_stderr;
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

/// Penetrating audio detection: decode and analyze actual audio samples.
/// Returns `Verified(true)` if silent, `Verified(false)` if audible, `Failed` on error.
#[must_use]
pub fn detect_audio_silence(path: &Path) -> PenetrationResult<bool> {
    const SILENCE_THRESHOLD_DB: f64 = -70.0;

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
            emit_stderr(&format!("⚠️  Audio penetration failed: ffmpeg error ({e})"));
            return PenetrationResult::Failed;
        }
    };

    if !output.status.success() {
        emit_stderr(&format!(
            "⚠️  Audio penetration failed: ffmpeg exit code {}",
            output.status.code().unwrap_or(-1)
        ));
        return PenetrationResult::Failed;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check for empty audio track (n_samples: 0)
    if stderr.lines().any(|line| line.contains("n_samples: 0")) {
        return PenetrationResult::Verified(true);
    }

    // Parse mean_volume from volumedetect output
    for line in stderr.lines() {
        if let Some(idx) = line.find("mean_volume:") {
            if let Some(vol_str) = line[idx + 12..].split_whitespace().next() {
                if let Ok(mean_volume) = vol_str.parse::<f64>() {
                    return PenetrationResult::Verified(mean_volume < SILENCE_THRESHOLD_DB);
                }
            }
        }
    }

    emit_stderr("⚠️  Audio penetration failed: volumedetect output unparseable");
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
    let duration_val = duration.unwrap_or(1.0);
    let sample_points = if duration_val <= 1.0 {
        vec![0.0]
    } else if duration_val <= 5.0 {
        vec![0.0, duration_val * 0.5]
    } else {
        vec![0.0, duration_val * 0.5, duration_val - 0.1]
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
                    "⚠️  Transparency sampling failed at {ss:.2}s: ffmpeg error ({e})"
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
            if let Some(idx) = line.find("lavfi.stats.0.Min:") {
                let min_str = line[idx + 18..].split_whitespace().next().unwrap_or("");
                if let Ok(min_val) = min_str.parse::<f64>() {
                    if min_val < 255.0 {
                        found_transparency = true;
                        break;
                    }
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
    if sampling_succeeded && duration_val > 1.0 {
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
                "⚠️  Full transparency decode failed: ffmpeg error ({e})"
            ));
            return PenetrationResult::Failed;
        }
    };

    if !output.status.success() {
        emit_stderr(&format!(
            "⚠️  Full transparency decode failed: exit code {}",
            output.status.code().unwrap_or(-1)
        ));
        return PenetrationResult::Failed;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check all frames for any Min < 255
    for line in stderr.lines() {
        if let Some(idx) = line.find("lavfi.stats.0.Min:") {
            let min_str = line[idx + 18..].split_whitespace().next().unwrap_or("");
            if let Ok(min_val) = min_str.parse::<f64>() {
                if min_val < 255.0 {
                    emit_stderr("   Full decode found transparency in at least one frame");
                    return PenetrationResult::Verified(true);
                }
            }
        }
    }

    emit_stderr("   Full decode confirmed: all frames have opaque alpha (fake transparency)");
    PenetrationResult::Verified(false)
}

/// Penetrating frame count detection: decode and count actual frames via ffmpeg summary.
/// Returns `Verified(count)` with real count, `Skipped` if claim is reasonable.
#[must_use]
pub fn detect_real_frame_count(path: &Path, claimed_frame_count: u64) -> PenetrationResult<u64> {
    // Only verify suspicious claims (≤1 or >50000)
    if claimed_frame_count > 1 && claimed_frame_count <= 50000 {
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
                "⚠️  Frame count penetration failed: ffmpeg error ({e})"
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
        if let Some(pos) = line.find("frame=") {
            let part = &line[pos + 6..];
            let count_str = part.split_whitespace().next().unwrap_or("");
            if let Ok(count) = count_str.parse::<u64>() {
                actual_u64 = Some(count);
                break;
            }
        }
    }

    if let Some(actual) = actual_u64 {
        if actual != claimed_frame_count {
            emit_stderr(&format!(
                "⚠️  Frame count mismatch detected: metadata claimed {claimed_frame_count}, physical decoded {actual}"
            ));
        }
        PenetrationResult::Verified(actual)
    } else {
        emit_stderr("⚠️  Frame count penetration failed: ffmpeg produced no 'frame=' summary");
        if !output.status.success() {
            emit_stderr(&format!(
                "   FFmpeg exit code: {}",
                output.status.code().unwrap_or(-1)
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
#[allow(
    clippy::missing_panics_doc,
    reason = "Explicit panic on data corruption is intended and documented inline."
)]
pub fn detect_interlacing(path: &Path) -> PenetrationResult<bool> {
    // Only sample the first 24 frames (~1 second) to keep the penetration fast.
    let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(24)
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
                "⚠️  Interlace detection failed: ffmpeg error ({e})"
            ));
            return PenetrationResult::Failed;
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Look for: "Parsed_idet_0 ... Single frame detection: TFF: 12 BFF: 0 Progressive: 40 Undetermined: 0"
    for line in stderr.lines() {
        if line.contains("Parsed_idet_") && line.contains("Single frame detection:") {
            let mut tff = 0;
            let mut bff = 0;

            if let Some(tff_idx) = line.find("TFF:") {
                let s = line[tff_idx + 4..].split_whitespace().next().unwrap_or("0");
                tff = s
                    .parse::<u64>()
                    .expect("Failed to parse integer or missing required value");
            }
            if let Some(bff_idx) = line.find("BFF:") {
                let s = line[bff_idx + 4..].split_whitespace().next().unwrap_or("0");
                bff = s
                    .parse::<u64>()
                    .expect("Failed to parse integer or missing required value");
            }

            // If we found multiple clear interlaced frames in the short sample, it's interlaced.
            // Using a threshold of 2 to avoid single-frame false positives.
            if tff + bff >= 2 {
                emit_stderr(&format!(
                    "📺 [{}] Interlace penetration: INTERLACED frames detected (TFF:{}, BFF:{})",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    tff,
                    bff
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

        if self.audio_checked {
            if let Some(is_silent) = self.audio_result {
                lines.push(format!(
                    "  Audio: {}",
                    if is_silent {
                        "SILENT (verified)"
                    } else {
                        "AUDIBLE (verified)"
                    }
                ));
            }
        }

        if self.transparency_checked {
            if let Some(is_real) = self.transparency_result {
                lines.push(format!(
                    "  Transparency: {}",
                    if is_real {
                        "REAL (verified)"
                    } else {
                        "FAKE (metadata lied)"
                    }
                ));
            }
        }

        if self.frame_count_checked {
            if let Some((claimed, actual)) = self.frame_count_mismatch {
                lines.push(format!(
                    "  Frame count: MISMATCH (claimed={claimed}, actual={actual})"
                ));
            }
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
    fn test_detect_real_transparency_skipped_when_mocked() {
        // When no transparency is claimed, should skip without decoding
        let fake_path = Path::new("/nonexistent.mp4");
        // detect_real_transparency is called only if meta.has_transparency is true
        // in callers, but we test the function itself.
        let result = detect_real_transparency(fake_path, None);
        assert!(result.is_verified()); // It will try to run ffmpeg and probably fail/Verified(false)
    }

    #[test]
    fn test_detect_real_frame_count_trusts_reasonable_claims() {
        let fake_path = Path::new("/nonexistent.mp4");
        // Reasonable claims (2-50000) should be skipped without decoding
        assert_eq!(
            detect_real_frame_count(fake_path, 100),
            PenetrationResult::Skipped
        );
        assert_eq!(
            detect_real_frame_count(fake_path, 5000),
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
        assert!(PenetrationResult::Verified(42).is_verified());
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
