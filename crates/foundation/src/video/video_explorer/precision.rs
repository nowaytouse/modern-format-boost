//! CRF precision constants and quality grade helpers.
//!
//! Exploration precision layers (bottom → top):
//! 1. **Parse** — ffmpeg/VMAF parsers reject non-finite or out-of-domain
//!    samples.
//! 2. **Search grid** — CRF steps via [`SearchPhase`]; JXL distances via
//!    `seal_jxl_distance` in `algorithm_seal`.
//! 3. **Algorithm seal** —
//!    [`crate::video_explorer::ExploreResult::seal_algorithm_outputs`] snaps
//!    CRF to the cache grid.
//! 4. **Delivery gates** — [`ExploreResult::sealed`] enforces coherence before
//!    pipeline acceptance.

use crate::crf_constants::{CRF_CACHE_KEY_MULTIPLIER, CRF_CACHE_MAX_VALID};

pub const CRF_PRECISION: f32 = crate::constants::CRF_PRECISION;

pub const SEARCH_STEP_COARSE: f32 = crate::constants::SEARCH_STEP_COARSE;
pub const SEARCH_STEP_FINE: f32 = crate::constants::SEARCH_STEP_FINE;
pub const SEARCH_STEP_ULTRA_FINE: f32 = crate::constants::SEARCH_STEP_ULTRA_FINE;
pub const SEARCH_STEP_CPU_FINEST: f32 = crate::constants::SEARCH_STEP_CPU_FINEST;

/// Same as `crf_constants::CRF_CACHE_KEY_MULTIPLIER` so cache keys match
/// `CrfCache` / `Crf::to_cache_key`.
pub const CACHE_KEY_MULTIPLIER: f64 = CRF_CACHE_KEY_MULTIPLIER;

#[inline]
#[must_use]
pub fn crf_to_cache_key(crf: f32) -> Option<i32> {
    if !crf.is_finite() || crf < 0.0 {
        return None;
    }
    let capped = f64::from(crf).min(CRF_CACHE_MAX_VALID);
    let normalized = (capped * CACHE_KEY_MULTIPLIER).round();
    crate::numeric_cast::f64_to_i32_strict(normalized, "crf_precision_key")
}

/// Snap exploration CRF to the shared `CrfCache` key grid (rejects non-finite /
/// negative).
#[inline]
#[must_use]
pub fn seal_exploration_crf(crf: f32) -> f32 {
    crate::media_conversion_gate::explore_seal_crf_or_zero(crf, "seal_exploration_crf")
}

#[inline]
#[must_use]
pub fn cache_key_to_crf(key: i32) -> f32 {
    if key <= 0_i32 {
        return f32::NAN;
    }
    crate::numeric_cast::f64_to_f32_lossy(
        crate::numeric_cast::i32_to_f64(key) / CACHE_KEY_MULTIPLIER,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPhase {
    GpuCoarse,
    GpuMedium,
    GpuFine,
    GpuUltraFine,
    CpuFinest,
}

impl SearchPhase {
    #[must_use]
    pub const fn step_size(&self) -> f32 {
        match self {
            Self::GpuCoarse => crate::constants::SEARCH_STEP_GPU_COARSE,
            Self::GpuMedium => crate::constants::SEARCH_STEP_GPU_MEDIUM,
            Self::GpuFine => SEARCH_STEP_FINE,
            Self::GpuUltraFine => SEARCH_STEP_ULTRA_FINE,
            Self::CpuFinest => SEARCH_STEP_CPU_FINEST,
        }
    }

    #[must_use]
    pub const fn is_gpu(&self) -> bool {
        matches!(
            self,
            Self::GpuCoarse | Self::GpuMedium | Self::GpuFine | Self::GpuUltraFine
        )
    }

    #[must_use]
    pub const fn next(&self) -> Option<Self> {
        match self {
            Self::GpuCoarse => Some(Self::GpuMedium),
            Self::GpuMedium => Some(Self::GpuFine),
            Self::GpuFine => Some(Self::GpuUltraFine),
            Self::GpuUltraFine => Some(Self::CpuFinest),
            Self::CpuFinest => None,
        }
    }
}

/// Step sizes per phase; mirrors `SearchPhase::step_size()` but allows runtime
/// override (e.g. tests). Defaults match `SearchPhase`.
#[derive(Debug, Clone)]
pub struct ThreePhaseSearch {
    pub gpu_coarse_step: f32,
    pub gpu_medium_step: f32,
    pub gpu_fine_step: f32,
    pub gpu_ultra_fine_step: f32,
    pub cpu_finest_step: f32,
}

impl Default for ThreePhaseSearch {
    fn default() -> Self {
        Self {
            gpu_coarse_step: crate::constants::SEARCH_STEP_GPU_COARSE,
            gpu_medium_step: crate::constants::SEARCH_STEP_GPU_MEDIUM,
            gpu_fine_step: SEARCH_STEP_FINE,
            gpu_ultra_fine_step: SEARCH_STEP_ULTRA_FINE,
            cpu_finest_step: SEARCH_STEP_CPU_FINEST,
        }
    }
}

impl ThreePhaseSearch {
    #[must_use]
    pub const fn step_for_phase(&self, phase: SearchPhase) -> f32 {
        match phase {
            SearchPhase::GpuCoarse => self.gpu_coarse_step,
            SearchPhase::GpuMedium => self.gpu_medium_step,
            SearchPhase::GpuFine => self.gpu_fine_step,
            SearchPhase::GpuUltraFine => self.gpu_ultra_fine_step,
            SearchPhase::CpuFinest => self.cpu_finest_step,
        }
    }
}

pub const SSIM_DISPLAY_PRECISION: u32 = crate::constants::SSIM_DISPLAY_PRECISION;
pub const SSIM_COMPARE_EPSILON: f64 = crate::types::SSIM_EPSILON;
pub const DEFAULT_MIN_SSIM: f64 = crate::constants::DEFAULT_MIN_SSIM;
pub const HIGH_QUALITY_MIN_SSIM: f64 = crate::constants::HIGH_QUALITY_MIN_SSIM;
pub const ACCEPTABLE_MIN_SSIM: f64 = crate::constants::ACCEPTABLE_MIN_SSIM;
pub const MIN_ACCEPTABLE_SSIM: f64 = crate::constants::MIN_ACCEPTABLE_SSIM;
pub const PSNR_DISPLAY_PRECISION: u32 = crate::constants::PSNR_DISPLAY_PRECISION;
pub const DEFAULT_MIN_PSNR: f64 = crate::constants::DEFAULT_MIN_PSNR;
pub const HIGH_QUALITY_MIN_PSNR: f64 = crate::constants::HIGH_QUALITY_MIN_PSNR;

/// Returns binary-search iteration count for CRF range. Returns None if
/// calculation fails.
#[must_use]
pub fn required_iterations(min_crf: u8, max_crf: u8) -> Option<u32> {
    let range = f64::from(max_crf.saturating_sub(min_crf));
    if range <= 0.0_f64 {
        return Some(1);
    }
    Some(crate::numeric_cast::f64_to_u32_strict(range.log2().ceil(), "crf_range_iterations")? + 1)
}

#[must_use]
pub fn ssim_meets_threshold(ssim: f64, threshold: f64) -> bool {
    crate::float_compare::ssim_meets_threshold(ssim, threshold)
}

#[must_use]
pub fn is_valid_ssim(ssim: f64) -> bool {
    crate::types::Ssim::new(ssim).is_ok()
}

#[must_use]
pub fn is_valid_psnr(psnr: f64) -> bool {
    if psnr.is_infinite() {
        return true;
    }
    psnr.is_finite() && psnr >= 0.0
}

/// `FFmpeg` reports `inf` PSNR for identical streams; exploration uses a finite
/// sentinel (M72).
pub const EXPLORE_PSNR_INF_SENTINEL: f64 = 100.0;

/// Seal parsed PSNR before it enters explore results.
#[must_use]
pub fn seal_psnr(psnr: f64) -> Option<f64> {
    is_valid_psnr(psnr).then_some(psnr)
}

/// Seal parsed SSIM to the canonical \[0,1\] finite domain.
#[must_use]
pub fn seal_ssim(ssim: f64) -> Option<f64> {
    is_valid_ssim(ssim).then_some(ssim)
}

/// Seal Y/U/V/All SSIM quadruple before explore fusion (M72).
#[must_use]
pub fn seal_ssim_yuv_all_bundle(y: f64, u: f64, v: f64, all: f64) -> Option<(f64, f64, f64, f64)> {
    let y_sealed = seal_ssim(y)?;
    let u_sealed = seal_ssim(u)?;
    let v_sealed = seal_ssim(v)?;
    let all_sealed = seal_ssim(all)?;
    Some((y_sealed, u_sealed, v_sealed, all_sealed))
}

/// Parse and seal an explore SSIM scalar token (M73 central parser).
///
/// # Errors
/// Returns an error when a leading numeric token is present but malformed.
pub fn parse_explore_ssim_metric_token(raw: &str) -> Result<Option<f64>, String> {
    let after = raw.trim_start();
    if after.starts_with("inf") {
        return Ok(seal_ssim(crate::constants::FFMPEG_SSIM_PERFECT_PARSE_VALUE));
    }
    let end = crate::media_conversion_gate::explore_metric_numeric_end(after, true);
    if end > 0 {
        let token = &after[..end];
        let value = token
            .parse::<f64>()
            .map_err(|err| format!("SSIM metric token {token:?} parse error: {err}"))?;
        return Ok(seal_ssim(value));
    }
    Ok(None)
}

/// Parse and seal an explore PSNR scalar token (M73 central parser).
///
/// # Errors
/// Returns an error when a leading numeric token is present but malformed.
pub fn parse_explore_psnr_metric_token(raw: &str) -> Result<Option<f64>, String> {
    let after = raw.trim_start();
    if after.starts_with("inf") || after.starts_with("-inf") {
        return Ok(seal_psnr(EXPLORE_PSNR_INF_SENTINEL));
    }
    let end = crate::media_conversion_gate::explore_metric_numeric_end(after, true);
    if end > 0 {
        let token = &after[..end];
        let v = token
            .parse::<f64>()
            .map_err(|err| format!("PSNR metric token {token:?} parse error: {err}"))?;
        if v.is_finite() && v > 0.0_f64 {
            return Ok(seal_psnr(v));
        }
        crate::media_conversion_gate::explore_metric_parse_reject_audit(
            "psnr",
            format!("token {v:.6} rejected (non-positive or non-finite)"),
        );
    }
    Ok(None)
}

/// Parse and seal VMAF-Y mean from a JSON numeric token (M77).
///
/// # Errors
/// Returns an error when a leading numeric token is present but malformed.
pub fn parse_explore_vmaf_y_metric_token(raw: &str) -> Result<Option<f64>, String> {
    let after = raw.trim_start();
    let end = crate::media_conversion_gate::explore_metric_numeric_end(after, true);
    if end > 0 {
        let token = &after[..end];
        let value = token
            .parse::<f64>()
            .map_err(|err| format!("VMAF-Y metric token {token:?} parse error: {err}"))?;
        return Ok(seal_vmaf_y(value));
    }
    Ok(None)
}

/// Parse and seal CAMBI mean from a JSON numeric token (M79).
///
/// # Errors
/// Returns an error when a leading numeric token is present but malformed.
pub fn parse_explore_cambi_metric_token(raw: &str) -> Result<Option<f64>, String> {
    let after = raw.trim_start();
    let end = crate::media_conversion_gate::explore_metric_numeric_end(after, true);
    if end > 0 {
        let token = &after[..end];
        let v = token
            .parse::<f64>()
            .map_err(|err| format!("CAMBI metric token {token:?} parse error: {err}"))?;
        if let Some(sealed) = seal_cambi(v) {
            return Ok(Some(sealed));
        }
        crate::media_conversion_gate::explore_metric_parse_reject_audit(
            "cambi",
            format!("token {v:.6} non-finite/negative"),
        );
    }
    Ok(None)
}

/// Parse and seal legacy stderr `MS-SSIM score:` tokens (M73).
///
/// # Errors
/// Returns an error when a non-empty score token is malformed.
pub fn parse_explore_ms_ssim_score_token(raw: &str) -> Result<Option<f64>, String> {
    let token = raw.trim();
    if token.is_empty() {
        return Ok(None);
    }
    let value = token
        .parse::<f64>()
        .map_err(|err| format!("MS-SSIM metric token {token:?} parse error: {err}"))?;
    Ok(seal_ms_ssim(value))
}

/// VMAF-Y mean in \[0, 100\] (exploration contract).
#[must_use]
pub fn is_valid_vmaf_y(vmaf: f64) -> bool {
    vmaf.is_finite() && (0.0..=100.0).contains(&vmaf)
}

/// Seal parsed VMAF-Y before it enters explore results.
#[must_use]
pub fn seal_vmaf_y(vmaf: f64) -> Option<f64> {
    is_valid_vmaf_y(vmaf).then_some(vmaf)
}

/// CAMBI score must be finite and non-negative (higher is worse, no hard cap
/// here).
#[must_use]
pub fn is_valid_cambi(cambi: f64) -> bool {
    cambi.is_finite() && cambi >= 0.0
}

/// Seal parsed CAMBI before it enters explore results.
#[must_use]
pub fn seal_cambi(cambi: f64) -> Option<f64> {
    is_valid_cambi(cambi).then_some(cambi)
}

/// Seal parsed MS-SSIM to the canonical \[0, 1\] finite domain.
#[must_use]
pub fn seal_ms_ssim(ms_ssim: f64) -> Option<f64> {
    is_valid_ms_ssim(ms_ssim).then_some(ms_ssim)
}

/// Seal Y/U/V MS-SSIM triplet and 4:2:0 weighted average before explore fusion
/// (M71).
#[must_use]
pub fn seal_ms_ssim_yuv_bundle(
    y: f64,
    u_ms_ssim: Option<f64>,
    v_ms_ssim: Option<f64>,
) -> Option<(f64, f64, f64, f64)> {
    let y_sealed = seal_ms_ssim(y)?;
    if let (Some(u), Some(v)) = (u_ms_ssim, v_ms_ssim) {
        let u_sealed = seal_ms_ssim(u)?;
        let v_sealed = seal_ms_ssim(v)?;
        let raw_avg = (y_sealed.mul_add(4.0, u_sealed) + v_sealed) / 6.0_f64;
        let avg_sealed = seal_ms_ssim_composite_average(raw_avg)?;
        Some((y_sealed, u_sealed, v_sealed, avg_sealed))
    } else {
        Some((y_sealed, y_sealed, y_sealed, y_sealed))
    }
}

/// Ultimate explore captured all three 3D metrics (strict delivery requires
/// this, not partial telemetry).
#[must_use]
pub const fn has_complete_ultimate_metrics(
    vmaf_y: Option<f64>,
    cambi: Option<f64>,
    psnr_uv: Option<(f64, f64)>,
) -> bool {
    vmaf_y.is_some() && cambi.is_some() && psnr_uv.is_some()
}

/// Hard sanity floors for sealed explore results (independent of per-file
/// adaptive Phase 3 floors).
#[must_use]
pub fn ultimate_metrics_meet_exploration_sanity(
    vmaf_y: Option<f64>,
    cambi: Option<f64>,
    psnr_uv: Option<(f64, f64)>,
) -> bool {
    let Some(vmaf) = vmaf_y else {
        return false;
    };
    let Some(cambi) = cambi else {
        return false;
    };
    let Some((u, v)) = psnr_uv else {
        return false;
    };
    vmaf.is_finite()
        && cambi.is_finite()
        && u.is_finite()
        && v.is_finite()
        && vmaf >= crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR
        && u >= crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR
        && v >= crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR
        && cambi <= crate::constants::EXPLORATION_CAMBI_MAX
}

/// Do not use for fixed-width terminal alignment; string length != display
/// width (CJK).
#[must_use]
pub fn ssim_quality_grade(ssim: f64) -> &'static str {
    if ssim >= crate::constants::SSIM_GRADE_EXCELLENT {
        "Excellent (visually indistinguishable)"
    } else if ssim >= crate::constants::SSIM_GRADE_GOOD {
        "Good (visually lossless)"
    } else if ssim >= crate::constants::SSIM_GRADE_ACCEPTABLE {
        "Acceptable (minor difference)"
    } else if ssim >= crate::constants::SSIM_GRADE_FAIR {
        "Fair (visible difference)"
    } else {
        "Poor (noticeable quality loss)"
    }
}

#[must_use]
pub fn psnr_quality_grade(psnr: f64) -> &'static str {
    if psnr.is_infinite() || (psnr - EXPLORE_PSNR_INF_SENTINEL).abs() < f64::EPSILON {
        "Lossless (identical)"
    } else if psnr >= crate::constants::PSNR_GRADE_EXCELLENT {
        "Excellent (visually indistinguishable)"
    } else if psnr >= crate::constants::PSNR_GRADE_GOOD {
        "Good (visually lossless)"
    } else if psnr >= crate::constants::PSNR_GRADE_ACCEPTABLE {
        "Acceptable (minor difference)"
    } else if psnr >= crate::constants::PSNR_GRADE_FAIR {
        "Fair (visible difference)"
    } else {
        "Poor (noticeable quality loss)"
    }
}

#[must_use]
pub fn format_ssim(ssim: f64) -> String {
    format!("{ssim:.4}")
}

#[must_use]
pub fn format_psnr(psnr: f64) -> String {
    if psnr.is_infinite() {
        "∞".to_string()
    } else {
        format!("{psnr:.2} dB")
    }
}

pub const DEFAULT_MIN_MS_SSIM: f64 = crate::constants::DEFAULT_MIN_MS_SSIM;
pub const HIGH_QUALITY_MIN_MS_SSIM: f64 = crate::constants::HIGH_QUALITY_MIN_MS_SSIM;
pub const ACCEPTABLE_MIN_MS_SSIM: f64 = crate::constants::ACCEPTABLE_MIN_MS_SSIM;

#[must_use]
pub fn is_valid_ms_ssim(ms_ssim: f64) -> bool {
    (0.0..=1.0).contains(&ms_ssim)
}

/// Float noise tolerance for composite MS-SSIM averages (Y/U/V already sealed).
const MS_SSIM_COMPOSITE_AVG_EPSILON: f64 = 1e-9;

#[must_use]
fn seal_ms_ssim_composite_average(raw_avg: f64) -> Option<f64> {
    if !raw_avg.is_finite() {
        return None;
    }
    if is_valid_ms_ssim(raw_avg) {
        return Some(raw_avg);
    }
    // Weighted mean of sealed [0,1] channels can land at 1.0 ± float noise — snap
    // to domain edge only.
    if (0.0..=1.0 + MS_SSIM_COMPOSITE_AVG_EPSILON).contains(&raw_avg) {
        let snapped = if raw_avg > 1.0 { 1.0 } else { 0.0 };
        return is_valid_ms_ssim(snapped).then_some(snapped);
    }
    None
}

/// Do not use for fixed-width terminal alignment; string length != display
/// width (CJK).
#[must_use]
pub fn ms_ssim_quality_grade(ms_ssim: f64) -> &'static str {
    if ms_ssim >= crate::constants::MS_SSIM_GRADE_EXCELLENT {
        "Excellent (visually indistinguishable)"
    } else if ms_ssim >= crate::constants::MS_SSIM_GRADE_GOOD {
        "Good (streaming quality)"
    } else if ms_ssim >= crate::constants::MS_SSIM_GRADE_ACCEPTABLE {
        "Acceptable (mobile quality)"
    } else if ms_ssim >= crate::constants::MS_SSIM_GRADE_FAIR {
        "Fair (visible difference)"
    } else {
        "Poor (noticeable quality loss)"
    }
}

#[must_use]
pub fn format_ms_ssim(ms_ssim: f64) -> String {
    format!("{ms_ssim:.4}")
}

#[cfg(test)]
mod seal_tests {
    use super::*;

    #[test]
    fn seal_exploration_crf_snaps_to_cache_grid() {
        assert!((seal_exploration_crf(23.126) - 23.13).abs() < f32::EPSILON);
        assert!((seal_exploration_crf(23.0) - 23.0).abs() < f32::EPSILON);
        assert!(seal_exploration_crf(f32::NAN).is_nan());
    }

    #[test]
    fn seal_vmaf_y_rejects_out_of_domain() {
        assert_eq!(seal_vmaf_y(95.5), Some(95.5));
        assert!(seal_vmaf_y(101.0).is_none());
        assert!(seal_vmaf_y(f64::NAN).is_none());
    }

    #[test]
    fn seal_cambi_rejects_negative_or_non_finite() {
        assert_eq!(seal_cambi(0.0), Some(0.0));
        assert_eq!(seal_cambi(7.5), Some(7.5));
        assert!(seal_cambi(-0.1).is_none());
        assert!(seal_cambi(f64::NAN).is_none());
        assert!(seal_cambi(f64::INFINITY).is_none());
    }

    #[test]
    fn seal_ms_ssim_rejects_out_of_domain() {
        assert_eq!(seal_ms_ssim(0.0), Some(0.0));
        assert_eq!(seal_ms_ssim(0.99), Some(0.99));
        assert!(seal_ms_ssim(-0.001).is_none());
        assert!(seal_ms_ssim(1.001).is_none());
        assert!(seal_ms_ssim(f64::NAN).is_none());
    }

    #[test]
    fn seal_ms_ssim_yuv_bundle_full_chroma() {
        let (y, u, v, avg) = seal_ms_ssim_yuv_bundle(0.9, Some(0.8), Some(0.7)).unwrap();
        assert!((y - 0.9).abs() < f64::EPSILON);
        assert!((u - 0.8).abs() < f64::EPSILON);
        assert!((v - 0.7).abs() < f64::EPSILON);
        assert!((avg - (0.9_f64.mul_add(4.0, 0.8) + 0.7) / 6.0_f64).abs() < f64::EPSILON);
    }

    #[test]
    fn seal_ms_ssim_yuv_bundle_y_only_fallback() {
        let (y, u, v, avg) = seal_ms_ssim_yuv_bundle(0.85, None, None).unwrap();
        assert!((y - 0.85).abs() < f64::EPSILON);
        assert!((u - 0.85).abs() < f64::EPSILON);
        assert!((v - 0.85).abs() < f64::EPSILON);
        assert!((avg - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn seal_ms_ssim_yuv_bundle_rejects_invalid_y() {
        assert!(seal_ms_ssim_yuv_bundle(1.05, Some(0.9), Some(0.9)).is_none());
    }

    #[test]
    fn seal_ms_ssim_yuv_bundle_tolerates_composite_float_noise() {
        assert!(seal_ms_ssim_yuv_bundle(1.0, Some(1.0), Some(1.0)).is_some());
    }

    #[test]
    fn seal_ssim_yuv_all_bundle_rejects_out_of_domain() {
        assert!(seal_ssim_yuv_all_bundle(1.01, 0.9, 0.9, 0.95).is_none());
    }

    #[test]
    fn seal_psnr_accepts_inf_sentinel() {
        assert!(
            (seal_psnr(EXPLORE_PSNR_INF_SENTINEL).unwrap_or_else(|| panic!("missing")) - 100.0)
                .abs()
                < f64::EPSILON
        );
        assert!(seal_psnr(-1.0).is_none());
    }

    #[test]
    fn parse_explore_ssim_metric_token_rejects_oob() {
        assert!(matches!(parse_explore_ssim_metric_token("1.05"), Ok(None)));
        assert!(matches!(
            parse_explore_ssim_metric_token("0.95"),
            Ok(Some(_))
        ));
    }

    #[test]
    fn parse_explore_metric_token_malformed_numeric_returns_error() {
        assert!(parse_explore_ssim_metric_token(".").is_err());
        assert!(parse_explore_psnr_metric_token("12.3.4").is_err());
        assert!(parse_explore_vmaf_y_metric_token(".").is_err());
        assert!(parse_explore_cambi_metric_token("5.0.1").is_err());
        assert!(parse_explore_ms_ssim_score_token("not-a-number").is_err());
    }

    #[test]
    fn ultimate_sanity_requires_complete_metrics() {
        assert!(!ultimate_metrics_meet_exploration_sanity(
            Some(90.0),
            None,
            Some((40.0, 40.0))
        ));
        assert!(ultimate_metrics_meet_exploration_sanity(
            Some(90.0),
            Some(5.0),
            Some((40.0, 40.0))
        ));
        assert!(!ultimate_metrics_meet_exploration_sanity(
            Some(80.0),
            Some(5.0),
            Some((40.0, 40.0))
        ));
    }
}
