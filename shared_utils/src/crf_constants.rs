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
