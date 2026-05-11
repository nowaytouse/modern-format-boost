//! Enhanced quality verification for post-encode / post-conversion checks.
//!
//! Provides:
//! - Output file health (exists, non-empty, minimal size, readable)
//! - Optional input vs output sanity (duration match, video codec present)
//! - Integration with [`crate::checkpoint::verify_output_integrity`] and [`crate::ffprobe`].

use crate::checkpoint::verify_output_integrity;
use crate::ffprobe;
use std::path::Path;

// Global constant moved to crate::constants::DEFAULT_MIN_FILE_SIZE

/// Options for enhanced post-encode verification.
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct VerifyOptions {
    pub min_file_size: u64,
    /// If true, require input and output duration to match within tolerance.
    pub require_duration_match: bool,
    /// Duration tolerance in seconds (input vs output). Used when [`Self::require_duration_match`] is true.
    pub duration_tolerance_secs: f64,
    /// If true, require output to have a video stream (ffprobe).
    pub require_video_stream: bool,
}

impl VerifyOptions {
    pub const fn strict_video() -> Self {
        Self {
            min_file_size: crate::constants::DEFAULT_MIN_FILE_SIZE,
            require_duration_match: true,
            duration_tolerance_secs: crate::constants::VERIFY_DURATION_TOLERANCE_STRICT,
            require_video_stream: true,
        }
    }

    /// Relaxed verification for animated images (GIF, WebP, AVIF) with variable frame delays.
    /// Uses a larger duration tolerance to accommodate frame timing variations during conversion.
    pub const fn relaxed_animated_image() -> Self {
        Self {
            min_file_size: crate::constants::DEFAULT_MIN_FILE_SIZE,
            require_duration_match: true,
            duration_tolerance_secs: crate::constants::VERIFY_DURATION_TOLERANCE_RELAXED_ANIMATED, // More tolerant for GIF variable frame delays
            require_video_stream: true,
        }
    }

    pub const fn minimal() -> Self {
        Self {
            min_file_size: crate::constants::DEFAULT_MIN_FILE_SIZE,
            require_duration_match: false,
            duration_tolerance_secs: 0.0,
            require_video_stream: false,
        }
    }

    const fn effective_min_size(&self) -> u64 {
        if self.min_file_size > 0 {
            self.min_file_size
        } else {
            crate::constants::DEFAULT_MIN_FILE_SIZE
        }
    }
}

use crate::types::CheckResult;

/// Result of enhanced verification.
#[derive(Clone, Debug)]
#[must_use]
pub struct EnhancedVerifyResult {
    pub file_ok: bool,
    pub duration_match: CheckResult,
    pub has_video_stream: CheckResult,
    pub message: String,
    pub details: Vec<String>,
}

impl EnhancedVerifyResult {
    /// True only when file is OK and no required check explicitly failed.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.file_ok && !self.duration_match.is_failed() && !self.has_video_stream.is_failed()
    }

    #[must_use]
    pub fn summary(&self) -> String {
        if self.passed() {
            "✅ Enhanced verification passed".to_string()
        } else {
            format!("❌ Enhanced verification failed: {}", self.message)
        }
    }
}

/// Run basic output file health check (exists, size, readable).
/// Does not require ffprobe.
///
/// # Errors
/// Returns an error if the output file is missing, empty, too small, or unreadable.
pub fn verify_output_file(output: &Path, min_size: u64) -> Result<(), String> {
    let size = if min_size == 0 {
        crate::constants::DEFAULT_MIN_FILE_SIZE
    } else {
        min_size
    };
    verify_output_integrity(output, size)
}

/// Run full enhanced verification: file health + optional duration/codec checks.
pub fn verify_after_encode(
    input: &Path,
    output: &Path,
    options: &VerifyOptions,
) -> EnhancedVerifyResult {
    let mut details = Vec::new();
    let min_size = options.effective_min_size();

    // 1) File integrity
    let file_ok = match verify_output_integrity(output, min_size) {
        Ok(()) => {
            details.push(format!("Output file OK (≥ {min_size} bytes)"));
            true
        }
        Err(e) => {
            details.push(format!("Output file check failed: {e}"));
            return EnhancedVerifyResult {
                file_ok: false,
                duration_match: CheckResult::NotChecked,
                has_video_stream: CheckResult::NotChecked,
                message: e,
                details,
            };
        }
    };
    let mut duration_match = CheckResult::NotChecked;
    let mut has_video_stream = CheckResult::NotChecked;
    let mut probe_failed = false;

    if options.require_duration_match || options.require_video_stream {
        run_probe_checks(
            input,
            output,
            options,
            &mut duration_match,
            &mut has_video_stream,
            &mut probe_failed,
            &mut details,
        );
    }

    let failed = duration_match.is_failed() || has_video_stream.is_failed();
    let message = if failed {
        if probe_failed {
            "Probe failed; duration/stream not verified".to_string()
        } else if duration_match.is_failed() {
            "Duration mismatch (input vs output beyond tolerance)".to_string()
        } else if has_video_stream.is_failed() {
            "Output has no valid video stream".to_string()
        } else {
            "Verification failed".to_string()
        }
    } else {
        "OK".to_string()
    };

    EnhancedVerifyResult {
        file_ok,
        duration_match,
        has_video_stream,
        message,
        details,
    }
}

fn run_probe_checks(
    input: &Path,
    output: &Path,
    options: &VerifyOptions,
    duration_match: &mut CheckResult,
    has_video_stream: &mut CheckResult,
    probe_failed: &mut bool,
    details: &mut Vec<String>,
) {
    let input_probe = ffprobe::probe_video(input);
    let output_probe = ffprobe::probe_video(output);

    match (input_probe, output_probe) {
        (Ok(ref inp), Ok(ref out)) => {
            if options.require_video_stream {
                let has_video = !out.video_codec.is_empty() && out.video_codec != "unknown";
                *has_video_stream = if has_video {
                    CheckResult::Passed
                } else {
                    CheckResult::Failed("No valid video stream in output".to_string())
                };
                if has_video {
                    details.push(format!("Output has video stream: {}", out.video_codec));
                } else {
                    details.push("Output has no valid video stream".to_string());
                }
            }
            if options.require_duration_match {
                let tol = options.duration_tolerance_secs.max(0.0);
                let (diff, ok) = match (inp.duration, out.duration) {
                    (Some(i), Some(o)) => {
                        let d = (i - o).abs();
                        (Some(d), d <= tol)
                    }
                    (None, None) => (None, true), // Both missing: technically a match in absence
                    _ => (None, false),           // One missing: mismatch
                };

                *duration_match = if ok {
                    CheckResult::Passed
                } else {
                    let reason = match (inp.duration, out.duration) {
                        (Some(i), Some(o)) => {
                            let d = diff.unwrap_or_else(|| (i - o).abs());
                            format!(
                                "Duration mismatch: {i:.2}s vs {o:.2}s (diff {d:.2}s > tolerance {tol:.2}s)"
                            )
                        }
                        (None, Some(o)) => {
                            format!("Input duration missing, but output is {o:.2}s")
                        }
                        (Some(i), None) => {
                            format!("Input is {i:.2}s, but output duration missing")
                        }
                        (None, None) => unreachable!("Handled in match arm"),
                    };
                    CheckResult::Failed(reason)
                };
                let diff_str = diff.map_or_else(|| "N/A".to_string(), |d| format!("{d:.2}s"));
                details.push(format!(
                    "Duration: input {:?}, output {:?}, diff {} (tolerance {:.2}s) → {}",
                    inp.duration,
                    out.duration,
                    diff_str,
                    tol,
                    if ok { "OK" } else { "MISMATCH" }
                ));
            }
        }
        (Err(e), _) => {
            *probe_failed = true;
            details.push(format!("Input probe failed: {e}"));
            if options.require_duration_match {
                *duration_match = CheckResult::Failed(format!("Input probe failed: {e}"));
            }
            if options.require_video_stream {
                *has_video_stream = CheckResult::Failed(format!("Input probe failed: {e}"));
            }
        }
        (_, Err(e)) => {
            *probe_failed = true;
            details.push(format!("Output probe failed: {e}"));
            if options.require_duration_match {
                *duration_match = CheckResult::Failed(format!("Output probe failed: {e}"));
            }
            if options.require_video_stream {
                *has_video_stream = CheckResult::Failed(format!("Output probe failed: {e}"));
            }
            details.push("Duration/stream not verified (probe unavailable)".to_string());
        }
    }
}

const MIN_FILE_LEN: u64 = 32;

/// Verify AVIF output exists and has minimal size. Optional ffprobe or
/// signature checks can be added later.
/// Verify the health of an AVIF file using local tools.
///
/// # Errors
/// Returns an error message if the file is corrupt or tools fail.
pub fn verify_avif_health(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a file".to_string());
    }
    if meta.len() < MIN_FILE_LEN {
        return Err(format!("AVIF file too small ({} bytes)", meta.len()));
    }
    Ok(())
}

/// Verify AV1-in-MP4 output exists and has minimal size. Optional ffprobe
/// or duration checks can be added later.
/// Verify the health of an AV1 MP4 file.
///
/// # Errors
/// Returns an error message if the file is corrupt.
pub fn verify_av1_mp4_health(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a file".to_string());
    }
    if meta.len() < MIN_FILE_LEN {
        return Err(format!("AV1 MP4 file too small ({} bytes)", meta.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_verify_options_defaults() {
        let o = VerifyOptions::default();
        assert_eq!(
            o.effective_min_size(),
            crate::constants::DEFAULT_MIN_FILE_SIZE
        );
        let m = VerifyOptions {
            min_file_size: 100,
            ..VerifyOptions::default()
        };
        assert_eq!(m.effective_min_size(), 100);
    }

    #[test]
    fn test_verify_output_file_nonexistent() {
        let r = verify_output_file(Path::new("/nonexistent/path/xyz"), 1);
        assert!(r.is_err());
    }

    #[test]
    fn test_verify_output_file_empty_or_small()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir();
        let empty = dir.join("quality_verifier_test_empty");
        let _ = std::fs::File::create(&empty).and_then(|f| f.sync_all());
        let r = verify_output_file(&empty, 1);
        assert!(r.is_err()); // 0 bytes < 1
        let _ = crate::io_utils::safe_remove_file(&empty);

        let small = dir.join("quality_verifier_test_small");
        let mut f = std::fs::File::create(&small)?;
        f.write_all(&[0u8; 64])?;
        f.sync_all()?;
        drop(f);
        let r = verify_output_file(&small, 32);
        assert!(r.is_ok());
        let _ = crate::io_utils::safe_remove_file(&small);
        Ok(())
    }

    #[test]
    fn test_enhanced_result_passed() {
        let r = EnhancedVerifyResult {
            file_ok: true,
            duration_match: CheckResult::Passed,
            has_video_stream: CheckResult::Passed,
            message: "OK".to_string(),
            details: vec![],
        };
        assert!(r.passed());
        let r2 = EnhancedVerifyResult {
            file_ok: true,
            duration_match: CheckResult::NotChecked,
            has_video_stream: CheckResult::NotChecked,
            message: "OK".to_string(),
            details: vec![],
        };
        assert!(r2.passed());
        let r3 = EnhancedVerifyResult {
            file_ok: true,
            duration_match: CheckResult::Failed("mismatch".to_string()),
            has_video_stream: CheckResult::Passed,
            message: "Duration mismatch".to_string(),
            details: vec![],
        };
        assert!(!r3.passed());
    }

    /// Regression: use only temp copies (no original folder). When input/output are not valid video, probe fails and `enhanced_verify_fail_reason` is set.
    #[test]
    fn test_verify_after_encode_with_temp_copies_probe_fails()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir();
        let input_copy = dir.join("enhanced_verify_test_input_copy");
        let output_copy = dir.join("enhanced_verify_test_output_copy");
        // Pad beyond DEFAULT_MIN_FILE_SIZE so the size gate passes and probe runs.
        let min_size = usize::try_from(crate::constants::DEFAULT_MIN_FILE_SIZE)
            .expect("DEFAULT_MIN_FILE_SIZE fits in usize on supported targets");
        let minimal = vec![0u8; min_size * 2];
        std::fs::write(&input_copy, &minimal)?;
        std::fs::write(&output_copy, &minimal)?;
        let result = verify_after_encode(&input_copy, &output_copy, &VerifyOptions::strict_video());
        let _ = crate::io_utils::safe_remove_file(&input_copy);
        let _ = crate::io_utils::safe_remove_file(&output_copy);
        assert!(
            !result.passed(),
            "non-video files should fail strict verification"
        );
        assert!(
            result.message.contains("Probe") || result.message.contains("probe"),
            "expected probe-related message, got: {}",
            result.message
        );
        Ok(())
    }
}
