//! CRF precision constants and quality grade helpers

pub const CRF_PRECISION: f32 = 0.25;

pub const COARSE_STEP: f32 = 2.0;

pub const FINE_STEP: f32 = 0.5;

pub const ULTRA_FINE_STEP: f32 = 0.25;

pub const CPU_FINEST_STEP: f32 = 0.1;


pub const CACHE_KEY_MULTIPLIER: f32 = 10.0;

#[inline]
pub fn crf_to_cache_key(crf: f32) -> i32 {
    let normalized = (crf * CACHE_KEY_MULTIPLIER).round();
    let key = normalized as i32;

    debug_assert!(
        (0..=630).contains(&key),
        "Cache key {} out of expected range [0, 630] for CRF {}",
        key,
        crf
    );

    key
}

#[inline]
pub fn cache_key_to_crf(key: i32) -> f32 {
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

pub fn required_iterations(min_crf: u8, max_crf: u8) -> u32 {
    let range = (max_crf - min_crf) as f64;
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

pub fn ssim_quality_grade(ssim: f64) -> &'static str {
    if ssim >= 0.98 {
        "Excellent (几乎无法区分)"
    } else if ssim >= 0.95 {
        "Good (视觉无损)"
    } else if ssim >= 0.90 {
        "Acceptable (轻微差异)"
    } else if ssim >= 0.85 {
        "Fair (可见差异)"
    } else {
        "Poor (明显质量损失)"
    }
}

pub fn psnr_quality_grade(psnr: f64) -> &'static str {
    if psnr.is_infinite() {
        "Lossless (完全相同)"
    } else if psnr >= 45.0 {
        "Excellent (几乎无法区分)"
    } else if psnr >= 40.0 {
        "Good (视觉无损)"
    } else if psnr >= 35.0 {
        "Acceptable (轻微差异)"
    } else if psnr >= 30.0 {
        "Fair (可见差异)"
    } else {
        "Poor (明显质量损失)"
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

pub fn ms_ssim_quality_grade(ms_ssim: f64) -> &'static str {
    if ms_ssim >= 0.95 {
        "Excellent (几乎无法区分)"
    } else if ms_ssim >= 0.90 {
        "Good (流媒体质量)"
    } else if ms_ssim >= 0.85 {
        "Acceptable (移动端质量)"
    } else if ms_ssim >= 0.80 {
        "Fair (可见差异)"
    } else {
        "Poor (明显质量损失)"
    }
}

pub fn format_ms_ssim(ms_ssim: f64) -> String {
    format!("{:.4}", ms_ssim)
}
