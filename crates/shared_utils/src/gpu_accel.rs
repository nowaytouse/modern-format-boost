//! GPU Acceleration Module - Unified hardware encoder detection and selection
//!
//! 🔥 v4.9: Providing unified GPU acceleration support for all four tools
//!
//! ## Supported Hardware Encoders
//!
//! | Platform | HEVC Encoder | AV1 Encoder | H.264 Encoder |
//! |------|------------|-----------|--------------|
//! | NVIDIA | `hevc_nvenc` | `av1_nvenc` | `h264_nvenc` |
//! | Apple Silicon | `hevc_videotoolbox` | - | `h264_videotoolbox` |
//! | Intel QSV | `hevc_qsv` | `av1_qsv` | `h264_qsv` |
//! | AMD AMF | `hevc_amf` | `av1_amf` | `h264_amf` |
//! | VAAPI (Linux) | `hevc_vaapi` | `av1_vaapi` | `h264_vaapi` |
//!
//! ## Usage
//!
//! ```rust
//! use shared_utils::gpu_accel::{GpuAccel, GpuEncoder};
//!
//! let gpu = GpuAccel::detect();
//! if let Some(encoder) = gpu.get_hevc_encoder() {
//!     println!("Using GPU encoder: {}", encoder.ffmpeg_name());
//! }
//! ```

use chrono::{DateTime, FixedOffset, Utc};
use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crate::explore_strategy::CrfCache;

fn beijing_time_now() -> String {
    // UTC+8 (28800 seconds) is always a valid fixed offset
    let beijing = FixedOffset::east_opt(8 * 3600).unwrap_or_else(|| {
        FixedOffset::east_opt(0).unwrap_or_else(|| unsafe { std::hint::unreachable_unchecked() })
    });
    let now: DateTime<Utc> = Utc::now();
    now.with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S (UTC+8)")
        .to_string()
}

fn describe_thread_panic(payload: Box<dyn Any + Send + 'static>) -> String {
    payload.downcast::<String>().map_or_else(
        |payload| {
            payload.downcast::<&'static str>().map_or_else(
                |_| "non-string panic payload".to_string(),
                |msg| (*msg).to_string(),
            )
        },
        |msg| *msg,
    )
}

struct StderrCapture {
    lines: Arc<Mutex<VecDeque<String>>>,
    max_lines: usize,
}

impl StderrCapture {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(max_lines))),
            max_lines,
        }
    }

    fn spawn_capture_thread(&self, stderr: std::process::ChildStderr) -> JoinHandle<()> {
        use std::io::{BufRead, BufReader};

        let lines = Arc::clone(&self.lines);
        let max = self.max_lines;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let mut buf = lines.lock().unwrap_or_else(|err| {
                            tracing::warn!(
                                "GPU stderr capture buffer mutex was poisoned; recovering buffered stderr state"
                            );
                            err.into_inner()
                        });
                        if buf.len() >= max {
                            buf.pop_front();
                        }
                        buf.push_back(line);
                    }
                    Err(err) => {
                        tracing::warn!("Failed to read GPU encoder stderr: {}", err);
                        break;
                    }
                }
            }
        })
    }

    fn get_lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

/// Total duration (seconds) to sample for GPU probe/SSIM when video is short. Longer segments improve adaptation to varying content.
pub const GPU_SAMPLE_DURATION: f32 = 60.0;

/// Duration (seconds) per segment in multi-segment sampling (5 segments). Longer segments improve SSIM representativeness across media types.
pub const GPU_SEGMENT_DURATION: f32 = 15.0;

/// Ultimate mode: longer sample for better accuracy (was 45.0)
pub const GPU_SAMPLE_DURATION_ULTIMATE: f32 = 60.0;

/// Ultimate mode: longer segment per position (5 segments = 65s total, was 50s)
pub const GPU_SEGMENT_DURATION_ULTIMATE: f32 = 13.0;

/// Number of segments to sample in multi-segment GPU probing.
pub const GPU_SAMPLE_SEGMENTS: usize = 5;

fn collect_vf_filters(vf_args: &[String]) -> Vec<String> {
    let mut filters = Vec::new();
    let mut idx = 0;

    while idx + 1 < vf_args.len() {
        let current = vf_args.get(idx);
        let next = vf_args.get(idx + 1);
        if current.is_some_and(|v| v == "-vf") && next.is_some_and(|v| !v.is_empty()) {
            if let Some(n) = next {
                filters.push(n.clone());
            }
            idx += 2;
        } else {
            idx += 1;
        }
    }

    filters
}

#[must_use]
pub(crate) fn build_multi_segment_sampling_filter(
    duration: f64,
    ultimate_mode: bool,
) -> Option<String> {
    if duration < 60.0 {
        return None;
    }

    let seg_dur = if ultimate_mode {
        GPU_SEGMENT_DURATION_ULTIMATE
    } else {
        GPU_SEGMENT_DURATION
    };
    let positions = [
        0.0,
        duration * 0.25,
        duration * 0.50,
        duration * 0.75,
        (duration * 0.90).max(duration - f64::from(seg_dur)),
    ];

    Some(format!(
        "select='{}',setpts=N/FRAME_RATE/TB",
        positions
            .iter()
            .map(|&pos| format!("between(t,{:.1},{:.1})", pos, pos + f64::from(seg_dur)))
            .collect::<Vec<_>>()
            .join("+")
    ))
}

fn build_sampling_vf_args(vf_args: &[String], duration: f64, ultimate_mode: bool) -> Vec<String> {
    let mut filters = Vec::new();
    if let Some(prefix) = build_multi_segment_sampling_filter(duration, ultimate_mode) {
        filters.push(prefix);
    }
    filters.extend(collect_vf_filters(vf_args));

    if filters.is_empty() {
        Vec::new()
    } else {
        vec!["-vf".to_string(), filters.join(",")]
    }
}

/// Step size for CRF adjustments during GPU coarse search.
pub const GPU_COARSE_STEP: f32 = 1.0;

/// Absolute maximum number of iterations allowed in GPU coarse search.
pub const GPU_ABSOLUTE_MAX_ITERATIONS: u32 = 750;

/// Maximum number of iterations for GPU coarse search (alias for `GPU_ABSOLUTE_MAX_ITERATIONS`).
pub const GPU_MAX_ITERATIONS: u32 = GPU_ABSOLUTE_MAX_ITERATIONS;

const GPU_NEGATIVE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone)]
struct CachedGpuAccel {
    accel: GpuAccel,
    diagnostics: Vec<String>,
    last_probe: std::time::Instant,
}

impl CachedGpuAccel {
    fn probe_now() -> Self {
        let (accel, diagnostics) = GpuAccel::detect_internal();
        Self {
            accel,
            diagnostics,
            last_probe: std::time::Instant::now(),
        }
    }

    fn should_refresh(&self) -> bool {
        !self.accel.enabled && self.last_probe.elapsed() >= GPU_NEGATIVE_CACHE_TTL
    }
}

static GPU_ACCEL: OnceLock<Mutex<CachedGpuAccel>> = OnceLock::new();

/// Maximum concurrent GPU encode tasks (probe/encode). Read from env `MODERN_FORMAT_BOOST_GPU_CONCURRENCY` (default 4).
fn gpu_concurrency_max() -> usize {
    static CACHE: OnceLock<usize> = OnceLock::new();
    *CACHE.get_or_init(|| {
        std::env::var("MODERN_FORMAT_BOOST_GPU_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4)
    })
}

static GPU_CONCURRENCY_CURRENT: Mutex<usize> = Mutex::new(0);
static GPU_CONCURRENCY_CVAR: Condvar = Condvar::new();

fn acquire_gpu_slot() {
    let max = gpu_concurrency_max();
    let mut g = GPU_CONCURRENCY_CURRENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while *g >= max {
        g = GPU_CONCURRENCY_CVAR
            .wait(g)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    *g += 1;
}

fn release_gpu_slot() {
    let mut g = GPU_CONCURRENCY_CURRENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *g = g.saturating_sub(1);
    drop(g);
    GPU_CONCURRENCY_CVAR.notify_one();
}

/// Guard that releases a GPU concurrency slot on drop.
struct GpuSlotGuard;

impl Drop for GpuSlotGuard {
    fn drop(&mut self) {
        release_gpu_slot();
    }
}

#[cfg(target_os = "linux")]
fn vaapi_device_path() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            std::env::var("MODERN_FORMAT_BOOST_VAAPI_DEVICE")
                .or_else(|_| std::env::var("VAAPI_DEVICE"))
                .unwrap_or_else(|_| "/dev/dri/renderD128".to_string())
        })
        .as_str()
}

fn temp_extension_for(output: &std::path::Path, suffix: &str) -> String {
    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("MP4");
    format!("{suffix}.{ext}")
}

/// Returns a temp extension string (e.g. "`gpu_temp.mp4`") for the given output path.
/// Used by callers and by warmup encoding internally via `temp_extension_for`(_, "warmup").
#[must_use]
pub fn derive_gpu_temp_extension(output: &std::path::Path) -> String {
    temp_extension_for(output, "gpu_temp")
}

/// The type of GPU hardware encoder available on this system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuType {
    /// NVIDIA NVENC encoder.
    Nvidia,
    /// Apple `VideoToolbox` encoder (Apple Silicon).
    Apple,
    /// Intel Quick Sync Video encoder.
    IntelQsv,
    /// AMD AMF encoder.
    AmdAmf,
    /// VA-API encoder (Linux).
    Vaapi,
    /// No GPU encoder available; falls back to CPU.
    None,
}

impl std::fmt::Display for GpuType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nvidia => write!(f, "NVIDIA NVENC"),
            Self::Apple => write!(f, "Apple VideoToolbox"),
            Self::IntelQsv => write!(f, "Intel QSV"),
            Self::AmdAmf => write!(f, "AMD AMF"),
            Self::Vaapi => write!(f, "VA-API"),
            Self::None => write!(f, "None (CPU)"),
        }
    }
}

/// Represents a specific GPU hardware encoder with its configuration parameters.
#[derive(Debug, Clone)]
pub struct GpuEncoder {
    /// The GPU type this encoder belongs to.
    pub gpu_type: GpuType,
    /// The `FFmpeg` encoder name (e.g., "`hevc_nvenc`").
    pub name: &'static str,
    /// The codec this encoder produces (e.g., "hevc", "av1", "h264").
    pub codec: &'static str,
    /// Whether this encoder supports CRF-based quality control.
    pub supports_crf: bool,
    /// The `FFmpeg` parameter name used for CRF/quality (e.g., "cq", "q:v").
    pub crf_param: &'static str,
    /// The valid CRF range as (min, max) inclusive.
    pub crf_range: (u8, u8),
    /// Additional `FFmpeg` arguments passed to this encoder.
    pub extra_args: Vec<&'static str>,
}

impl GpuEncoder {
    /// Returns the `FFmpeg` encoder name (e.g., "`hevc_nvenc`").
    #[must_use]
    pub const fn ffmpeg_name(&self) -> &'static str {
        self.name
    }

    /// Converts a CRF value to encoder-specific arguments for this GPU encoder.
    ///
    /// For CRF-supporting encoders, returns the CRF parameter with clamping.
    /// For non-CRF encoders, falls back to bitrate-based arguments.
    #[must_use]
    pub fn get_crf_args(&self, crf: f32) -> Vec<String> {
        if self.supports_crf {
            let quality_value = if self.gpu_type == GpuType::Apple {
                crf.mul_add(-2.0, 100.0).clamp(1.0, 100.0)
            } else {
                crf.clamp(f32::from(self.crf_range.0), f32::from(self.crf_range.1))
            };

            vec![
                format!("-{}", self.crf_param),
                format!("{:.0}", quality_value),
            ]
        } else {
            let bitrate = crf_to_estimated_bitrate(crf, self.codec);
            vec!["-b:v".to_string(), format!("{}k", bitrate)]
        }
    }

    /// Returns the extra arguments for this encoder as a slice.
    #[must_use]
    pub fn extra_args(&self) -> &[&'static str] {
        &self.extra_args
    }
}

/// Represents the detected GPU acceleration capabilities.
#[derive(Debug, Clone)]
pub struct GpuAccel {
    /// The type of GPU detected.
    pub gpu_type: GpuType,
    /// Available HEVC hardware encoder, if any.
    pub hevc_encoder: Option<GpuEncoder>,
    /// Available AV1 hardware encoder, if any.
    pub av1_encoder: Option<GpuEncoder>,
    /// Available H.264 hardware encoder, if any.
    pub h264_encoder: Option<GpuEncoder>,
    /// Whether GPU acceleration is enabled and usable.
    pub enabled: bool,
}

impl Default for GpuAccel {
    fn default() -> Self {
        Self {
            gpu_type: GpuType::None,
            hevc_encoder: None,
            av1_encoder: None,
            h264_encoder: None,
            enabled: false,
        }
    }
}

impl GpuAccel {
    /// Detects available GPU acceleration and returns a cached snapshot.
    ///
    /// Successful probes stay cached. Failed probes are soft-cached and automatically retried
    /// after a short TTL so transient startup or device-busy failures do not latch CPU mode for
    /// the lifetime of the process.
    #[must_use]
    pub fn detect() -> Self {
        let cached = Self::cached_state();
        if cached.should_refresh() {
            return Self::detect_fresh();
        }
        cached.accel
    }

    /// Detects available GPU acceleration and forces an immediate re-probe if the cached state is
    /// currently unavailable.
    #[must_use]
    pub fn detect_with_retry() -> Self {
        let cached = Self::cached_state();
        if cached.accel.enabled {
            cached.accel
        } else {
            Self::detect_fresh()
        }
    }

    /// Performs a fresh GPU detection, bypassing the singleton cache.
    #[must_use]
    pub fn detect_fresh() -> Self {
        let probed = CachedGpuAccel::probe_now();
        let accel = probed.accel.clone();
        Self::store_cached_state(probed);
        accel
    }

    /// Returns diagnostics from the last GPU probe attempt.
    #[must_use]
    pub fn last_probe_diagnostics() -> Vec<String> {
        Self::cached_state().diagnostics
    }

    /// Prints GPU detection information to stderr.
    pub fn print_detection_info(&self) {
        let diagnostics = Self::last_probe_diagnostics();
        if !crate::progress_mode::is_verbose_mode() {
            if self.enabled {
                // Log to file only (stderr layer filters out target "gpu_detection" for less terminal noise).
                tracing::info!(target: "gpu_detection", "  GPU: {}", self.gpu_type);
            } else {
                // Surface why detection failed so the user has context without needing --verbose.
                let reason = diagnostics
                    .first()
                    .map_or("no supported encoder found", String::as_str);
                crate::log_eprintln!("⚠️ GPU probe failed ({}), using CPU encoding", reason);
            }
            return;
        }
        crate::log_eprintln!("Detecting GPU acceleration...");
        if self.enabled {
            crate::log_eprintln!("   ✅ GPU: {} detected", self.gpu_type);
            if let Some(enc) = &self.hevc_encoder {
                crate::log_eprintln!("      • HEVC: {}", enc.name);
            }
            if let Some(enc) = &self.av1_encoder {
                crate::log_eprintln!("      • AV1: {}", enc.name);
            }
            if let Some(enc) = &self.h264_encoder {
                crate::log_eprintln!("      • H.264: {}", enc.name);
            }
            for diagnostic in diagnostics.iter().take(3) {
                crate::log_eprintln!("      • Probe note: {}", diagnostic);
            }
        } else {
            crate::log_eprintln!("   ⚠️ No GPU acceleration available, using CPU encoding");
            for diagnostic in diagnostics.iter().take(3) {
                crate::log_eprintln!("      • {}", diagnostic);
            }
        }
    }

    fn cached_state() -> CachedGpuAccel {
        GPU_ACCEL
            .get_or_init(|| Mutex::new(CachedGpuAccel::probe_now()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn store_cached_state(state: CachedGpuAccel) {
        let cache = GPU_ACCEL.get_or_init(|| Mutex::new(state.clone()));
        *cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    fn detect_internal() -> (Self, Vec<String>) {
        let encoders = match get_available_encoders() {
            Ok(encoders) => encoders,
            Err(err) => return (Self::default(), vec![err]),
        };
        let mut diagnostics = Vec::new();

        #[cfg(target_os = "macos")]
        {
            if let Some(accel) = Self::try_videotoolbox(&encoders, &mut diagnostics) {
                return (accel, diagnostics);
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            if let Some(accel) = Self::try_nvenc(&encoders, &mut diagnostics) {
                return (accel, diagnostics);
            }
            if let Some(accel) = Self::try_qsv(&encoders, &mut diagnostics) {
                return (accel, diagnostics);
            }
        }

        #[cfg(target_os = "windows")]
        if let Some(accel) = Self::try_amf(&encoders, &mut diagnostics) {
            return (accel, diagnostics);
        }

        #[cfg(target_os = "linux")]
        if let Some(accel) = Self::try_vaapi(&encoders, &mut diagnostics) {
            return (accel, diagnostics);
        }

        if diagnostics.is_empty() {
            diagnostics.push(
                "ffmpeg reported no supported hardware video encoders for this platform"
                    .to_string(),
            );
        }

        (Self::default(), diagnostics)
    }

    fn assemble(
        gpu_type: GpuType,
        hevc_encoder: Option<GpuEncoder>,
        av1_encoder: Option<GpuEncoder>,
        h264_encoder: Option<GpuEncoder>,
    ) -> Option<Self> {
        if hevc_encoder.is_none() && av1_encoder.is_none() && h264_encoder.is_none() {
            None
        } else {
            Some(Self {
                gpu_type,
                hevc_encoder,
                av1_encoder,
                h264_encoder,
                enabled: true,
            })
        }
    }

    fn probe_listed_encoder(
        encoders: &[String],
        encoder: GpuEncoder,
        diagnostics: &mut Vec<String>,
    ) -> Option<GpuEncoder> {
        if !encoders.iter().any(|line| line.contains(encoder.name)) {
            return None;
        }

        let name = encoder.name;
        match test_encoder(&encoder) {
            Ok(()) => Some(encoder),
            Err(err) => {
                diagnostics.push(format!("{name}: {err}"));
                None
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn try_videotoolbox(encoders: &[String], diagnostics: &mut Vec<String>) -> Option<Self> {
        let hevc_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Apple,
                name: "hevc_videotoolbox",
                codec: "hevc",
                supports_crf: true,
                crf_param: "q:v",
                crf_range: (0, 100),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PROFILE_VIDEO,
                    crate::constants::VAL_MAIN,
                    crate::constants::FFMPEG_ARG_TAG_VIDEO,
                    crate::constants::FFMPEG_TAG_HVC1,
                ],
            },
            diagnostics,
        );
        let h264_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Apple,
                name: "h264_videotoolbox",
                codec: "h264",
                supports_crf: true,
                crf_param: "q:v",
                crf_range: (0, 100),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PROFILE_VIDEO,
                    crate::constants::VAL_HIGH,
                ],
            },
            diagnostics,
        );

        if hevc_encoder.is_none() && h264_encoder.is_none() {
            return None;
        }

        Self::assemble(GpuType::Apple, hevc_encoder, None, h264_encoder)
    }

    #[cfg(not(target_os = "macos"))]
    fn try_nvenc(encoders: &[String], diagnostics: &mut Vec<String>) -> Option<Self> {
        let hevc_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Nvidia,
                name: "hevc_nvenc",
                codec: "hevc",
                supports_crf: true,
                crf_param: "cq",
                crf_range: (0, 51),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PRESET,
                    crate::constants::VAL_P4,
                    crate::constants::FFMPEG_ARG_TUNE,
                    crate::constants::VAL_HQ,
                    crate::constants::FFMPEG_ARG_RC,
                    crate::constants::VAL_VBR,
                    crate::constants::FFMPEG_ARG_PROFILE_VIDEO,
                    crate::constants::VAL_MAIN,
                ],
            },
            diagnostics,
        );
        let av1_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Nvidia,
                name: "av1_nvenc",
                codec: "av1",
                supports_crf: true,
                crf_param: "cq",
                crf_range: (0, 63),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PRESET,
                    crate::constants::VAL_P4,
                    crate::constants::FFMPEG_ARG_TUNE,
                    crate::constants::VAL_HQ,
                    crate::constants::FFMPEG_ARG_RC,
                    crate::constants::VAL_VBR,
                ],
            },
            diagnostics,
        );
        let h264_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Nvidia,
                name: "h264_nvenc",
                codec: "h264",
                supports_crf: true,
                crf_param: "cq",
                crf_range: (0, 51),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PRESET,
                    crate::constants::VAL_P4,
                    crate::constants::FFMPEG_ARG_TUNE,
                    crate::constants::VAL_HQ,
                    crate::constants::FFMPEG_ARG_RC,
                    crate::constants::VAL_VBR,
                    crate::constants::FFMPEG_ARG_PROFILE_VIDEO,
                    crate::constants::VAL_HIGH,
                ],
            },
            diagnostics,
        );

        Self::assemble(GpuType::Nvidia, hevc_encoder, av1_encoder, h264_encoder)
    }

    #[cfg(not(target_os = "macos"))]
    fn try_qsv(encoders: &[String], diagnostics: &mut Vec<String>) -> Option<Self> {
        let hevc_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::IntelQsv,
                name: "hevc_qsv",
                codec: "hevc",
                supports_crf: true,
                crf_param: "global_quality",
                crf_range: (1, 51),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PRESET,
                    crate::constants::VAL_MEDIUM,
                    crate::constants::FFMPEG_ARG_PROFILE_VIDEO,
                    crate::constants::VAL_MAIN,
                ],
            },
            diagnostics,
        );
        let av1_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::IntelQsv,
                name: "av1_qsv",
                codec: "av1",
                supports_crf: true,
                crf_param: "global_quality",
                crf_range: (1, 63),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PRESET,
                    crate::constants::VAL_MEDIUM,
                ],
            },
            diagnostics,
        );
        let h264_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::IntelQsv,
                name: "h264_qsv",
                codec: "h264",
                supports_crf: true,
                crf_param: "global_quality",
                crf_range: (1, 51),
                extra_args: vec![
                    crate::constants::FFMPEG_ARG_PRESET,
                    crate::constants::VAL_MEDIUM,
                    crate::constants::FFMPEG_ARG_PROFILE_VIDEO,
                    crate::constants::VAL_HIGH,
                ],
            },
            diagnostics,
        );

        Self::assemble(GpuType::IntelQsv, hevc_encoder, av1_encoder, h264_encoder)
    }

    #[cfg(target_os = "windows")]
    fn try_amf(encoders: &[String], diagnostics: &mut Vec<String>) -> Option<GpuAccel> {
        let hevc_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::AmdAmf,
                name: "hevc_amf",
                codec: "hevc",
                supports_crf: true,
                crf_param: "qp_i",
                crf_range: (0, 51),
                extra_args: vec!["-quality", "quality", "-profile:v", "main"],
            },
            diagnostics,
        );
        let av1_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::AmdAmf,
                name: "av1_amf",
                codec: "av1",
                supports_crf: true,
                crf_param: "qp_i",
                crf_range: (0, 63),
                extra_args: vec!["-quality", "quality"],
            },
            diagnostics,
        );
        let h264_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::AmdAmf,
                name: "h264_amf",
                codec: "h264",
                supports_crf: true,
                crf_param: "qp_i",
                crf_range: (0, 51),
                extra_args: vec!["-quality", "quality", "-profile:v", "high"],
            },
            diagnostics,
        );

        Self::assemble(GpuType::AmdAmf, hevc_encoder, av1_encoder, h264_encoder)
    }

    #[cfg(target_os = "linux")]
    fn try_vaapi(encoders: &[String], diagnostics: &mut Vec<String>) -> Option<GpuAccel> {
        let hevc_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Vaapi,
                name: "hevc_vaapi",
                codec: "hevc",
                supports_crf: true,
                crf_param: "qp",
                crf_range: (0, 52),
                extra_args: vec!["-vaapi_device", vaapi_device_path(), "-profile:v", "main"],
            },
            diagnostics,
        );
        let av1_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Vaapi,
                name: "av1_vaapi",
                codec: "av1",
                supports_crf: true,
                crf_param: "qp",
                crf_range: (0, 63),
                extra_args: vec!["-vaapi_device", vaapi_device_path()],
            },
            diagnostics,
        );
        let h264_encoder = Self::probe_listed_encoder(
            encoders,
            GpuEncoder {
                gpu_type: GpuType::Vaapi,
                name: "h264_vaapi",
                codec: "h264",
                supports_crf: true,
                crf_param: "qp",
                crf_range: (0, 52),
                extra_args: vec!["-vaapi_device", vaapi_device_path(), "-profile:v", "high"],
            },
            diagnostics,
        );

        Self::assemble(GpuType::Vaapi, hevc_encoder, av1_encoder, h264_encoder)
    }

    /// Returns the available HEVC encoder, or `None` if GPU is not enabled.
    #[must_use]
    pub const fn get_hevc_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.hevc_encoder.as_ref()
        } else {
            None
        }
    }

    /// Returns the available AV1 encoder, or `None` if GPU is not enabled.
    #[must_use]
    pub const fn get_av1_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.av1_encoder.as_ref()
        } else {
            None
        }
    }

    /// Returns the available H.264 encoder, or `None` if GPU is not enabled.
    #[must_use]
    pub const fn get_h264_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.h264_encoder.as_ref()
        } else {
            None
        }
    }

    /// Returns `true` if GPU acceleration is available and enabled.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.enabled
    }

    /// Returns a human-readable description of the GPU acceleration status.
    #[must_use]
    pub fn description(&self) -> String {
        if self.enabled {
            format!("{} (Hardware Accelerated)", self.gpu_type)
        } else {
            "CPU (Software Encoding)".to_string()
        }
    }
}

fn get_available_encoders() -> Result<Vec<String>, String> {
    match crate::ffmpeg_builder::FfmpegBuilder::list_encoders() {
        Ok(stdout) => Ok(stdout
            .lines()
            .filter(|line| line.starts_with(" V"))
            .map(std::string::ToString::to_string)
            .collect()),
        Err(err) => Err(format!("failed to run ffmpeg -encoders: {err}")),
    }
}

fn summarize_ffmpeg_failure_line(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    for line in &lines {
        if line.contains("Cannot create compression session")
            || line.contains("permission")
            || line.contains("not supported")
            || line.contains("Device")
            || line.contains("No device")
            || line.contains("Try -allow_sw 1")
            || line.contains("Invalid argument")
        {
            return (*line).to_string();
        }
    }

    for line in lines.iter().rev() {
        if !matches!(
            *line,
            "Conversion failed!"
                | "unknown"
                | "Error initializing output stream 0:0 --"
                | "At least one output file must be specified"
        ) && !line.contains("Nothing was written into output file")
            && !line.contains("Error while opening encoder")
        {
            return (*line).to_string();
        }
    }

    lines.last().map_or_else(
        || "unknown ffmpeg error".to_string(),
        |line| (*line).to_string(),
    )
}

fn summarize_ffmpeg_failure_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    let stderr_summary = summarize_ffmpeg_failure_line(&stderr);
    if stderr_summary == "unknown ffmpeg error" {
        summarize_ffmpeg_failure_line(&stdout)
    } else {
        stderr_summary
    }
}

fn test_encoder(encoder: &GpuEncoder) -> Result<(), String> {
    let mid_crf = f32::midpoint(
        f32::from(encoder.crf_range.0),
        f32::from(encoder.crf_range.1),
    );

    // Run a single-frame null encode to confirm the encoder is functional.
    // On macOS, VideoToolbox may reject the first probe with "Cannot create
    // compression session" when the GPU is briefly contended.  We therefore
    // try once without software fallback, and on failure retry with
    // `-allow_sw 1` before giving up.
    let mut attempts: &[bool] = &[false];
    #[cfg(target_os = "macos")]
    {
        if encoder.gpu_type == GpuType::Apple {
            attempts = &[false, true];
        }
    }

    let mut last_err = String::new();
    for &allow_sw in attempts {
        let mut builder = crate::tool_builders::FfmpegBuilder::new();
        builder
            .hide_banner()
            .input_format("lavfi")
            .input("nullsrc=s=128x128:d=0.1")
            .codec_video(encoder.name);

        for arg in encoder.get_crf_args(mid_crf) {
            builder.arg(arg);
        }
        for arg in encoder.extra_args() {
            builder.arg(arg);
        }
        if allow_sw {
            builder.arg("-allow_sw").arg("1");
        }

        let output = builder
            .frames_v(1)
            .format("null")
            .output_pipe()
            .build()
            .output();

        match output {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => {
                last_err = summarize_ffmpeg_failure_output(&out.stdout, &out.stderr);
            }
            Err(err) => {
                last_err = err.to_string();
            }
        }
    }
    Err(last_err)
}

fn crf_to_estimated_bitrate(crf: f32, codec: &str) -> u32 {
    let base_bitrate = match codec {
        "av1" => 4000,
        "h264" => 8000,
        _ => 5000,
    };

    let crf_factor = match codec {
        "hevc" | "h264" => 0.9_f32.powf((crf - 23.0) / 6.0),
        "av1" => 0.9_f32.powf((crf - 30.0) / 6.0),
        _ => 1.0,
    };

    crate::numeric_cast::f32_to_u32_sat(
        crate::numeric_cast::f64_to_f32_lossy(f64::from(base_bitrate)) * crf_factor,
    )
}

/// Result of a smart sampling strategy for selecting representative video segments.
#[derive(Debug, Clone)]
pub struct SmartSampleResult {
    /// The `FFmpeg` filter string for the smart sample, if applicable.
    pub sample_filter: String,
    /// The actual duration of the sample used.
    pub actual_duration: f32,
    /// A human-readable description of the sampling strategy used.
    pub strategy: String,
}

/// Calculate a smart sample range for a video file.
///
/// # Errors
/// Returns an `anyhow::Result` if calculation fails.
pub fn calculate_smart_sample(
    input: &std::path::Path,
    total_duration: f32,
    target_sample_duration: f32,
) -> anyhow::Result<SmartSampleResult> {
    use anyhow::Context;

    if total_duration <= target_sample_duration * 1.2 {
        return Ok(SmartSampleResult {
            sample_filter: String::new(),
            actual_duration: total_duration,
            strategy: format!(
                "Full video ({total_duration:.1}s, close to target {target_sample_duration:.1}s)"
            ),
        });
    }

    let sample_ratio = target_sample_duration / total_duration;
    let sample_percentage = sample_ratio * 100.0;

    let scene_threshold = 0.3;
    let entropy_threshold = 6.0;

    let select_expr = if sample_ratio > 0.5 {
        format!(
            "gt(scene,{})+gt(entropy,{})",
            scene_threshold * 0.5,
            entropy_threshold * 0.8
        )
    } else if sample_ratio > 0.2 {
        format!("gt(scene,{scene_threshold})+gt(entropy,{entropy_threshold})")
    } else {
        format!(
            "gt(scene,{})*gt(entropy,{})",
            scene_threshold * 1.5,
            entropy_threshold * 1.2
        )
    };

    let test_output = crate::tool_builders::FfmpegBuilder::new()
        .hide_banner()
        .arg("-t")
        .arg("10")
        .input(input)
        .arg("-vf")
        .arg(format!("select='{select_expr}',showinfo"))
        .format("null")
        .output_pipe()
        .build()
        .output()
        .context("Failed to test smart sample filter")?;

    let stderr = String::from_utf8_lossy(&test_output.stderr);
    let frame_count = stderr.matches("n:").count();

    if frame_count == 0 {
        return Ok(SmartSampleResult {
            sample_filter: String::new(),
            actual_duration: target_sample_duration,
            strategy: format!(
                "Uniform sampling ({target_sample_duration:.1}s, {sample_percentage:.1}%)"
            ),
        });
    }

    Ok(SmartSampleResult {
        sample_filter: format!("select='{select_expr}',setpts=N/FRAME_RATE/TB"),
        actual_duration: target_sample_duration,
        strategy: format!(
            "Smart sampling ({target_sample_duration:.1}s, {sample_percentage:.1}%, scene+entropy)"
        ),
    })
}

/// A quality score combining SSIM, compression ratio, and a weighted combined score.
#[derive(Debug, Clone, Copy)]
pub struct QualityScore {
    /// The SSIM (Structural Similarity Index) score.
    pub ssim: f64,
    /// The ratio of output size to input size.
    pub compression_ratio: f64,
    /// A weighted combination of SSIM and compression ratio.
    pub combined_score: f64,
}

impl QualityScore {
    /// Returns the SSIM score as a typed `Ssim` value, if valid.
    #[inline]
    #[must_use]
    pub fn ssim_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.ssim).ok()
    }

    /// Returns whether the SSIM score meets the given threshold.
    #[inline]
    #[must_use]
    pub fn ssim_meets(&self, threshold: f64) -> bool {
        crate::float_compare::ssim_meets_threshold(self.ssim, threshold)
    }
}

/// The phase of a quality search: GPU coarse search or CPU fine search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPhase {
    /// GPU coarse search phase.
    Gpu,
    /// CPU fine search phase.
    Cpu,
}

/// Calculates a quality score from SSIM, file sizes, and search phase.
#[must_use]
pub fn calculate_quality_score(
    ssim: f64,
    output_size: u64,
    input_size: u64,
    phase: SearchPhase,
) -> QualityScore {
    let compression_ratio = if input_size == 0 {
        1.0
    } else {
        crate::numeric_cast::u64_to_f64(output_size) / crate::numeric_cast::u64_to_f64(input_size)
    };

    let (ssim_weight, size_weight): (f64, f64) = match phase {
        SearchPhase::Gpu => (0.4, 0.6),
        SearchPhase::Cpu => (0.7, 0.3),
    };

    let size_score = (1.0 - compression_ratio).max(0.0);
    let combined_score = ssim_weight.mul_add(ssim, size_weight * size_score);

    QualityScore {
        ssim,
        compression_ratio,
        combined_score,
    }
}

/// Returns whether the new quality score is meaningfully better than the old one.
#[must_use]
pub fn is_quality_better(
    new_score: &QualityScore,
    old_score: &QualityScore,
    min_ssim_threshold: f64,
) -> bool {
    if new_score.ssim < min_ssim_threshold {
        return false;
    }
    if old_score.combined_score <= 0.0 {
        return new_score.combined_score > 0.0;
    }
    let improvement =
        (new_score.combined_score - old_score.combined_score) / old_score.combined_score;
    improvement > 0.005
}

fn estimate_cpu_search_center_dynamic_impl(
    gpu_boundary: f32,
    gpu_type: GpuType,
    compression_potential: Option<f64>,
) -> f32 {
    let base_offset = match gpu_type {
        GpuType::Apple | GpuType::AmdAmf => 5.0,
        GpuType::Nvidia | GpuType::Vaapi => 4.0,
        GpuType::IntelQsv => 3.5,
        GpuType::None => 0.0,
    };

    let adjustment = compression_potential.map_or(0.0, |potential| {
        if potential < 0.3 {
            0.3
        } else if potential > 0.7 {
            -0.2
        } else {
            0.0
        }
    });

    gpu_boundary + base_offset + adjustment
}

/// Estimates the center of the CPU search range based on a GPU boundary CRF and GPU type.
///
/// `codec` is reserved for future codec-specific GPU→CPU CRF mapping; it is accepted for API
/// stability and intentionally ignored until tuning data exists.
#[must_use]
pub fn estimate_cpu_search_center_dynamic(
    gpu_boundary: f32,
    gpu_type: GpuType,
    codec: &str,
    compression_potential: Option<f64>,
) -> f32 {
    let _ = codec;
    estimate_cpu_search_center_dynamic_impl(gpu_boundary, gpu_type, compression_potential)
}

/// Estimates a CPU search range from a GPU range, adjusting for GPU type and codec.
#[must_use]
pub fn estimate_cpu_search_range(
    gpu_range: (f32, f32),
    gpu_type: GpuType,
    codec: &str,
    compression_potential: Option<f64>,
) -> (f32, f32) {
    let (gpu_low, gpu_high) = gpu_range;
    let cpu_low =
        estimate_cpu_search_center_dynamic(gpu_low, gpu_type, codec, compression_potential);
    let cpu_high =
        estimate_cpu_search_center_dynamic(gpu_high, gpu_type, codec, compression_potential);

    if cpu_low < cpu_high {
        (cpu_low, cpu_high)
    } else {
        (cpu_high, cpu_low)
    }
}

/// Estimates the CPU search center from a GPU boundary CRF and GPU type.
#[must_use]
pub fn estimate_cpu_search_center(gpu_boundary: f32, gpu_type: GpuType, codec: &str) -> f32 {
    estimate_cpu_search_center_dynamic(gpu_boundary, gpu_type, codec, None)
}

/// Converts a GPU boundary CRF to a CPU search range, clamped to min/max CRF.
#[must_use]
pub fn gpu_boundary_to_cpu_range(
    gpu_boundary: f32,
    gpu_type: GpuType,
    codec: &str,
    min_crf: f32,
    max_crf: f32,
) -> (f32, f32) {
    let cpu_center = estimate_cpu_search_center(gpu_boundary, gpu_type, codec);

    let cpu_low = gpu_boundary.max(min_crf);
    let cpu_high = (cpu_center + 3.0).min(max_crf);

    (cpu_low, cpu_high)
}

/// Converts a GPU CRF to an estimated CPU CRF (deprecated, use `estimate_cpu_search_center`).
#[deprecated(since = "5.0.1", note = "use estimate_cpu_search_center instead")]
#[must_use]
pub fn gpu_to_cpu_crf(gpu_crf: f32, gpu_type: GpuType, codec: &str) -> f32 {
    estimate_cpu_search_center(gpu_crf, gpu_type, codec)
}

/// Result of a GPU-based coarse search for optimal CRF.
#[derive(Debug, Clone)]
pub struct GpuCoarseResult {
    /// The CRF value at the compression boundary found by the GPU search.
    pub gpu_boundary_crf: f32,
    /// The output file size (bytes) at the best CRF found, if any compression point was found.
    pub gpu_best_size: Option<u64>,
    /// The SSIM score at the best CRF found, if measured.
    pub gpu_best_ssim: Option<f64>,
    /// The type of GPU used for the search.
    pub gpu_type: GpuType,
    /// The codec that was searched (e.g., "hevc", "av1", "h264").
    pub codec: String,
    /// Number of encode iterations performed during the search.
    pub iterations: u32,
    /// Whether a compression boundary was successfully found.
    pub found_boundary: bool,
    /// Whether the search included fine-tuning (more than 8 iterations).
    pub fine_tuned: bool,
    /// Log messages produced during the search.
    pub log: Vec<String>,
    /// The estimated input file size used for sample scaling.
    pub sample_input_size: u64,
    /// The CRF at which a quality ceiling was detected, if any.
    pub quality_ceiling_crf: Option<f32>,
    /// The SSIM score at the quality ceiling, if detected.
    pub quality_ceiling_ssim: Option<f64>,
}

impl GpuCoarseResult {
    /// Returns the best SSIM score as a typed `Ssim` value, if available.
    #[inline]
    #[must_use]
    pub fn best_ssim_typed(&self) -> Option<crate::types::Ssim> {
        self.gpu_best_ssim
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

    /// Returns the quality ceiling SSIM as a typed `Ssim` value, if available.
    #[inline]
    #[must_use]
    pub fn ceiling_ssim_typed(&self) -> Option<crate::types::Ssim> {
        self.quality_ceiling_ssim
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

    /// Returns the best file size as a typed `FileSize` value, if available.
    #[inline]
    pub fn best_size_typed(&self) -> Option<crate::types::FileSize> {
        self.gpu_best_size.map(crate::types::FileSize::new)
    }
}

/// Mapping between GPU and CPU CRF values for a specific GPU type and codec.
#[derive(Debug, Clone)]
pub struct CrfMapping {
    /// The GPU type this mapping applies to.
    pub gpu_type: GpuType,
    /// The codec this mapping is for (e.g., "hevc", "av1").
    pub codec: &'static str,
    /// The offset to add to GPU CRF to estimate equivalent CPU CRF.
    pub offset: f32,
    /// The uncertainty range in the CRF mapping.
    pub uncertainty: f32,
}

impl CrfMapping {
    /// Creates a CRF mapping for HEVC encoding with the given GPU type.
    #[must_use]
    pub const fn hevc(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple => (5.0, 0.5),
            GpuType::Nvidia => (3.8, 0.3),
            GpuType::IntelQsv => (3.5, 0.3),
            GpuType::AmdAmf => (4.8, 0.5),
            GpuType::Vaapi => (3.8, 0.4),
            GpuType::None => (0.0, 0.0),
        };
        Self {
            gpu_type,
            codec: "hevc",
            offset,
            uncertainty,
        }
    }

    /// Creates a CRF mapping for AV1 encoding with the given GPU type.
    #[must_use]
    pub const fn av1(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple | GpuType::None => (0.0, 0.0),
            GpuType::Nvidia | GpuType::Vaapi => (3.8, 0.4),
            GpuType::IntelQsv => (3.5, 0.3),
            GpuType::AmdAmf => (4.5, 0.5),
        };
        Self {
            gpu_type,
            codec: "av1",
            offset,
            uncertainty,
        }
    }

    /// Converts a GPU CRF to a CPU search range, returning (center, low, high).
    #[must_use]
    pub fn gpu_to_cpu_range(&self, gpu_crf: f32, min_crf: f32, max_crf: f32) -> (f32, f32, f32) {
        let center = (gpu_crf + self.offset).min(max_crf);
        let low = gpu_crf.max(min_crf);
        let high = (center + self.uncertainty).min(max_crf);
        (center, low, high)
    }

    /// Converts a CPU CRF back to the equivalent GPU CRF.
    #[must_use]
    pub fn cpu_to_gpu(&self, cpu_crf: f32) -> f32 {
        cpu_crf - self.offset
    }

    /// Prints the CRF mapping information to stderr.
    pub fn print_mapping_info(&self) {
        crate::log_eprintln!(
            "   📊 GPU/CPU CRF Mapping ({} - {}):",
            self.gpu_type,
            self.codec.to_uppercase()
        );
        if self.gpu_type == GpuType::Apple {
            crate::log_eprintln!("      • VideoToolbox q:v: 1=lowest, 100=highest quality");
            crate::log_eprintln!(
                "      • SSIM ceiling: 0.91~0.97 (content-dependent, cannot reach 0.98+)"
            );
            crate::log_eprintln!("      • Best value: q:v 75-80 (SSIM ~0.97, good compression)");
        } else {
            crate::log_eprintln!("      • GPU 60s sampling + step=2 → accurate boundary");
        }
        crate::log_eprintln!(
            "      • CPU offset: +{:.1} (CPU needs higher CRF for same compression)",
            self.offset
        );
        crate::log_eprintln!("      • 💡 CPU fine-tunes for SSIM 0.98+ (GPU max ~0.97)");
    }
}

/// Configuration for a GPU-based coarse CRF search.
#[derive(Debug, Clone)]
pub struct GpuCoarseConfig {
    /// The initial CRF value to start the search from.
    pub initial_crf: f32,
    /// The minimum CRF value allowed during the search.
    pub min_crf: f32,
    /// The maximum CRF value allowed during the search.
    pub max_crf: f32,
    /// The step size for CRF adjustments during the search.
    pub step: f32,
    /// Maximum number of iterations before the search stops.
    pub max_iterations: u32,
    /// When true (ultimate mode), use longer sample/segment durations for SSIM.
    pub ultimate_mode: bool,
    /// The encoding preset to use (e.g., Medium, Fast).
    pub preset: crate::types::EncoderPreset,
}

impl Default for GpuCoarseConfig {
    fn default() -> Self {
        Self {
            initial_crf: 18.0,
            min_crf: 0.0,
            max_crf: 51.0,
            step: GPU_COARSE_STEP,
            max_iterations: 10,
            ultimate_mode: false,
            preset: crate::types::EncoderPreset::Medium,
        }
    }
}

fn calculate_psnr_fast(input: &str, output: &str) -> Result<f64, String> {
    let psnr_output = crate::tool_builders::FfmpegBuilder::new()
        .input(std::path::Path::new(input))
        .input(std::path::Path::new(output))
        .filter_complex("[0:v][1:v]psnr=stats_file=-")
        .format("null")
        .output_pipe()
        .build()
        .output()
        .map_err(|e| format!("PSNR calculation failed: {e}"))?;

    if !psnr_output.status.success() {
        let stderr = String::from_utf8_lossy(&psnr_output.stderr);
        return Err(format!(
            "ffmpeg psnr failed: {}",
            crate::io_utils::tail_error_lines(&stderr, 5)
        ));
    }

    let stderr = String::from_utf8_lossy(&psnr_output.stderr);

    // Try multiple parsing strategies
    for line in stderr.lines() {
        // Strategy 1: Look for "psnr_avg:" in stats output
        if line.contains("psnr_avg:") {
            if let Some(pos) = line.find("psnr_avg:") {
                let after = &line[pos + 9..];
                if let Some(space_pos) = after.find(char::is_whitespace) {
                    if let Ok(psnr) = after[..space_pos].trim().parse::<f64>() {
                        return Ok(psnr);
                    }
                } else if let Ok(psnr) = after.trim().parse::<f64>() {
                    return Ok(psnr);
                }
            }
        }

        // Strategy 2: Look for "average:" in stats output
        if line.contains("average:") {
            if let Some(pos) = line.find("average:") {
                let after = &line[pos + 8..];
                let parts: Vec<&str> = after.split_whitespace().collect();
                if let Some(first) = parts.first() {
                    if let Ok(psnr) = first.trim().parse::<f64>() {
                        return Ok(psnr);
                    }
                }
            }
        }
    }

    Err(format!(
        "Failed to parse PSNR from ffmpeg output. Tail: {}",
        crate::io_utils::tail_error_lines(&stderr, 5)
    ))
}

#[derive(Debug)]
struct QualityCeilingDetector {
    samples: Vec<(f32, f64)>,
    plateau_threshold: f64,
    plateau_count: usize,
    ceiling_detected: bool,
}

impl QualityCeilingDetector {
    const fn new() -> Self {
        Self {
            samples: Vec::new(),
            plateau_threshold: 0.1,
            plateau_count: 0,
            ceiling_detected: false,
        }
    }

    fn add_sample(&mut self, crf: f32, quality: f64) -> bool {
        self.samples.push((crf, quality));

        if self.samples.len() >= 2 {
            let last = self.samples.last().map_or(0.0, |s| s.1);
            let prev = self
                .samples
                .get(self.samples.len().saturating_sub(2))
                .map_or(0.0, |s| s.1);
            let change = (last - prev).abs();

            if change < self.plateau_threshold {
                self.plateau_count += 1;

                if self.plateau_count >= 3 {
                    self.ceiling_detected = true;
                    return true;
                }
            } else {
                self.plateau_count = 0;
            }
        }

        false
    }

    fn get_ceiling(&self) -> Option<(f32, f64)> {
        if self.samples.len() >= 3 {
            self.samples
                .iter()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .copied()
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct PsnrSsimMapper {
    calibration_points: Vec<(f64, f64)>,
    calibrated: bool,
}

impl PsnrSsimMapper {
    const fn new() -> Self {
        Self {
            calibration_points: Vec::new(),
            calibrated: false,
        }
    }

    fn add_calibration_point(&mut self, psnr: f64, ssim: f64) {
        self.calibration_points.push((psnr, ssim));
        if self.calibration_points.len() >= 2 {
            self.calibrated = true;
        }
    }

    fn predict_ssim_from_psnr(&self, psnr: f64) -> Option<f64> {
        if !self.calibrated || self.calibration_points.len() < 2 {
            return None;
        }

        let mut points = self.calibration_points.clone();
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        for i in 0..points.len().saturating_sub(1) {
            let (psnr1, ssim1) = *points.get(i).unwrap_or(&(0.0, 0.0));
            let (psnr2, ssim2) = *points.get(i + 1).unwrap_or(&(0.0, 0.0));

            if psnr >= psnr1 && psnr <= psnr2 {
                let denom = psnr2 - psnr1;
                if denom.abs() < f64::EPSILON {
                    return Some(f64::midpoint(ssim1, ssim2));
                }
                let ratio = (psnr - psnr1) / denom;
                let predicted_ssim = ratio.mul_add(ssim2 - ssim1, ssim1);
                return Some(predicted_ssim);
            }
        }

        if psnr < points.first().map_or(0.0, |p| p.0) {
            let (psnr1, ssim1) = *points.first().unwrap_or(&(0.0, 0.0));
            let (psnr2, ssim2) = *points.get(1).unwrap_or(&(0.0, 0.0));
            let denom = psnr2 - psnr1;
            if denom.abs() < f64::EPSILON {
                return Some(ssim1);
            }
            let slope = (ssim2 - ssim1) / denom;
            Some(slope.mul_add(psnr - psnr1, ssim1))
        } else {
            let n = points.len();
            let (psnr1, ssim1) = *points.get(n.saturating_sub(2)).unwrap_or(&(0.0, 0.0));
            let (psnr2, ssim2) = *points.last().unwrap_or(&(0.0, 0.0));
            let denom = psnr2 - psnr1;
            if denom.abs() < f64::EPSILON {
                return Some(ssim2);
            }
            let slope = (ssim2 - ssim1) / denom;
            Some(slope.mul_add(psnr - psnr2, ssim2))
        }
    }

    fn get_mapping_quality(&self) -> f64 {
        if self.calibration_points.len() < 3 {
            return 0.5;
        }

        let n = crate::numeric_cast::usize_to_f64(self.calibration_points.len());
        (0.6 + (n / 20.0).min(0.35)).min(0.95)
    }

    fn print_report(&self) {
        if !self.calibrated {
            crate::log_eprintln!("   ⚠️ PSNR-SSIM mapping not calibrated");
            return;
        }

        crate::log_eprintln!("   📊 PSNR-SSIM Mapping Report:");
        crate::log_eprintln!(
            "      Calibration points: {}",
            self.calibration_points.len()
        );
        crate::log_eprintln!(
            "      Mapping quality: {:.1}%",
            self.get_mapping_quality() * 100.0
        );

        if self.calibration_points.len() >= 2 {
            let test_psnrs = vec![35.0, 38.0, 40.0, 42.0, 45.0];
            crate::log_eprintln!("      Example mappings:");
            for psnr in test_psnrs {
                if let Some(ssim) = self.predict_ssim_from_psnr(psnr) {
                    crate::log_eprintln!("         PSNR {:.1}dB → SSIM {:.4}", psnr, ssim);
                }
            }
        }
    }
}

/// Perform a GPU-based coarse search for optimal CRF.
///
/// # Errors
/// Returns an `anyhow::Result` if search fails.
#[must_use]
pub fn gpu_coarse_search(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    vf_args: &[String],
    progress_cb: Option<&dyn Fn(f32, u64)>,
) -> GpuCoarseResult {
    gpu_coarse_search_with_log(
        input,
        output,
        encoder,
        input_size,
        config,
        vf_args,
        progress_cb,
        None,
    )
}

/// Perform a GPU-based coarse search with custom logging.
///
/// # Errors
/// Returns an `anyhow::Result` if search fails.
#[must_use]
pub fn gpu_coarse_search_with_log(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    vf_args: &[String],
    progress_cb: Option<&dyn Fn(f32, u64)>,
    log_cb: Option<&dyn Fn(&str)>,
) -> GpuCoarseResult {
    let result = gpu_coarse_search_with_log_impl(
        input,
        output,
        encoder,
        input_size,
        config,
        vf_args,
        progress_cb,
        log_cb,
    );
    // Ensure temp output is always deleted, regardless of success/failure
    if let Err(err) = std::fs::remove_file(output) {
        if err.kind() != std::io::ErrorKind::NotFound {
            crate::progress_mode::emit_stderr(&format!(
                "⚠️ Failed to remove GPU coarse-search temp output {}: {}",
                output.display(),
                err
            ));
        }
    }
    result
}

#[allow(clippy::too_many_lines)]
fn gpu_coarse_search_with_log_impl(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    vf_args: &[String],
    progress_cb: Option<&dyn Fn(f32, u64)>,
    log_cb: Option<&dyn Fn(&str)>,
) -> GpuCoarseResult {
    use anyhow::{bail, Context};

    const LARGE_FILE_THRESHOLD: u64 = 500 * 1024 * 1024;
    const VERY_LARGE_FILE_THRESHOLD: u64 = 2 * 1024 * 1024 * 1024;
    const LONG_DURATION_THRESHOLD: f32 = 600.0;
    const VERY_LONG_DURATION_THRESHOLD: f32 = 3600.0;
    const WARMUP_DURATION: f32 = 5.0;
    const CHANGE_RATE_THRESHOLD: f64 = 0.02;

    let mut log = Vec::new();

    // Always show GPU search logs in ultimate mode for transparency
    let silent_mode = progress_cb.is_some() && !config.ultimate_mode;

    macro_rules! log_msg {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            if !silent_mode {
                if let Some(cb) = &log_cb {
                    cb(&msg);
                } else {
                    crate::log_eprintln!("{}", msg);
                }
            }
            log.push(msg);
        }};
    }

    let gpu = GpuAccel::detect_with_retry();

    if !gpu.is_available() {
        log_msg!("   ╔═══════════════════════════════════════════════════════════╗");
        log_msg!("   ║  ⚠️  FALLBACK: No GPU available!                          ║");
        log_msg!("   ║  Skipping GPU coarse search, using CPU-only mode          ║");
        log_msg!("   ║  This may take longer but results will be accurate        ║");
        log_msg!("   ╚═══════════════════════════════════════════════════════════╝");
        return GpuCoarseResult {
            gpu_boundary_crf: config.initial_crf,
            gpu_best_size: None,
            gpu_best_ssim: None,
            gpu_type: GpuType::None,
            codec: encoder.to_string(),
            iterations: 0,
            found_boundary: false,
            fine_tuned: false,
            log,
            sample_input_size: input_size,
            quality_ceiling_crf: None,
            quality_ceiling_ssim: None,
        };
    }

    let gpu_encoder = match encoder {
        "hevc" => gpu.get_hevc_encoder(),
        "av1" => gpu.get_av1_encoder(),
        "h264" => gpu.get_h264_encoder(),
        _ => None,
    };

    let Some(gpu_encoder) = gpu_encoder else {
        log_msg!("   ╔═══════════════════════════════════════════════════════════╗");
        log_msg!(
            "   ║  ⚠️  FALLBACK: No GPU encoder for {}!              ║",
            encoder.to_uppercase()
        );
        log_msg!("   ║  Skipping GPU coarse search, using CPU-only mode          ║");
        log_msg!("   ║  This may take longer but results will be accurate        ║");
        log_msg!("   ╚═══════════════════════════════════════════════════════════╝");
        return GpuCoarseResult {
            gpu_boundary_crf: config.initial_crf,
            gpu_best_size: None,
            gpu_best_ssim: None,
            gpu_type: gpu.gpu_type,
            codec: encoder.to_string(),
            iterations: 0,
            found_boundary: false,
            fine_tuned: false,
            log,
            sample_input_size: input_size,
            quality_ceiling_crf: None,
            quality_ceiling_ssim: None,
        };
    };

    let skip_gpu_size_threshold: u64 = if config.ultimate_mode {
        100 * 1024
    } else {
        500 * 1024
    };
    let skip_gpu_duration_threshold: f32 = if config.ultimate_mode { 1.0 } else { 3.0 };

    let quick_duration: f32 = {
        let duration_output = crate::tool_builders::FfprobeBuilder::new()
            .loglevel("error")
            .show_entries("format=duration")
            .print_format("default=noprint_wrappers=1:nokey=1")
            .arg("--")
            .input(input)
            .build()
            .output();

        duration_output
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(GPU_SAMPLE_DURATION)
    };

    let skip_gpu =
        input_size < skip_gpu_size_threshold || quick_duration < skip_gpu_duration_threshold;

    if skip_gpu {
        let reason = if input_size < skip_gpu_size_threshold {
            format!(
                "file too small ({:.1}KB < {}KB)",
                crate::numeric_cast::u64_to_f64(input_size) / 1024.0,
                skip_gpu_size_threshold / 1024
            )
        } else {
            format!("duration too short ({quick_duration:.1}s < {skip_gpu_duration_threshold:.1}s)")
        };
        log_msg!("   ⚡ Skip GPU: {} → CPU-only mode", reason);
        return GpuCoarseResult {
            gpu_boundary_crf: config.initial_crf,
            gpu_best_size: None,
            gpu_best_ssim: None,
            gpu_type: gpu.gpu_type,
            codec: encoder.to_string(),
            iterations: 0,
            found_boundary: false,
            fine_tuned: false,
            log,
            sample_input_size: input_size,
            quality_ceiling_crf: None,
            quality_ceiling_ssim: None,
        };
    }

    let is_large_file = input_size >= LARGE_FILE_THRESHOLD;
    let is_very_large_file = input_size >= VERY_LARGE_FILE_THRESHOLD;
    let is_long_video = quick_duration >= LONG_DURATION_THRESHOLD;
    let is_very_long_video = quick_duration >= VERY_LONG_DURATION_THRESHOLD;

    let (sample_duration_limit, skip_parallel) = if is_very_large_file || is_very_long_video {
        let limit = if config.ultimate_mode {
            50.0_f32
        } else {
            30.0_f32
        };
        let display_limit = crate::numeric_cast::f32_to_u32_sat(limit);
        log_msg!(
            "   ⚠️ Very large file detected → Conservative mode ({}s sample)",
            display_limit
        );
        (limit, true)
    } else if is_large_file || is_long_video {
        let limit = if config.ultimate_mode {
            70.0_f32
        } else {
            45.0_f32
        };
        let display_limit = crate::numeric_cast::f32_to_u32_sat(limit);
        log_msg!(
            "   📊 Large file detected → Sequential mode ({}s sample)",
            display_limit
        );
        (limit, true)
    } else {
        let limit = if config.ultimate_mode {
            GPU_SAMPLE_DURATION_ULTIMATE
        } else {
            GPU_SAMPLE_DURATION
        };
        let display_limit = crate::numeric_cast::f32_to_u32_sat(limit);
        log_msg!(
            "   ✅ Normal file → Parallel mode ({}s sample)",
            display_limit
        );
        (limit, false)
    };

    let max_iterations_limit = GPU_ABSOLUTE_MAX_ITERATIONS;

    log_msg!(
        "GPU Search ({}, {:.2}MB, {:.1}s)",
        gpu.gpu_type,
        crate::numeric_cast::u64_to_f64(input_size) / 1024.0 / 1024.0,
        quick_duration
    );
    log.push(format!(
        "GPU: {} | Input: {:.2}MB | Duration: {:.1}s",
        gpu.gpu_type,
        crate::numeric_cast::u64_to_f64(input_size) / 1024.0 / 1024.0,
        quick_duration
    ));

    let mut iterations = 0u32;

    let duration = quick_duration;
    let actual_sample_duration = duration.min(sample_duration_limit);

    let sample_input_size = if duration < 60.0 {
        input_size
    } else {
        let multi_segment_duration = if config.ultimate_mode {
            GPU_SAMPLE_DURATION_ULTIMATE
        } else {
            GPU_SAMPLE_DURATION
        };
        let ratio = multi_segment_duration / duration;
        crate::numeric_cast::f64_to_u64_sat(
            crate::numeric_cast::u64_to_f64(input_size) * f64::from(ratio),
        )
    };

    let warmup_duration = duration.min(WARMUP_DURATION);

    let encode_warmup = |crf: f32| -> anyhow::Result<u64> {
        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.extra_args();
        let warmup_output = output.with_extension(temp_extension_for(output, "warmup"));

        let mut builder = crate::tool_builders::FfmpegBuilder::new();
        builder
            .overwrite()
            .arg("-t")
            .arg(format!("{warmup_duration}"))
            .input(input)
            .vcodec_str(gpu_encoder.name);

        for arg in &crf_args {
            builder.arg(arg);
        }
        for arg in extra_args {
            builder.arg(arg);
        }

        builder.codec_audio("none").output(&warmup_output);

        let mut cmd = builder.build();

        let result = cmd.output().context("Failed to run warmup encode")?;
        let size = if result.status.success() {
            std::fs::metadata(&warmup_output).map_or(0, |m| m.len())
        } else {
            0
        };
        if let Err(err) = std::fs::remove_file(&warmup_output) {
            if err.kind() != std::io::ErrorKind::NotFound {
                crate::progress_mode::emit_stderr(&format!(
                    "⚠️ Failed to remove GPU warmup output {}: {}",
                    warmup_output.display(),
                    err
                ));
            }
        }
        Ok(size)
    };

    let warmup_input_size = if duration <= WARMUP_DURATION || duration == 0.0 {
        input_size
    } else {
        crate::numeric_cast::f64_to_u64_sat(
            crate::numeric_cast::u64_to_f64(input_size) * f64::from(warmup_duration)
                / f64::from(duration),
        )
    };

    let warmup_result = encode_warmup(config.max_crf);
    let can_compress_at_max = warmup_result
        .as_ref()
        .map_or(true, |size| *size < warmup_input_size);

    if !can_compress_at_max {
        log_msg!(
            "   ⚡ Warmup: max_crf={:.0} cannot compress → skip GPU search",
            config.max_crf
        );
        return GpuCoarseResult {
            gpu_boundary_crf: config.max_crf,
            gpu_best_size: warmup_result.ok(),
            gpu_best_ssim: None,
            gpu_type: gpu.gpu_type,
            codec: encoder.to_string(),
            iterations: 1,
            found_boundary: false,
            fine_tuned: false,
            log,
            sample_input_size,
            quality_ceiling_crf: None,
            quality_ceiling_ssim: None,
        };
    }
    log_msg!(
        "   🔥 Warmup: max_crf={:.0} can compress → continue search",
        config.max_crf
    );

    let seg_dur = if config.ultimate_mode {
        GPU_SEGMENT_DURATION_ULTIMATE
    } else {
        GPU_SEGMENT_DURATION
    };

    if duration >= 60.0 {
        log_msg!(
            "   📊 Multi-segment sampling: 5 segments × {:.0}s = {:.0}s (0%, 25%, 50%, 75%, 90%)",
            seg_dur,
            seg_dur * 5.0
        );
    } else {
        log_msg!("   📊 Full video sampling: {:.1}s", duration);
    }

    let encode_gpu = |crf: f32| -> anyhow::Result<u64> {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        use std::time::Instant;

        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.extra_args();

        let mut builder = crate::tool_builders::FfmpegBuilder::new();
        builder.overwrite();

        let use_multi_segment = duration >= 60.0;
        let sampling_vf_args =
            build_sampling_vf_args(vf_args, f64::from(duration), config.ultimate_mode);

        builder
            .input(input)
            .arg("-map")
            .arg("0:v:0")
            .vcodec_str(gpu_encoder.name);

        if !use_multi_segment {
            builder.arg("-t").arg(format!("{actual_sample_duration}"));
        }
        for arg in &sampling_vf_args {
            builder.arg(arg);
        }

        for arg in &crf_args {
            builder.arg(arg);
        }
        for arg in extra_args {
            builder.arg(arg);
        }

        builder
            .codec_audio("none")
            .arg("-progress")
            .arg("pipe:1")
            .output(output);

        let mut cmd = builder.build();
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;
        let start_time = Instant::now();

        let stderr_capture = StderrCapture::new(100);
        let stderr_handle = child
            .stderr
            .take()
            .map(|stderr| stderr_capture.spawn_capture_thread(stderr));

        crate::verbose_eprintln!("GPU encoding started - Beijing: {}", beijing_time_now());

        let mut last_progress_time = Instant::now();
        let mut fallback_logged = false;

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);

            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if let Some(val) = line.strip_prefix("out_time_us=") {
                            if let Ok(time_us) = val.parse::<u64>() {
                                if last_progress_time.elapsed().as_secs_f64() >= 1.0 {
                                    let current_secs =
                                        crate::numeric_cast::u64_to_f64(time_us) / 1_000_000.0;
                                    let pct = (current_secs / f64::from(actual_sample_duration)
                                        * 100.0)
                                        .min(100.0);
                                    let elapsed_secs = start_time.elapsed().as_secs_f64();
                                    let eta =
                                        if pct > 0.1 && current_secs > 0.0 && elapsed_secs > 0.0 {
                                            let speed = current_secs / elapsed_secs;
                                            if speed > 0.0 {
                                                crate::numeric_cast::f64_to_u64_sat(
                                                    ((f64::from(actual_sample_duration)
                                                        - current_secs)
                                                        / speed)
                                                        .max(0.0),
                                                )
                                            } else {
                                                0
                                            }
                                        } else {
                                            0
                                        };
                                    let speed = if current_secs > 0.0 {
                                        start_time.elapsed().as_secs_f64() / current_secs
                                    } else {
                                        0.0
                                    };

                                    let estimated_final_size = if let Ok(metadata) =
                                        std::fs::metadata(output)
                                    {
                                        let current_size = metadata.len();
                                        fallback_logged = false;
                                        crate::numeric_cast::f64_to_u64_sat(
                                            crate::numeric_cast::u64_to_f64(current_size)
                                                / pct.max(1.0)
                                                * 100.0,
                                        )
                                    } else {
                                        if !fallback_logged {
                                            crate::log_eprintln!(
                                                "Using linear estimation (metadata unavailable)"
                                            );
                                            fallback_logged = true;
                                        }
                                        crate::numeric_cast::f64_to_u64_sat(
                                            (crate::numeric_cast::u64_to_f64(sample_input_size)
                                                * (1.0 / pct.max(0.1)))
                                            .min(
                                                crate::numeric_cast::u64_to_f64(sample_input_size)
                                                    * 10.0,
                                            ),
                                        )
                                    };

                                    crate::log_eprintln!("⏳ Progress: {:.1}% ({:.1}s / {:.1}s) - ETA: {}s - Speed: {:.2}x",
                                        pct, current_secs, actual_sample_duration, eta, speed);

                                    if let Some(cb) = progress_cb {
                                        cb(crf, estimated_final_size);
                                    }
                                    last_progress_time = Instant::now();
                                }
                            }
                        }
                    }
                    Err(err) => {
                        crate::log_eprintln!(
                            "⚠️  Failed to read GPU encoder stdout progress stream: {}",
                            err
                        );
                        break;
                    }
                }
            }
        }

        let status = child.wait().context("Failed to wait for ffmpeg")?;

        if let Some(handle) = stderr_handle {
            if let Err(payload) = handle.join() {
                crate::log_eprintln!(
                    "⚠️  GPU stderr capture thread panicked: {}",
                    describe_thread_panic(payload)
                );
            }
        }

        if !status.success() {
            let stderr_lines = stderr_capture.get_lines();
            let stderr_text = if stderr_lines.is_empty() {
                "No stderr output".to_string()
            } else {
                stderr_lines.join("\n")
            };
            bail!(
                "GPU encoding failed (exit code: {:?})\nStderr:\n{}",
                status.code(),
                stderr_text
            );
        }

        crate::verbose_eprintln!("Encoding completed - Beijing: {}", beijing_time_now());

        Ok(std::fs::metadata(output)?.len())
    };

    let encode_parallel = |crfs: &[f32]| -> Vec<(f32, anyhow::Result<u64>)> {
        crfs.iter()
            .enumerate()
            .map(|(i, &crf)| {
                let crf_args = gpu_encoder.get_crf_args(crf);
                let extra_args: Vec<String> = gpu_encoder
                    .extra_args()
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect();
                let input_path = input.to_path_buf();
                let output_path = output.with_extension(format!("tmp{i}.mp4"));
                let encoder_name = gpu_encoder.name.to_string();
                let sample_dur = actual_sample_duration;

                thread::spawn(move || {
                    // Concurrency slot released on drop (see `GpuSlotGuard`).
                    let _gpu_slot_guard = GpuSlotGuard;
                    acquire_gpu_slot();
                    let mut builder = crate::tool_builders::FfmpegBuilder::new();
                    builder
                        .overwrite()
                        .arg("-t")
                        .arg(format!("{sample_dur}"))
                        .input(&input_path)
                        .vcodec_str(&encoder_name);

                    for arg in &crf_args {
                        builder.arg(arg);
                    }
                    for arg in &extra_args {
                        builder.arg(arg);
                    }

                    builder.codec_audio("none").output(&output_path);

                    let mut cmd = builder.build();

                    let result = cmd.output();

                    let size = match result {
                        Ok(out) if out.status.success() => std::fs::metadata(&output_path)
                            .map(|m| m.len())
                            .map_err(|e| anyhow::anyhow!("{e}")),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            Err(anyhow::anyhow!(
                                "GPU encoding failed: {}",
                                crate::io_utils::tail_error_lines(&stderr, 5)
                            ))
                        }
                        Err(e) => Err(anyhow::anyhow!("{e}")),
                    };

                    if let Err(err) = std::fs::remove_file(&output_path) {
                        if err.kind() != std::io::ErrorKind::NotFound {
                            crate::progress_mode::emit_stderr(&format!(
                                "⚠️ Failed to remove GPU probe output {}: {}",
                                output_path.display(),
                                err
                            ));
                        }
                    }

                    (crf, size)
                })
            })
            .collect::<Vec<_>>() // We do need to collect here to ensure all threads are SPAWNED before joining
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| (0.0, Err(anyhow::anyhow!("thread panic"))))
            })
            .collect()
    };

    let mut size_cache: CrfCache<u64> = CrfCache::new();
    let mut best_crf: Option<f32> = None;
    let mut best_size: Option<u64> = None;

    let encode_cached = |crf: f32, cache: &mut CrfCache<u64>| -> anyhow::Result<u64> {
        if let Some(&size) = cache.get(crf) {
            return Ok(size);
        }
        let size = encode_gpu(crf)?;
        cache.insert(crf, size);
        Ok(size)
    };

    let calc_change_rate = |prev: u64, curr: u64| -> f64 {
        if prev == 0 {
            return f64::MAX;
        }
        ((crate::numeric_cast::u64_to_f64(curr) - crate::numeric_cast::u64_to_f64(prev))
            / crate::numeric_cast::u64_to_f64(prev.max(1)))
        .abs()
    };

    let mut boundary_low: f32 = config.min_crf;
    let mut boundary_high: f32 = config.max_crf;
    let mut prev_size: Option<u64> = None;
    let mut found_compress_point = false;

    let use_initial =
        config.initial_crf >= config.min_crf + 5.0 && config.initial_crf <= config.max_crf - 5.0;

    let probe_crfs = if use_initial {
        log_msg!(
            "   🎯 [GPU] Using initial_crf {:.0} as search anchor",
            config.initial_crf
        );
        vec![config.initial_crf, config.max_crf, config.min_crf]
    } else {
        let mid_crf = f32::midpoint(config.min_crf, config.max_crf);
        log_msg!(
            "   ⚠️ [GPU] initial_crf {:.0} out of range, using mid_crf {:.0}",
            config.initial_crf,
            mid_crf
        );
        vec![mid_crf, config.max_crf, config.min_crf]
    };

    let probe_results = if skip_parallel {
        log_msg!("   ⚡ Skip parallel probe (large file mode)");
        let test_crf = probe_crfs.first().copied().unwrap_or(0.0);
        log_msg!("   🔄 [GPU] Testing CRF {:.0} (anchor point)...", test_crf);
        let single_result = encode_gpu(test_crf);
        if let Ok(size) = &single_result {
            size_cache.insert(test_crf, *size);
            iterations += 1;
            if let Some(cb) = progress_cb {
                cb(test_crf, *size);
            }
        }
        vec![(test_crf, single_result)]
    } else {
        log_msg!(
            "   🚀 [GPU] Parallel probe: CRF {:.0}, {:.0}, {:.0}",
            probe_crfs.first().copied().unwrap_or(0.0),
            probe_crfs.get(1).copied().unwrap_or(0.0),
            probe_crfs.get(2).copied().unwrap_or(0.0)
        );
        encode_parallel(&probe_crfs)
    };

    if !skip_parallel {
        for (crf, result) in &probe_results {
            if let Ok(size) = result {
                size_cache.insert(*crf, *size);
                iterations += 1;
                if let Some(cb) = progress_cb {
                    cb(*crf, *size);
                }
            }
        }
    }

    let initial_result = probe_results
        .iter()
        .find(|(c, _)| (*c - probe_crfs.first().copied().unwrap_or(0.0)).abs() < 0.1);
    let max_result = if probe_crfs.len() > 1 {
        probe_results
            .iter()
            .find(|(c, _)| (*c - probe_crfs.get(1).copied().unwrap_or(0.0)).abs() < 0.1)
    } else {
        None
    };
    let min_result = if probe_crfs.len() > 2 {
        probe_results
            .iter()
            .find(|(c, _)| (*c - probe_crfs.get(2).copied().unwrap_or(0.0)).abs() < 0.1)
    } else {
        None
    };

    if let Some((initial_crf_val, Ok(initial_size))) = initial_result {
        if *initial_size < sample_input_size {
            best_crf = Some(*initial_crf_val);
            best_size = Some(*initial_size);
            found_compress_point = true;

            boundary_low = *initial_crf_val;
            boundary_high = config.max_crf;
            log_msg!(
                "   ✅ initial_crf {:.0} compresses! Searching higher CRF [{:.0}, {:.0}]",
                initial_crf_val,
                boundary_low,
                boundary_high
            );

            if let Some((_, Ok(max_size))) = max_result {
                if *max_size < sample_input_size && *max_size < *initial_size {
                    best_crf = Some(config.max_crf);
                    best_size = Some(*max_size);
                    log_msg!(
                        "   📊 max_crf {:.0} is better: {:.1}% smaller",
                        config.max_crf,
                        (1.0 - crate::numeric_cast::u64_to_f64(*max_size)
                            / crate::numeric_cast::u64_to_f64(*initial_size))
                            * 100.0
                    );
                }
            }
        } else {
            boundary_low = config.min_crf;
            boundary_high = *initial_crf_val;
            prev_size = Some(*initial_size);
            log_msg!(
                "   ⚠️ initial_crf {:.0} cannot compress! Searching lower CRF [{:.0}, {:.0}]",
                initial_crf_val,
                boundary_low,
                boundary_high
            );

            if let Some((_, Ok(min_size))) = min_result {
                if *min_size < sample_input_size {
                    best_crf = Some(config.min_crf);
                    best_size = Some(*min_size);
                    found_compress_point = true;
                    log_msg!("   ✅ min_crf {:.0} compresses!", config.min_crf);
                }
            }
        }
    }

    let gpu_decay_factor: f32 = if config.ultimate_mode { 0.6 } else { 0.5 };
    let gpu_max_wall_hits: u32 = if config.ultimate_mode { 6 } else { 4 };
    let gpu_min_step: f32 = if config.ultimate_mode { 0.1 } else { 0.5 };

    let stage1_threshold = if config.ultimate_mode { 2.0 } else { 4.0 };

    if (boundary_high - boundary_low) > stage1_threshold {
        if found_compress_point {
            let crf_range = config.max_crf - boundary_low;
            let initial_step = (crf_range / 2.0).clamp(4.0, 15.0);

            log_msg!("   📈 Stage 1A: Curve model search upward (v6.0)");
            log_msg!(
                "      CRF range: {:.1} → Initial step: {:.1}",
                crf_range,
                initial_step
            );
            log_msg!(
                "      Strategy: step × {:.1} per wall hit, max {} hits",
                gpu_decay_factor,
                gpu_max_wall_hits
            );

            let mut stagnation_count = 0u32;
            let mut last_size = best_size.unwrap_or(0);
            let mut current_step = initial_step;
            let mut wall_hits: u32 = 0;
            let mut test_crf = boundary_low + current_step;
            let mut last_compressible_crf = boundary_low;
            let mut last_compressible_size = best_size.unwrap_or(0);

            while test_crf <= config.max_crf && iterations < max_iterations_limit {
                let cached = size_cache.get(test_crf).copied();
                let size_result =
                    cached.map_or_else(|| encode_cached(test_crf, &mut size_cache), Ok);

                match size_result {
                    Ok(size) => {
                        if cached.is_none() {
                            iterations += 1;
                            if let Some(cb) = progress_cb {
                                cb(test_crf, size);
                            }
                        }

                        let size_delta_pct = if last_size > 0 {
                            (crate::numeric_cast::u64_to_f64(size)
                                - crate::numeric_cast::u64_to_f64(last_size))
                            .abs()
                                / crate::numeric_cast::u64_to_f64(last_size.max(1))
                                * 100.0
                        } else {
                            100.0
                        };
                        last_size = size;

                        if size_delta_pct < 0.5 {
                            stagnation_count += 1;
                        } else {
                            stagnation_count = 0;
                        }

                        if size < sample_input_size {
                            last_compressible_crf = test_crf;
                            last_compressible_size = size;
                            best_crf = Some(test_crf);
                            best_size = Some(size);
                            boundary_low = test_crf;
                            log_msg!(
                                "   ✓ CRF {:.1}: {:.1}% (step {:.1}) → continue",
                                test_crf,
                                (crate::numeric_cast::u64_to_f64(size)
                                    / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                                    - 1.0)
                                    * 100.0,
                                current_step
                            );

                            if stagnation_count >= 3 {
                                log_msg!(
                                    "   ⚡ [GPU] Size plateau detected ({} stagnant iterations). Stopping Stage 1A.",
                                    stagnation_count
                                );
                                break;
                            }

                            test_crf += current_step;
                        } else {
                            wall_hits += 1;
                            log_msg!(
                                "   ✗ CRF {:.1}: WALL HIT #{} (size +{:.1}%)",
                                test_crf,
                                wall_hits,
                                (crate::numeric_cast::u64_to_f64(size)
                                    / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                                    - 1.0)
                                    * 100.0
                            );

                            if wall_hits >= gpu_max_wall_hits {
                                log_msg!(
                                    "   🧱 MAX WALL HITS ({})! Stopping at CRF {:.1}",
                                    gpu_max_wall_hits,
                                    last_compressible_crf
                                );
                                boundary_high = test_crf;
                                break;
                            }

                            let curve_step =
                                initial_step * gpu_decay_factor.powi(wall_hits.cast_signed());
                            let new_step = if curve_step < 1.0 {
                                gpu_min_step
                            } else {
                                curve_step
                            };

                            let phase_info = if new_step <= gpu_min_step + 0.01 {
                                "→ FINE TUNING".to_string()
                            } else {
                                format!("decay ×{gpu_decay_factor:.1}^{wall_hits}")
                            };
                            log_msg!(
                                "   ↩️ Curve backtrack: step {:.1} → {:.1} ({})",
                                current_step,
                                new_step,
                                phase_info
                            );

                            current_step = new_step;
                            boundary_high = test_crf;
                            test_crf = last_compressible_crf + current_step;
                            stagnation_count = 0; // Reset stagnation on wall hit
                        }
                    }
                    Err(_) => break,
                }
            }

            if last_compressible_crf > 0.0 {
                best_crf = Some(last_compressible_crf);
                best_size = Some(last_compressible_size);
            }
        } else {
            let crf_range = boundary_high - config.min_crf;
            let initial_step = (crf_range / 2.0).clamp(4.0, 15.0);

            log_msg!("   📉 Stage 1B: Curve model search downward (v6.0)");
            log_msg!(
                "      CRF range: {:.1} → Initial step: {:.1}",
                crf_range,
                initial_step
            );

            let mut current_step = initial_step;
            let mut wall_hits: u32 = 0;
            let mut test_crf = boundary_high - current_step;
            let mut last_fail_crf = boundary_high;

            while test_crf >= config.min_crf && iterations < max_iterations_limit {
                let cached = size_cache.get(test_crf).copied();
                let size_result =
                    cached.map_or_else(|| encode_cached(test_crf, &mut size_cache), Ok);

                match size_result {
                    Ok(size) => {
                        if cached.is_none() {
                            iterations += 1;
                            if let Some(cb) = progress_cb {
                                cb(test_crf, size);
                            }
                        }

                        if size < sample_input_size {
                            best_crf = Some(test_crf);
                            best_size = Some(size);
                            found_compress_point = true;
                            boundary_low = test_crf;
                            log_msg!(
                                "   ✓ CRF {:.1}: {:.1}% (step {:.1}) → found compress point",
                                test_crf,
                                (crate::numeric_cast::u64_to_f64(size)
                                    / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                                    - 1.0)
                                    * 100.0,
                                current_step
                            );
                            break;
                        }
                        wall_hits += 1;
                        log_msg!(
                            "   ✗ CRF {:.1}: WALL HIT #{} (size +{:.1}%)",
                            test_crf,
                            wall_hits,
                            (crate::numeric_cast::u64_to_f64(size)
                                / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                                - 1.0)
                                * 100.0
                        );

                        if wall_hits >= gpu_max_wall_hits {
                            log_msg!(
                                "   🧱 MAX WALL HITS ({})! Cannot find compress point",
                                gpu_max_wall_hits
                            );
                            break;
                        }

                        let curve_step =
                            initial_step * gpu_decay_factor.powi(wall_hits.cast_signed());
                        let new_step = if curve_step < 1.0 {
                            gpu_min_step
                        } else {
                            curve_step
                        };
                        log_msg!(
                            "   ↩️ Curve backtrack: step {:.1} → {:.1}",
                            current_step,
                            new_step
                        );

                        current_step = new_step;
                        last_fail_crf = test_crf;
                        prev_size = Some(size);
                        test_crf -= current_step;
                    }
                    Err(_) => break,
                }
            }

            let _ = last_fail_crf;
        }
    }

    let skip_stage2 = best_crf.is_some_and(|b| {
        let fract = (b * 2.0).fract();
        fract.abs() < 0.01 || (fract - 1.0).abs() < 0.01
    });

    if found_compress_point && !skip_stage2 && (boundary_high - boundary_low) > 1.0 {
        let mut lo = crate::numeric_cast::f32_to_i32_sat(boundary_low.ceil());
        let mut hi = crate::numeric_cast::f32_to_i32_sat(boundary_high.floor());

        let max_binary_iter = 5;
        let mut binary_iter = 0;

        while lo < hi && iterations < max_iterations_limit && binary_iter < max_binary_iter {
            binary_iter += 1;
            let mid = lo + (hi - lo) / 2;
            let test_crf = f32::from(u16::try_from(mid.max(0)).unwrap_or(0));

            if let Some(&cached_size) = size_cache.get(test_crf) {
                if cached_size < sample_input_size {
                    hi = mid;
                    best_crf = Some(test_crf);
                    best_size = Some(cached_size);
                } else {
                    lo = mid + 1;
                }
                continue;
            }

            match encode_cached(test_crf, &mut size_cache) {
                Ok(size) => {
                    iterations += 1;
                    if let Some(cb) = progress_cb {
                        cb(test_crf, size);
                    }

                    if let Some(prev) = prev_size {
                        let rate = calc_change_rate(prev, size);
                        if rate < CHANGE_RATE_THRESHOLD {
                            log_msg!("   ⚡ Stage2 early stop: Δ{:.3}%", rate * 100.0);
                            break;
                        }
                    }

                    if size < sample_input_size {
                        hi = mid;
                        best_crf = Some(test_crf);
                        best_size = Some(size);
                        prev_size = Some(size);
                    } else {
                        lo = mid + 1;
                    }
                }
                Err(_) => break,
            }
        }
    } else if skip_stage2 {
        log_msg!("   ⚡ Skip Stage2: boundary at 0.5 precision");
    }

    let mut ceiling_detector = QualityCeilingDetector::new();
    let mut psnr_ssim_mapper = PsnrSsimMapper::new();

    if let Some(mut current_best) = best_crf {
        if iterations >= max_iterations_limit {
            log_msg!(
                "   ⚡ Skip Stage3: reached absolute limit ({})",
                max_iterations_limit
            );
        } else {
            let stage3_step = if config.ultimate_mode { 0.1 } else { 0.5 };
            log_msg!(
                "   📍 Stage 3: Fine-tune with {:.1} step (quality ceiling detection)",
                stage3_step
            );

            let mut offset = stage3_step;
            let mut consecutive_small_improvements = 0;
            // `iterations` only increases on real encodes; cache hits can advance `offset`/`break`
            // without bumping it — bound total spins so the loop cannot run unbounded.
            let mut stage3_spins = 0u32;
            let stage3_spin_cap = max_iterations_limit.saturating_mul(8).max(512);

            while iterations < max_iterations_limit {
                stage3_spins += 1;
                if stage3_spins > stage3_spin_cap {
                    log_msg!("   ⚠️ Stage 3: stopping after spin safety cap ({stage3_spin_cap})");
                    break;
                }

                let test_crf = current_best - offset;

                if test_crf < config.min_crf {
                    log_msg!("   ⚡ Stop: reached min_crf {:.1}", config.min_crf);
                    break;
                }

                let result = if let Some(&cached_size) = size_cache.get(test_crf) {
                    log_msg!("   📦 Cache hit: CRF {:.1}", test_crf);
                    Ok(cached_size)
                } else {
                    let r = encode_cached(test_crf, &mut size_cache);
                    if r.is_ok() {
                        iterations += 1;
                    }
                    r
                };

                if let Ok(size) = result {
                    if let Some(cb) = progress_cb {
                        cb(test_crf, size);
                    }

                    if size < sample_input_size {
                        let improvement = best_size.map_or(0.0, |b| {
                            (crate::numeric_cast::u64_to_f64(b)
                                - crate::numeric_cast::u64_to_f64(size))
                                / crate::numeric_cast::u64_to_f64(b.max(1))
                                * 100.0
                        });
                        log_msg!("   ✓ CRF {:.1}: {:.1}% improvement", test_crf, improvement);

                        best_crf = Some(test_crf);
                        best_size = Some(size);
                        current_best = test_crf;

                        let input_str = input.to_string_lossy();
                        let output_str = output.to_string_lossy();
                        if let Ok(psnr) = calculate_psnr_fast(&input_str, &output_str) {
                            log_msg!("      📊 PSNR: {:.2}dB", psnr);

                            if ceiling_detector.add_sample(test_crf, psnr) {
                                if let Some((ceiling_crf, ceiling_psnr)) =
                                    ceiling_detector.get_ceiling()
                                {
                                    log_msg!("   🎯 GPU Quality Ceiling Detected!");
                                    log_msg!(
                                        "      └─ CRF {:.1}, PSNR {:.2}dB (PSNR plateau)",
                                        ceiling_crf,
                                        ceiling_psnr
                                    );
                                    log_msg!(
                                        "      └─ Further CRF reduction won't improve quality"
                                    );
                                    log_msg!("   ⚡ Stop: GPU reached its quality limit");
                                    break;
                                }
                            }
                        } else {
                            log_msg!("      ⚠️ PSNR calc failed, fallback to size-only");
                        }

                        if improvement < 0.5 {
                            consecutive_small_improvements += 1;
                            log_msg!(
                                "      ⚠️ Small improvement ({}/2)",
                                consecutive_small_improvements
                            );

                            if consecutive_small_improvements >= 2 {
                                log_msg!("   ⚡ Stop: 2 consecutive improvements < 0.5%");
                                break;
                            }
                        } else if improvement < 1.0 {
                            log_msg!("      ⚠️ Improvement < 1%, may stop soon");
                            consecutive_small_improvements += 1;

                            if consecutive_small_improvements >= 3 {
                                log_msg!("   ⚡ Stop: 3 consecutive improvements < 1%");
                                break;
                            }
                        } else {
                            consecutive_small_improvements = 0;
                        }

                        offset += 0.5;
                    } else {
                        log_msg!(
                            "   ✗ CRF {:.1} cannot compress → boundary reached",
                            test_crf
                        );
                        break;
                    }
                } else {
                    log_msg!("   ⚠️ Encoding failed at CRF {:.1}, stopping", test_crf);
                    break;
                }
            }

            if iterations >= max_iterations_limit {
                log_msg!(
                    "   ⚠️ Reached absolute iteration limit ({}) in Stage 3",
                    max_iterations_limit
                );
            }

            if ceiling_detector.ceiling_detected {
                if let Some((ceiling_crf, ceiling_psnr)) = ceiling_detector.get_ceiling() {
                    log_msg!("   ═══════════════════════════════════════════════════");
                    log_msg!("   🎯 GPU Quality Ceiling Summary:");
                    log_msg!("      CRF: {:.1}", ceiling_crf);
                    log_msg!("      PSNR: {:.2}dB", ceiling_psnr);
                    log_msg!("      Note: GPU encoder reached its quality limit");
                    log_msg!("      CPU encoding can break through this ceiling");
                }
            }
        }
    }

    let (last_tested_crf, found, fine_tuned) = best_crf.map_or_else(
        || (config.max_crf, false, false),
        |b| (b, true, iterations > 8),
    );

    let quality_ceiling_info = if ceiling_detector.ceiling_detected {
        ceiling_detector.get_ceiling()
    } else {
        None
    };

    let quality_ceiling_crf = quality_ceiling_info.map(|(crf, _psnr)| crf);

    let (gpu_ssim, gpu_psnr) = if found {
        log_msg!(
            "   📍 Final quality validation at CRF {:.1}",
            last_tested_crf
        );
        match encode_gpu(last_tested_crf) {
            Ok(_) => {
                let ssim_output = crate::tool_builders::FfmpegBuilder::new()
                    .input(input)
                    .input(output)
                    .filter_complex("ssim")
                    .format("null")
                    .output_pipe()
                    .build()
                    .output();

                let psnr_result =
                    calculate_psnr_fast(&input.to_string_lossy(), &output.to_string_lossy());

                let ssim = ssim_output.ok().and_then(|out| {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    stderr
                        .lines()
                        .find(|l| l.contains("SSIM") && l.contains("All:"))
                        .and_then(|line| line.find("All:").map(|pos| &line[pos + 4..]))
                        .and_then(|after_all| {
                            let val_str = after_all
                                .find(' ')
                                .map_or(after_all, |pos| &after_all[..pos]);
                            val_str.trim().parse::<f64>().ok()
                        })
                        .inspect(|&ssim| {
                            log_msg!("      📊 Final GPU SSIM: {:.6}", ssim);
                        })
                });

                let psnr = psnr_result.ok().inspect(|&p| {
                    log_msg!("      📊 Final GPU PSNR: {:.2}dB", p);
                });

                if let (Some(p), Some(s)) = (psnr, ssim) {
                    psnr_ssim_mapper.add_calibration_point(p, s);
                    log_msg!(
                        "      ✅ Added PSNR-SSIM calibration point: {:.2}dB → {:.6}",
                        p,
                        s
                    );
                }

                (ssim, psnr)
            }
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    let gpu_boundary_crf = quality_ceiling_info
        .map(|(crf, _)| crf)
        .map_or(last_tested_crf, |ceiling_crf| {
            log_msg!("   🎯 GPU Quality Ceiling Detected!");
            log_msg!("      └─ Ceiling CRF: {:.1} (PSNR plateau)", ceiling_crf);
            log_msg!("      └─ Last tested CRF: {:.1}", last_tested_crf);
            if !crate::float_compare::approx_eq_crf(ceiling_crf, last_tested_crf) {
                log_msg!("      └─ Boundary = Ceiling (lower CRFs are bloated, no quality gain)");
            }
            ceiling_crf
        });

    log_msg!("   ═══════════════════════════════════════════════════");
    if found {
        log_msg!(
            "   📊 GPU Boundary CRF: {:.1} (highest quality that compresses)",
            gpu_boundary_crf
        );
        if let Some(size) = best_size {
            let ratio = crate::numeric_cast::u64_to_f64(size)
                / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                * 100.0;
            log_msg!("   📊 GPU Best Size: {:.1}% of input", ratio);
        }
        if let Some(ssim) = gpu_ssim {
            let quality_hint = if ssim >= 0.97 {
                "🟢 Near ceiling"
            } else if ssim >= 0.95 {
                "🟡 Good"
            } else {
                "🟠 Below expected"
            };
            log_msg!("   📊 GPU Best SSIM: {:.6} {}", ssim, quality_hint);
        }
        if let Some(psnr) = gpu_psnr {
            log_msg!("   📊 GPU Best PSNR: {:.2}dB", psnr);
        }

        if psnr_ssim_mapper.calibrated {
            log_msg!("   ═══════════════════════════════════════════════════");
            psnr_ssim_mapper.print_report();
        }

        let mapping = match encoder {
            "av1" => CrfMapping::av1(gpu.gpu_type),
            _ => CrfMapping::hevc(gpu.gpu_type),
        };
        let (cpu_center, cpu_low, cpu_high) =
            mapping.gpu_to_cpu_range(gpu_boundary_crf, config.min_crf, config.max_crf);
        log_msg!(
            "   📊 CPU Search Range: [{:.1}, {:.1}] (center: {:.1})",
            cpu_low,
            cpu_high,
            cpu_center
        );
    } else {
        log_msg!("   ⚠️ No compression boundary found (file may be already compressed)");
    }
    log_msg!(
        "   📈 GPU Iterations: {} (fine-tuned: {})",
        iterations,
        if fine_tuned { "yes" } else { "no" }
    );

    if let Err(err) = std::fs::remove_file(output) {
        if err.kind() != std::io::ErrorKind::NotFound {
            crate::progress_mode::emit_stderr(&format!(
                "⚠️ Failed to remove final GPU coarse-search temp output {}: {}",
                output.display(),
                err
            ));
        }
    }

    GpuCoarseResult {
        gpu_boundary_crf,
        gpu_best_size: best_size,
        gpu_best_ssim: gpu_ssim,
        gpu_type: gpu.gpu_type,
        codec: encoder.to_string(),
        iterations,
        found_boundary: found,
        fine_tuned,
        log,
        sample_input_size,
        quality_ceiling_crf,
        quality_ceiling_ssim: gpu_ssim,
    }
}

/// Derives the CPU search range from a GPU coarse search result.
#[must_use]
pub fn get_cpu_search_range_from_gpu(
    gpu_result: &GpuCoarseResult,
    original_min_crf: f32,
    original_max_crf: f32,
) -> (f32, f32, f32) {
    if !gpu_result.found_boundary {
        let center = f32::midpoint(original_min_crf, original_max_crf);
        return (original_min_crf, original_max_crf, center);
    }

    let mapping = match gpu_result.codec.as_str() {
        "av1" => CrfMapping::av1(gpu_result.gpu_type),
        _ => CrfMapping::hevc(gpu_result.gpu_type),
    };

    mapping.gpu_to_cpu_range(
        gpu_result.gpu_boundary_crf,
        original_min_crf,
        original_max_crf,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_cpu_search_center() {
        let cpu_center = estimate_cpu_search_center(10.0, GpuType::Apple, "hevc");
        assert!(
            (cpu_center - 15.0).abs() < 0.1,
            "Expected ~15.0, got {cpu_center}"
        );

        let cpu_center = estimate_cpu_search_center(10.0, GpuType::Nvidia, "hevc");
        assert!(
            (cpu_center - 14.0).abs() < 0.1,
            "Expected ~14.0, got {cpu_center}"
        );

        let cpu_center = estimate_cpu_search_center(10.0, GpuType::None, "hevc");
        assert!(
            (cpu_center - 10.0).abs() < 0.1,
            "Expected ~10.0, got {cpu_center}"
        );
    }

    #[test]
    fn test_gpu_boundary_to_cpu_range() {
        let (low, high) = gpu_boundary_to_cpu_range(10.0, GpuType::Apple, "hevc", 8.0, 28.0);
        assert!(
            (low - 10.0).abs() < 0.1,
            "low={low} should be ~10.0 (GPU boundary)"
        );
        assert!(
            (15.0..=22.0).contains(&high),
            "high={high} should be in [15, 22]"
        );

        let (low, high) = gpu_boundary_to_cpu_range(12.0, GpuType::Nvidia, "hevc", 10.0, 28.0);
        assert!((low - 12.0).abs() < 0.1, "low should be GPU boundary");
        assert!(
            (10.0..=28.0).contains(&high),
            "high={high} should stay within CRF clamp"
        );
    }

    #[test]
    fn test_videotoolbox_crf_mapping_crf_0() {
        let encoder = GpuEncoder {
            gpu_type: GpuType::Apple,
            name: "hevc_videotoolbox",
            codec: "hevc",
            supports_crf: true,
            crf_param: "q:v",
            crf_range: (1, 100),
            extra_args: vec![],
        };

        let args = encoder.get_crf_args(0.0);
        assert_eq!(args, vec!["-q:v", "100"], "CRF 0 should map to q:v 100");
    }

    #[test]
    fn test_videotoolbox_crf_mapping_crf_51() {
        let encoder = GpuEncoder {
            gpu_type: GpuType::Apple,
            name: "hevc_videotoolbox",
            codec: "hevc",
            supports_crf: true,
            crf_param: "q:v",
            crf_range: (1, 100),
            extra_args: vec![],
        };

        let args = encoder.get_crf_args(51.0);
        assert_eq!(
            args,
            vec!["-q:v", "1"],
            "CRF 51 should clamp to q:v 1 (not negative)"
        );
    }

    #[test]
    fn test_videotoolbox_crf_mapping_various() {
        let encoder = GpuEncoder {
            gpu_type: GpuType::Apple,
            name: "hevc_videotoolbox",
            codec: "hevc",
            supports_crf: true,
            crf_param: "q:v",
            crf_range: (1, 100),
            extra_args: vec![],
        };

        let args = encoder.get_crf_args(1.0);
        assert_eq!(args, vec!["-q:v", "98"], "CRF 1 should map to q:v 98");

        let args = encoder.get_crf_args(25.0);
        assert_eq!(args, vec!["-q:v", "50"], "CRF 25 should map to q:v 50");

        let args = encoder.get_crf_args(50.0);
        assert_eq!(args, vec!["-q:v", "1"], "CRF 50 should clamp to q:v 1");
    }

    #[test]
    fn test_videotoolbox_crf_mapping_no_overflow() {
        let encoder = GpuEncoder {
            gpu_type: GpuType::Apple,
            name: "hevc_videotoolbox",
            codec: "hevc",
            supports_crf: true,
            crf_param: "q:v",
            crf_range: (1, 100),
            extra_args: vec![],
        };

        for crf in [
            0.0, 0.5, 1.0, 10.0, 20.0, 30.0, 40.0, 50.0, 51.0, 60.0, 100.0,
        ] {
            let args = encoder.get_crf_args(crf);
            let qv: f32 = args
                .get(1)
                .unwrap_or(&String::new())
                .parse()
                .unwrap_or_else(|e| panic!("error: {e:?}"));
            assert!(qv >= 1.0, "q:v should be >= 1, got {qv} for CRF {crf}");
            assert!(qv <= 100.0, "q:v should be <= 100, got {qv} for CRF {crf}");
        }
    }

    #[test]
    fn test_build_multi_segment_sampling_filter_for_long_videos() {
        let filter = build_multi_segment_sampling_filter(120.0, false)
            .unwrap_or_else(|| panic!("long videos should use multi-segment sampling"));
        assert!(filter.contains("between(t,0.0,15.0)"));
        assert!(filter.contains("between(t,30.0,45.0)"));
        assert!(filter.contains("between(t,60.0,75.0)"));
        assert!(filter.contains("between(t,90.0,105.0)"));
        assert!(filter.contains("between(t,108.0,123.0)"));
    }

    #[test]
    fn test_build_multi_segment_sampling_filter_skips_short_videos() {
        assert!(build_multi_segment_sampling_filter(59.9, true).is_none());
    }

    #[test]
    fn test_negative_gpu_cache_refresh_policy() {
        let stale_negative = CachedGpuAccel {
            accel: GpuAccel::default(),
            diagnostics: vec![],
            last_probe: std::time::Instant::now()
                .checked_sub(GPU_NEGATIVE_CACHE_TTL)
                .unwrap_or_else(|| panic!("Time went backwards")),
        };
        assert!(
            stale_negative.should_refresh(),
            "negative cache entries should refresh after the retry TTL"
        );

        let fresh_positive = CachedGpuAccel {
            accel: GpuAccel {
                gpu_type: GpuType::Apple,
                hevc_encoder: None,
                av1_encoder: None,
                h264_encoder: Some(GpuEncoder {
                    gpu_type: GpuType::Apple,
                    name: "h264_videotoolbox",
                    codec: "h264",
                    supports_crf: true,
                    crf_param: "q:v",
                    crf_range: (0, 100),
                    extra_args: vec![],
                }),
                enabled: true,
            },
            diagnostics: vec![],
            last_probe: std::time::Instant::now()
                .checked_sub(GPU_NEGATIVE_CACHE_TTL)
                .unwrap_or_else(|| panic!("Time went backwards")),
        };
        assert!(
            !fresh_positive.should_refresh(),
            "successful detections should not be re-probed by the negative-cache policy"
        );
    }

    #[test]
    fn test_summarize_ffmpeg_failure_line_prefers_specific_diagnostic() {
        let stderr = "\
Error while opening encoder - maybe incorrect parameters\n\
[hevc_videotoolbox @ 0x123] Cannot create compression session: -12908\n\
Conversion failed!";

        assert_eq!(
            summarize_ffmpeg_failure_line(stderr),
            "[hevc_videotoolbox @ 0x123] Cannot create compression session: -12908"
        );
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::PathBuf;

    proptest! {
        #[test]
        fn prop_gpu_temp_extension_matches_output(ext in "[a-z]{2,4}") {
            let output = PathBuf::from(format!("/path/to/output.{ext}"));
            let temp_ext = derive_gpu_temp_extension(&output);

            prop_assert!(temp_ext.ends_with(&ext),
                "Temp extension '{}' should end with '{}'", temp_ext, ext);

            prop_assert_eq!(temp_ext, format!("gpu_temp.{}", ext));
        }

        #[test]
        fn prop_gpu_temp_common_formats(
            format_idx in 0usize..5
        ) {
            let formats = ["mp4", "mkv", "webm", "mov", "avi"];
            let ext = *formats.get(format_idx).unwrap_or(&"");
            let output = PathBuf::from(format!("/video/output.{ext}"));
            let temp_ext = derive_gpu_temp_extension(&output);

            prop_assert_eq!(temp_ext, format!("gpu_temp.{}", ext),
                "Format {} should derive correctly", ext);
        }
    }
}
