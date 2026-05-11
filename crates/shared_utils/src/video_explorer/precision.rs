//! CRF precision constants and quality grade helpers

use crate::crf_constants::{CRF_CACHE_KEY_MULTIPLIER, CRF_CACHE_MAX_VALID};

pub const CRF_PRECISION: f32 = crate::constants::CRF_PRECISION;

pub const SEARCH_STEP_COARSE: f32 = crate::constants::SEARCH_STEP_COARSE;
pub const SEARCH_STEP_FINE: f32 = crate::constants::SEARCH_STEP_FINE;
pub const SEARCH_STEP_ULTRA_FINE: f32 = crate::constants::SEARCH_STEP_ULTRA_FINE;
pub const SEARCH_STEP_CPU_FINEST: f32 = crate::constants::SEARCH_STEP_CPU_FINEST;

/// Same as `crf_constants::CRF_CACHE_KEY_MULTIPLIER` so cache keys match `CrfCache` / `Crf::to_cache_key`.
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

#[inline]
#[must_use]
pub fn cache_key_to_crf(key: i32) -> f32 {
    if key <= 0_i32 {
        return 0.0;
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

/// Step sizes per phase; mirrors `SearchPhase::step_size()` but allows runtime override (e.g. tests). Defaults match `SearchPhase`.
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

/// Returns binary-search iteration count for CRF range. Returns None if calculation fails.
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
    psnr >= 0.0 || psnr.is_infinite()
}

/// Do not use for fixed-width terminal alignment; string length != display width (CJK).
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
    if psnr.is_infinite() {
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

/// Do not use for fixed-width terminal alignment; string length != display width (CJK).
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
