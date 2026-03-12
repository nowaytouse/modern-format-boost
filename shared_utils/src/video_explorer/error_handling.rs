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
    pub fn ms_ssim_or_err(&self) -> Result<f64> {
        self.ms_ssim.ok_or_else(|| anyhow::anyhow!("MS-SSIM not measured"))
    }

    /// Get SSIM score or return error if not measured
    pub fn ssim_or_err(&self) -> Result<f64> {
        self.ssim.ok_or_else(|| anyhow::anyhow!("SSIM not measured"))
    }

    /// Get VMAF score or return error if not measured
    pub fn vmaf_or_err(&self) -> Result<f64> {
        self.vmaf_y.ok_or_else(|| anyhow::anyhow!("VMAF not measured"))
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
    pub fn is_real_success(&self) -> bool {
        matches!(self, CompressionResult::Success { .. })
    }

    /// Get the CRF value, regardless of success/failure
    pub fn crf(&self) -> f32 {
        match self {
            CompressionResult::Success { crf, .. } => *crf,
            CompressionResult::QualityFailed { attempted_crf, .. } => *attempted_crf,
            CompressionResult::SizeFailed { attempted_crf, .. } => *attempted_crf,
            CompressionResult::NoCompressionPossible { fallback_crf, .. } => *fallback_crf,
        }
    }

    /// Get error message for failed compressions
    pub fn error_message(&self) -> Option<String> {
        match self {
            CompressionResult::Success { .. } => None,
            CompressionResult::QualityFailed { reason, actual_score, target_score, .. } => {
                Some(format!(
                    "Quality check failed: {} (score: {:.4}, target: {:.2})",
                    reason,
                    actual_score.unwrap_or(0.0),
                    target_score
                ))
            }
            CompressionResult::SizeFailed { output_size, input_size, .. } => {
                Some(format!(
                    "Size target failed: output {} bytes >= input {} bytes",
                    output_size, input_size
                ))
            }
            CompressionResult::NoCompressionPossible { reason, .. } => {
                Some(format!("No compression possible: {}", reason))
            }
        }
    }
}

/// Validate quality score against target, return explicit error
pub fn validate_quality_score(
    score: Option<f64>,
    target: f64,
    metric_name: &str,
) -> Result<f64> {
    match score {
        Some(s) if s >= target => Ok(s),
        Some(s) => bail!(
            "{} score {:.4} below target {:.2}",
            metric_name,
            s,
            target
        ),
        None => bail!("{} not measured", metric_name),
    }
}

/// Validate size reduction, return explicit error if failed
pub fn validate_size_reduction(output_size: u64, input_size: u64) -> Result<()> {
    if output_size < input_size {
        Ok(())
    } else {
        bail!(
            "Output size {} bytes >= input size {} bytes ({:+.1}%)",
            output_size,
            input_size,
            ((output_size as f64 / input_size as f64) - 1.0) * 100.0
        )
    }
}
