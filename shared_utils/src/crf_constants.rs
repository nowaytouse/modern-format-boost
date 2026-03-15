//! CRF Constants Module
//!
//! Unified CRF (Constant Rate Factor) range constants for all video encoders.
//! 统一的 CRF 常量定义，避免在多个文件中重复定义。

pub const HEVC_CRF_MIN: f32 = 0.0;

pub const HEVC_CRF_MAX: f32 = 51.0;

pub const HEVC_CRF_DEFAULT: f32 = 23.0;

pub const HEVC_CRF_VISUALLY_LOSSLESS: f32 = 18.0;

pub const HEVC_CRF_PRACTICAL_MAX: f32 = 32.0;

pub const AV1_CRF_MIN: f32 = 0.0;

pub const AV1_CRF_MAX: f32 = 63.0;

pub const AV1_CRF_DEFAULT: f32 = 30.0;

pub const AV1_CRF_VISUALLY_LOSSLESS: f32 = 20.0;

pub const AV1_CRF_PRACTICAL_MAX: f32 = 45.0;

pub const VP9_CRF_MIN: f32 = 0.0;

pub const VP9_CRF_MAX: f32 = 63.0;

pub const VP9_CRF_DEFAULT: f32 = 31.0;

pub const X264_CRF_MIN: f32 = 0.0;

pub const X264_CRF_MAX: f32 = 51.0;

pub const X264_CRF_DEFAULT: f32 = 23.0;

pub const CRF_CACHE_KEY_MULTIPLIER: f32 = 100.0;

pub const CRF_CACHE_MAX_VALID: f32 = 63.99;

pub const NORMAL_MAX_ITERATIONS: u32 = 60;

pub const EMERGENCY_MAX_ITERATIONS: u32 = 500;

use std::sync::atomic::{AtomicU32, Ordering};

// To store f32 in AtomicU32, we multiply by 100.0 and round.
pub static GLOBAL_LAST_HIT_CRF_AV1: AtomicU32 = AtomicU32::new(0);
pub static GLOBAL_LAST_HIT_CRF_HEVC: AtomicU32 = AtomicU32::new(0);

pub fn update_global_last_hit_crf_av1(crf: f32) {
    if crf > 0.0 {
        GLOBAL_LAST_HIT_CRF_AV1.store((crf * 100.0).round() as u32, Ordering::Relaxed);
    }
}

pub fn get_global_last_hit_crf_av1() -> Option<f32> {
    let val = GLOBAL_LAST_HIT_CRF_AV1.load(Ordering::Relaxed);
    if val > 0 {
        Some(val as f32 / 100.0)
    } else {
        None
    }
}

pub fn update_global_last_hit_crf_hevc(crf: f32) {
    if crf > 0.0 {
        GLOBAL_LAST_HIT_CRF_HEVC.store((crf * 100.0).round() as u32, Ordering::Relaxed);
    }
}

pub fn get_global_last_hit_crf_hevc() -> Option<f32> {
    let val = GLOBAL_LAST_HIT_CRF_HEVC.load(Ordering::Relaxed);
    if val > 0 {
        Some(val as f32 / 100.0)
    } else {
        None
    }
}
