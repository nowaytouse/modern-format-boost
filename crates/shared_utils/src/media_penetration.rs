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

    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified(_))
    }
}

/// Penetrating audio detection: decode and analyze actual audio samples.
/// Returns `Verified(true)` if silent, `Verified(false)` if audible, `Failed` on error.
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
            emit_stderr(&format!(
                "⚠️  Audio penetration failed: ffmpeg error ({})",
                e
            ));
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
/// Returns `Verified(true)` if alpha is used, `Verified(false)` if fake, `Skipped` if no claim.
pub fn detect_real_transparency(path: &Path, has_transparency_claim: bool) -> PenetrationResult<bool> {
    if !has_transparency_claim {
        return PenetrationResult::Skipped;
    }
    
    // Sample 3 frames and check alpha channel variance
    let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(3)
        .arg("-vf")
        .arg("alphaextract,signalstats")
        .format("null")
        .output_pipe()
        .build()
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            emit_stderr(&format!(
                "⚠️  Transparency penetration failed: ffmpeg error ({})",
                e
            ));
            return PenetrationResult::Failed;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No alpha channel") || stderr.contains("Invalid pixel format") {
            // Metadata lied: claimed transparency but no alpha channel exists
            emit_stderr("⚠️  Transparency penetration: metadata claimed alpha but none exists");
            return PenetrationResult::Verified(false);
        }
        emit_stderr(&format!(
            "⚠️  Transparency penetration failed: ffmpeg exit code {}",
            output.status.code().unwrap_or(-1)
        ));
        return PenetrationResult::Failed;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    
    // Parse YMIN/YMAX from signalstats to detect alpha variance
    let mut has_variance = false;
    for line in stderr.lines() {
        if line.contains("lavfi.signalstats.YMIN") && line.contains("lavfi.signalstats.YMAX") {
            if let (Some(min_idx), Some(max_idx)) = (line.find("YMIN="), line.find("YMAX=")) {
                if let Some(min_str) = line[min_idx + 5..].split_whitespace().next() {
                    if let Some(max_str) = line[max_idx + 5..].split_whitespace().next() {
                        if let (Ok(min), Ok(max)) = (min_str.parse::<u8>(), max_str.parse::<u8>()) {
                            if min != max {
                                has_variance = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    
    if !has_variance {
        emit_stderr("⚠️  Transparency penetration: alpha channel exists but is unused (all opaque)");
    }
    
    PenetrationResult::Verified(has_variance)
}

/// Penetrating frame count detection: decode and count actual frames.
/// Returns `Verified(count)` with real count, `Skipped` if claim is reasonable.
pub fn detect_real_frame_count(path: &Path, claimed_frame_count: u64) -> PenetrationResult<u64> {
    // Only verify suspicious claims (≤1 or >50000)
    if claimed_frame_count > 1 && claimed_frame_count <= 50000 {
        return PenetrationResult::Skipped;
    }
    
    // Decode and count frames via showinfo
    let output = match crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .arg("-vf")
        .arg("showinfo")
        .format("null")
        .output_pipe()
        .build()
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            emit_stderr(&format!(
                "⚠️  Frame count penetration failed: ffmpeg error ({})",
                e
            ));
            return PenetrationResult::Failed;
        }
    };

    if !output.status.success() {
        emit_stderr(&format!(
            "⚠️  Frame count penetration failed: ffmpeg exit code {}",
            output.status.code().unwrap_or(-1)
        ));
        return PenetrationResult::Failed;
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let actual_count = stderr.lines().filter(|line| line.contains(" n:")).count();
    
    if actual_count == 0 {
        emit_stderr("⚠️  Frame count penetration failed: showinfo produced no output");
        return PenetrationResult::Failed;
    }

    let actual_u64 = actual_count as u64;
    if actual_u64 != claimed_frame_count {
        emit_stderr(&format!(
            "⚠️  Frame count mismatch detected: metadata claimed {}, actual decoded {}",
            claimed_frame_count, actual_u64
        ));
    }
    
    PenetrationResult::Verified(actual_u64)
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
    pub fn has_any_mismatch(&self) -> bool {
        self.audio_result.is_some()
            || (self.transparency_result.is_some() && !self.transparency_result.unwrap())
            || self.frame_count_mismatch.is_some()
    }

    pub fn report(&self) -> String {
        let mut lines = Vec::new();
        
        if self.audio_checked {
            if let Some(is_silent) = self.audio_result {
                lines.push(format!(
                    "  Audio: {}",
                    if is_silent { "SILENT (verified)" } else { "AUDIBLE (verified)" }
                ));
            }
        }
        
        if self.transparency_checked {
            if let Some(is_real) = self.transparency_result {
                lines.push(format!(
                    "  Transparency: {}",
                    if is_real { "REAL (verified)" } else { "FAKE (metadata lied)" }
                ));
            }
        }
        
        if self.frame_count_checked {
            if let Some((claimed, actual)) = self.frame_count_mismatch {
                lines.push(format!(
                    "  Frame count: MISMATCH (claimed={}, actual={})",
                    claimed, actual
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
    fn test_detect_real_transparency_no_claim_returns_skipped() {
        // When no transparency is claimed, should skip without decoding
        let fake_path = Path::new("/nonexistent.mp4");
        let result = detect_real_transparency(fake_path, false);
        assert_eq!(result, PenetrationResult::Skipped);
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
    }

    #[test]
    fn test_penetration_summary_report() {
        let mut summary = PenetrationSummary::default();
        summary.audio_checked = true;
        summary.audio_result = Some(true);
        summary.transparency_checked = true;
        summary.transparency_result = Some(false);
        
        let report = summary.report();
        assert!(report.contains("SILENT"));
        assert!(report.contains("FAKE"));
    }
}
