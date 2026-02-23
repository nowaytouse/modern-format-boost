//! Enhanced quality verification for post-encode / post-conversion checks.
//!
//! Provides:
//! - Output file health (exists, non-empty, minimal size, readable)
//! - Optional input vs output sanity (duration match, video codec present)
//! - Integration with [crate::checkpoint::verify_output_integrity] and [crate::ffprobe].

use std::path::Path;

use crate::checkpoint::verify_output_integrity;
use crate::ffprobe;

/// Minimum file size (bytes) for "valid" output when not specified.
pub const DEFAULT_MIN_FILE_SIZE: u64 = 32;

/// Options for enhanced post-encode verification.
#[derive(Clone, Debug, Default)]
pub struct VerifyOptions {
    /// Minimum output file size in bytes. If 0, uses [DEFAULT_MIN_FILE_SIZE].
    pub min_file_size: u64,
    /// If true, require input and output duration to match within tolerance.
    pub require_duration_match: bool,
    /// Duration tolerance in seconds (input vs output). Used when [require_duration_match] is true.
    pub duration_tolerance_secs: f64,
    /// If true, require output to have a video stream (ffprobe).
    pub require_video_stream: bool,
}

impl VerifyOptions {
    pub fn strict_video() -> Self {
        Self {
            min_file_size: DEFAULT_MIN_FILE_SIZE,
            require_duration_match: true,
            duration_tolerance_secs: 0.5,
            require_video_stream: true,
        }
    }

    pub fn minimal() -> Self {
        Self {
            min_file_size: DEFAULT_MIN_FILE_SIZE,
            require_duration_match: false,
            duration_tolerance_secs: 0.0,
            require_video_stream: false,
        }
    }

    fn effective_min_size(&self) -> u64 {
        if self.min_file_size > 0 {
            self.min_file_size
        } else {
            DEFAULT_MIN_FILE_SIZE
        }
    }
}

/// Result of enhanced verification.
#[derive(Clone, Debug)]
pub struct EnhancedVerifyResult {
    pub file_ok: bool,
    pub duration_match: Option<bool>,
    pub has_video_stream: Option<bool>,
    pub message: String,
    pub details: Vec<String>,
}

impl EnhancedVerifyResult {
    pub fn passed(&self) -> bool {
        self.file_ok
            && self.duration_match.unwrap_or(true)
            && self.has_video_stream.unwrap_or(true)
    }

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
pub fn verify_output_file(output: &Path, min_size: u64) -> Result<(), String> {
    let effective = if min_size > 0 { min_size } else { DEFAULT_MIN_FILE_SIZE };
    verify_output_integrity(output, effective).map_err(|e| e.to_string())
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
            details.push(format!("Output file OK (≥ {} bytes)", min_size));
            true
        }
        Err(e) => {
            details.push(format!("Output file check failed: {}", e));
            return EnhancedVerifyResult {
                file_ok: false,
                duration_match: None,
                has_video_stream: None,
                message: e,
                details,
            };
        }
    };

    let mut duration_match: Option<bool> = None;
    let mut has_video_stream: Option<bool> = None;

    if options.require_duration_match || options.require_video_stream {
        let input_probe = ffprobe::probe_video(input);
        let output_probe = ffprobe::probe_video(output);

        match (input_probe, output_probe) {
            (Ok(ref inp), Ok(ref out)) => {
                if options.require_video_stream {
                    let has_video = !out.video_codec.is_empty() && out.video_codec != "unknown";
                    has_video_stream = Some(has_video);
                    if has_video {
                        details.push(format!("Output has video stream: {}", out.video_codec));
                    } else {
                        details.push("Output has no valid video stream".to_string());
                    }
                }
                if options.require_duration_match {
                    let tol = options.duration_tolerance_secs.max(0.0);
                    let diff = (inp.duration - out.duration).abs();
                    let ok = diff <= tol;
                    duration_match = Some(ok);
                    details.push(format!(
                        "Duration: input {:.2}s, output {:.2}s, diff {:.2}s (tolerance {:.2}s) → {}",
                        inp.duration,
                        out.duration,
                        diff,
                        tol,
                        if ok { "OK" } else { "MISMATCH" }
                    ));
                }
            }
            (Err(e), _) => {
                details.push(format!("Input probe failed: {}", e));
                if options.require_duration_match {
                    duration_match = Some(false);
                }
                if options.require_video_stream {
                    has_video_stream = None;
                }
            }
            (_, Err(e)) => {
                details.push(format!("Output probe failed: {}", e));
                if options.require_duration_match {
                    duration_match = Some(false);
                }
                if options.require_video_stream {
                    has_video_stream = Some(false);
                }
            }
        }
    }

    let failed = duration_match == Some(false) || has_video_stream == Some(false);
    let message = if failed {
        if duration_match == Some(false) {
            "Duration mismatch or output probe failed".to_string()
        } else if has_video_stream == Some(false) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_verify_options_defaults() {
        let o = VerifyOptions::default();
        assert_eq!(o.effective_min_size(), DEFAULT_MIN_FILE_SIZE);
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
    fn test_verify_output_file_empty_or_small() {
        let dir = std::env::temp_dir();
        let empty = dir.join("quality_verifier_test_empty");
        let _ = std::fs::File::create(&empty).and_then(|f| f.sync_all());
        let r = verify_output_file(&empty, 1);
        assert!(r.is_err()); // 0 bytes < 1
        let _ = std::fs::remove_file(&empty);

        let small = dir.join("quality_verifier_test_small");
        let mut f = std::fs::File::create(&small).unwrap();
        f.write_all(&[0u8; 64]).unwrap();
        f.sync_all().unwrap();
        drop(f);
        let r = verify_output_file(&small, 32);
        assert!(r.is_ok());
        let _ = std::fs::remove_file(&small);
    }

    #[test]
    fn test_enhanced_result_passed() {
        let r = EnhancedVerifyResult {
            file_ok: true,
            duration_match: Some(true),
            has_video_stream: Some(true),
            message: "OK".to_string(),
            details: vec![],
        };
        assert!(r.passed());
        let r2 = EnhancedVerifyResult {
            file_ok: true,
            duration_match: None,
            has_video_stream: None,
            message: "OK".to_string(),
            details: vec![],
        };
        assert!(r2.passed());
        let r3 = EnhancedVerifyResult {
            file_ok: true,
            duration_match: Some(false),
            has_video_stream: Some(true),
            message: "Duration mismatch".to_string(),
            details: vec![],
        };
        assert!(!r3.passed());
    }
}
