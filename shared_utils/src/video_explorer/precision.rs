//! CRF precision constants and quality grade helpers

use crate::crf_constants::{CRF_CACHE_KEY_MULTIPLIER, CRF_CACHE_MAX_VALID};

pub const CRF_PRECISION: f32 = 0.25;

pub const COARSE_STEP: f32 = 2.0;

pub const FINE_STEP: f32 = 0.5;

pub const ULTRA_FINE_STEP: f32 = 0.25;

pub const CPU_FINEST_STEP: f32 = 0.1;

/// Same as `crf_constants::CRF_CACHE_KEY_MULTIPLIER` so cache keys match CrfCache / Crf::to_cache_key.
pub const CACHE_KEY_MULTIPLIER: f32 = CRF_CACHE_KEY_MULTIPLIER;

#[inline]
pub fn crf_to_cache_key(crf: f32) -> i32 {
    if !crf.is_finite() || crf < 0.0 {
        return 0;
    }
    let capped = crf.min(CRF_CACHE_MAX_VALID);
    let normalized = (capped * CACHE_KEY_MULTIPLIER).round();
    let key = normalized as i32;
    debug_assert!(
        key >= 0 && key <= (CRF_CACHE_MAX_VALID * CACHE_KEY_MULTIPLIER) as i32,
        "Cache key {} out of expected range for CRF {}",
        key,
        crf
    );
    key
}

#[inline]
pub fn cache_key_to_crf(key: i32) -> f32 {
    if key <= 0 {
        return 0.0;
    }
    key as f32 / CACHE_KEY_MULTIPLIER
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
    pub fn step_size(&self) -> f32 {
        match self {
            SearchPhase::GpuCoarse => 4.0,
            SearchPhase::GpuMedium => 1.0,
            SearchPhase::GpuFine => FINE_STEP,
            SearchPhase::GpuUltraFine => ULTRA_FINE_STEP,
            SearchPhase::CpuFinest => CPU_FINEST_STEP,
        }
    }

    pub fn is_gpu(&self) -> bool {
        matches!(
            self,
            SearchPhase::GpuCoarse
                | SearchPhase::GpuMedium
                | SearchPhase::GpuFine
                | SearchPhase::GpuUltraFine
        )
    }

    pub fn next(&self) -> Option<SearchPhase> {
        match self {
            SearchPhase::GpuCoarse => Some(SearchPhase::GpuMedium),
            SearchPhase::GpuMedium => Some(SearchPhase::GpuFine),
            SearchPhase::GpuFine => Some(SearchPhase::GpuUltraFine),
            SearchPhase::GpuUltraFine => Some(SearchPhase::CpuFinest),
            SearchPhase::CpuFinest => None,
        }
    }
}

/// Step sizes per phase; mirrors SearchPhase::step_size() but allows runtime override (e.g. tests). Defaults match SearchPhase.
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
            gpu_coarse_step: 4.0,
            gpu_medium_step: 1.0,
            gpu_fine_step: FINE_STEP,
            gpu_ultra_fine_step: ULTRA_FINE_STEP,
            cpu_finest_step: CPU_FINEST_STEP,
        }
    }
}

impl ThreePhaseSearch {
    pub fn step_for_phase(&self, phase: SearchPhase) -> f32 {
        match phase {
            SearchPhase::GpuCoarse => self.gpu_coarse_step,
            SearchPhase::GpuMedium => self.gpu_medium_step,
            SearchPhase::GpuFine => self.gpu_fine_step,
            SearchPhase::GpuUltraFine => self.gpu_ultra_fine_step,
            SearchPhase::CpuFinest => self.cpu_finest_step,
        }
    }
}

pub const SSIM_DISPLAY_PRECISION: u32 = 4;

pub const SSIM_COMPARE_EPSILON: f64 = crate::types::SSIM_EPSILON;

pub const DEFAULT_MIN_SSIM: f64 = 0.95;

pub const HIGH_QUALITY_MIN_SSIM: f64 = 0.98;

pub const ACCEPTABLE_MIN_SSIM: f64 = 0.90;

pub const MIN_ACCEPTABLE_SSIM: f64 = 0.85;

pub const PSNR_DISPLAY_PRECISION: u32 = 2;

pub const DEFAULT_MIN_PSNR: f64 = 35.0;

pub const HIGH_QUALITY_MIN_PSNR: f64 = 40.0;

/// Returns binary-search iteration count for CRF range. Requires `max_crf >= min_crf` (otherwise saturates to 0 range).
pub fn required_iterations(min_crf: u8, max_crf: u8) -> u32 {
    let range = (max_crf.saturating_sub(min_crf)) as f64;
    if range <= 0.0 {
        return 1;
    }
    (range.log2().ceil() as u32) + 1
}

pub fn ssim_meets_threshold(ssim: f64, threshold: f64) -> bool {
    crate::float_compare::ssim_meets_threshold(ssim, threshold)
}

pub fn is_valid_ssim(ssim: f64) -> bool {
    crate::types::Ssim::new(ssim).is_ok()
}

pub fn is_valid_psnr(psnr: f64) -> bool {
    psnr >= 0.0 || psnr.is_infinite()
}

/// Do not use for fixed-width terminal alignment; string length != display width (CJK).
pub fn ssim_quality_grade(ssim: f64) -> &'static str {
    if ssim >= 0.98 {
        "Excellent (visually indistinguishable)"
    } else if ssim >= 0.95 {
        "Good (visually lossless)"
    } else if ssim >= 0.90 {
        "Acceptable (minor difference)"
    } else if ssim >= 0.85 {
        "Fair (visible difference)"
    } else {
        "Poor (noticeable quality loss)"
    }
}

pub fn psnr_quality_grade(psnr: f64) -> &'static str {
    if psnr.is_infinite() {
        "Lossless (identical)"
    } else if psnr >= 45.0 {
        "Excellent (visually indistinguishable)"
    } else if psnr >= 40.0 {
        "Good (visually lossless)"
    } else if psnr >= 35.0 {
        "Acceptable (minor difference)"
    } else if psnr >= 30.0 {
        "Fair (visible difference)"
    } else {
        "Poor (noticeable quality loss)"
    }
}

pub fn format_ssim(ssim: f64) -> String {
    format!("{:.4}", ssim)
}

pub fn format_psnr(psnr: f64) -> String {
    if psnr.is_infinite() {
        "∞".to_string()
    } else {
        format!("{:.2} dB", psnr)
    }
}

pub const DEFAULT_MIN_MS_SSIM: f64 = 0.90;

pub const HIGH_QUALITY_MIN_MS_SSIM: f64 = 0.95;

pub const ACCEPTABLE_MIN_MS_SSIM: f64 = 0.85;

pub fn is_valid_ms_ssim(ms_ssim: f64) -> bool {
    (0.0..=1.0).contains(&ms_ssim)
}

/// Do not use for fixed-width terminal alignment; string length != display width (CJK).
pub fn ms_ssim_quality_grade(ms_ssim: f64) -> &'static str {
    if ms_ssim >= 0.95 {
        "Excellent (visually indistinguishable)"
    } else if ms_ssim >= 0.90 {
        "Good (streaming quality)"
    } else if ms_ssim >= 0.85 {
        "Acceptable (mobile quality)"
    } else if ms_ssim >= 0.80 {
        "Fair (visible difference)"
    } else {
        "Poor (noticeable quality loss)"
    }
}

pub fn format_ms_ssim(ms_ssim: f64) -> String {
    format!("{:.4}", ms_ssim)
}
