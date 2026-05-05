//! Honest error handling module - no silent fallbacks, explicit error reporting

use anyhow::{bail, Result};

/// Quality metric that must be explicitly present, not defaulted
#[derive(Debug, Clone)]
pub struct QualityMetric {
    pub ms_ssim: Option<f64>,
    pub ssim: Option<f64>,
    pub vmaf_y: Option<f64>,
    pub cambi: Option<f64>,
    pub psnr_uv: Option<(f64, f64)>,
}

impl QualityMetric {
    /// Get MS-SSIM score or return error if not measured
    ///
    /// # Errors
    /// Returns error if MS-SSIM was not measured.
    pub fn ms_ssim_or_err(&self) -> Result<f64> {
        self.ms_ssim
            .ok_or_else(|| anyhow::anyhow!("MS-SSIM not measured"))
    }

    /// Get SSIM score or return error if not measured
    ///
    /// # Errors
    /// Returns error if SSIM was not measured.
    pub fn ssim_or_err(&self) -> Result<f64> {
        self.ssim
            .ok_or_else(|| anyhow::anyhow!("SSIM not measured"))
    }

    /// Get VMAF score or return error if not measured
    ///
    /// # Errors
    /// Returns error if VMAF was not measured.
    pub fn vmaf_or_err(&self) -> Result<f64> {
        self.vmaf_y
            .ok_or_else(|| anyhow::anyhow!("VMAF not measured"))
    }
}

/// Compression result that explicitly tracks success/failure
#[derive(Debug)]
pub enum CompressionResult {
    Success {
        crf: f32,
        size: u64,
        quality: QualityMetric,
    },
    QualityFailed {
        attempted_crf: f32,
        reason: String,
        actual_score: Option<f64>,
        target_score: f64,
    },
    SizeFailed {
        attempted_crf: f32,
        output_size: u64,
        input_size: u64,
    },
    NoCompressionPossible {
        reason: String,
        fallback_crf: f32,
    },
}

impl CompressionResult {
    /// Check if this is a real success (not a fallback)
    #[must_use]
    pub const fn is_real_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Get the CRF value, regardless of success/failure
    #[must_use]
    pub const fn crf(&self) -> f32 {
        match self {
            Self::Success { crf, .. } => *crf,
            Self::QualityFailed { attempted_crf, .. } | Self::SizeFailed { attempted_crf, .. } => {
                *attempted_crf
            }
            Self::NoCompressionPossible { fallback_crf, .. } => *fallback_crf,
        }
    }

    /// Get error message for failed compressions
    #[must_use]
    pub fn error_message(&self) -> Option<String> {
        match self {
            Self::Success { .. } => None,
            Self::QualityFailed {
                reason,
                actual_score,
                target_score,
                ..
            } => {
                let score = actual_score
                    .map_or_else(|| "unknown".to_string(), |value| format!("{value:.4}"));
                Some(format!(
                    "Quality check failed: {reason} (score: {score}, target: {target_score:.2})"
                ))
            }
            Self::SizeFailed {
                output_size,
                input_size,
                ..
            } => Some(format!(
                "Size target failed: output {output_size} bytes >= input {input_size} bytes"
            )),
            Self::NoCompressionPossible { reason, .. } => {
                Some(format!("No compression possible: {reason}"))
            }
        }
    }
}

/// Validate quality score against target, return explicit error
///
/// # Errors
/// Returns error if score is below target or not measured.
pub fn validate_quality_score(score: Option<f64>, target: f64, metric_name: &str) -> Result<f64> {
    match score {
        Some(s) if s >= target => Ok(s),
        Some(s) => bail!("{metric_name} score {s:.4} below target {target:.2}"),
        None => bail!("{metric_name} not measured"),
    }
}

/// Validate size reduction, return explicit error if failed
///
/// # Errors
/// Returns error if output size is not smaller than input size.
pub fn validate_size_reduction(output_size: u64, input_size: u64) -> Result<()> {
    if output_size < input_size {
        Ok(())
    } else {
        bail!(
            "Output size {output_size} bytes >= input size {input_size} bytes ({:+.1}%)",
            ((crate::numeric_cast::u64_to_f64(output_size)
                / crate::numeric_cast::u64_to_f64(input_size))
                - 1.0)
                * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::CompressionResult;

    #[test]
    fn error_message_reports_unknown_score_honestly() {
        let result = CompressionResult::QualityFailed {
            attempted_crf: 23.0,
            reason: "SSIM not measured".to_string(),
            actual_score: None,
            target_score: 0.99_f64,
        };

        let message = result
            .error_message()
            .unwrap_or_else(|| panic!("quality failure should have a message"));
        assert!(message.contains("score: unknown"));
        assert!(!message.contains("score: 0.0000"));
    }
}
