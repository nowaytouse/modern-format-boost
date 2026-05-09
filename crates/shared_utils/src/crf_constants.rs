//! CRF Constants Module
//!
//! Unified CRF (Constant Rate Factor) range constants for all video encoders.
//! Unified CRF constants definition to avoid duplication across multiple files.

pub const HEVC_CRF_MIN: f64 = crate::constants::HEVC_CRF_MIN_F64;

pub const HEVC_CRF_MAX: f64 = crate::constants::HEVC_CRF_MAX_F64;

pub const HEVC_CRF_DEFAULT: f64 = crate::constants::HEVC_CRF_DEFAULT_F64;

pub const HEVC_CRF_VISUALLY_LOSSLESS: f64 = crate::constants::HEVC_CRF_VISUALLY_LOSSLESS_F64;

pub const HEVC_CRF_PRACTICAL_MAX: f64 = crate::constants::HEVC_CRF_PRACTICAL_MAX_F64;

pub const AV1_CRF_MIN: f64 = crate::constants::AV1_CRF_MIN_F64;

pub const AV1_CRF_MAX: f64 = crate::constants::AV1_CRF_MAX_F64;

pub const AV1_CRF_DEFAULT: f64 = crate::constants::AV1_CRF_DEFAULT_F64;

pub const AV1_CRF_VISUALLY_LOSSLESS: f64 = crate::constants::AV1_CRF_VISUALLY_LOSSLESS_F64;

pub const AV1_CRF_PRACTICAL_MAX: f64 = crate::constants::AV1_CRF_PRACTICAL_MAX_F64;

pub const VP9_CRF_MIN: f64 = crate::constants::VP9_CRF_MIN_F64;

pub const VP9_CRF_MAX: f64 = crate::constants::VP9_CRF_MAX_F64;

pub const VP9_CRF_DEFAULT: f64 = crate::constants::VP9_CRF_DEFAULT_F64;

pub const X264_CRF_MIN: f64 = crate::constants::X264_CRF_MIN_F64;

pub const X264_CRF_MAX: f64 = crate::constants::X264_CRF_MAX_F64;

pub const X264_CRF_DEFAULT: f64 = crate::constants::X264_CRF_DEFAULT_F64;

pub const CRF_CACHE_KEY_MULTIPLIER: f64 = crate::constants::CRF_CACHE_KEY_MULTIPLIER;

pub const CRF_CACHE_MAX_VALID: f64 = crate::constants::CRF_CACHE_MAX_VALID;

pub const NORMAL_MAX_ITERATIONS: u32 = crate::constants::NORMAL_MAX_ITERATIONS;

pub const EMERGENCY_MAX_ITERATIONS: u32 = crate::constants::EMERGENCY_MAX_ITERATIONS;

use std::sync::atomic::{AtomicU32, Ordering};

// To store f64 in AtomicU32, we multiply by 100.0 and round.
pub static GLOBAL_LAST_HIT_CRF_AV1: AtomicU32 = AtomicU32::new(0);
pub static GLOBAL_LAST_HIT_CRF_HEVC: AtomicU32 = AtomicU32::new(0);

pub fn update_global_last_hit_crf_av1(crf: f64) {
    if crf > 0.0 {
        GLOBAL_LAST_HIT_CRF_AV1.store(
            crate::numeric_cast::f64_to_u32_sat((crf * 100.0).round()),
            Ordering::Relaxed,
        );
    }
}

pub fn get_global_last_hit_crf_av1() -> Option<f64> {
    let val = GLOBAL_LAST_HIT_CRF_AV1.load(Ordering::Relaxed);
    if val > 0 {
        Some(crate::numeric_cast::u32_to_f64(val) / 100.0)
    } else {
        None
    }
}

pub fn update_global_last_hit_crf_hevc(crf: f64) {
    if crf > 0.0 {
        GLOBAL_LAST_HIT_CRF_HEVC.store(
            crate::numeric_cast::f64_to_u32_sat((crf * 100.0).round()),
            Ordering::Relaxed,
        );
    }
}

pub fn get_global_last_hit_crf_hevc() -> Option<f64> {
    let val = GLOBAL_LAST_HIT_CRF_HEVC.load(Ordering::Relaxed);
    if val > 0 {
        Some(crate::numeric_cast::u32_to_f64(val) / 100.0)
    } else {
        None
    }
}
