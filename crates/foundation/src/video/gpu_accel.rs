//! GPU Acceleration Module - Unified hardware encoder detection and selection
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
//! use foundation::gpu_accel::{GpuAccel, GpuEncoder};
//!
//! let gpu = foundation::gpu_accel::GpuAccel::detect();
//! if let Some(encoder) = gpu.get_hevc_encoder() {
//!     // log_detail!("Using GPU encoder: {}", encoder.name);
//! }
//! ```

use crate::builder_base::ToolBuilder;
use crate::{FfmpegBuilder, FfprobeBuilder};
use chrono::{DateTime, FixedOffset, Utc};
use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use tracing::info;

use crate::explore_strategy::CrfCache;

/// Gets the current Beijing time (UTC+8) as a formatted string.
///
/// Used for logging and debugging with a consistent timezone reference.
///
/// # Returns
/// Formatted Beijing time string
fn beijing_time_now() -> String {
    let beijing = if let Some(v) = FixedOffset::east_opt(crate::constants::BEIJING_TIME_OFFSET_SECS)
    {
        Some(v)
    } else {
        crate::media_conversion_gate::delivery_gpu_batch_audit(
            "delivery_gpu",
            format!(
                "TIMEZONE ANOMALY: Beijing offset {}s rejected by chrono; falling back to UTC \
                 label",
                crate::constants::BEIJING_TIME_OFFSET_SECS
            ),
        );
        FixedOffset::east_opt(0)
    };
    let Some(beijing) = beijing else {
        return Utc::now().format("%Y-%m-%d %H:%M:%S (UTC)").to_string();
    };
    let now: DateTime<Utc> = Utc::now();
    now.with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S (UTC+8)")
        .to_string()
}

/// Describes a thread panic payload for logging.
///
/// Attempts to extract a string message from the panic payload.
/// Falls back to a generic description if the payload is not a string.
///
/// # Arguments
/// * `payload` - The panic payload from the thread
///
/// # Returns
/// String description of the panic
fn describe_thread_panic(payload: Box<dyn Any + Send + 'static>) -> String {
    match payload.downcast::<String>() {
        Ok(msg) => *msg,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(msg) => (*msg).to_string(),
            Err(_) => "non-string panic payload".to_string(),
        },
    }
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
                        let mut buf = crate::media_conversion_gate::mutex_guard_or_recover(
                            "delivery_gpu_stderr_lines",
                            lines.lock(),
                        );
                        if buf.len() >= max {
                            buf.pop_front();
                        }
                        buf.push_back(line);
                    }
                    Err(err) => {
                        crate::media_conversion_gate::delivery_gpu_batch_audit(
                            "delivery_gpu",
                            format!(
                                "GPU AUDIT: Failed to read GPU encoder stderr | Forensic: Error \
                                 '{err}'"
                            ),
                        );
                        break;
                    }
                }
            }
        })
    }

    fn get_lines(&self) -> Vec<String> {
        return crate::media_conversion_gate::mutex_guard_or_recover(
            "gpu_progress_lines",
            self.lines.lock(),
        )
        .iter()
        .cloned()
        .collect();
    }
}

// --- Sampling Positions ---
pub const GPU_SAMPLE_POS_START: f64 = crate::constants::GPU_SAMPLE_POS_START;
pub const GPU_SAMPLE_POS_QUARTER: f64 = crate::constants::GPU_SAMPLE_POS_QUARTER;
pub const GPU_SAMPLE_POS_HALF: f64 = crate::constants::GPU_SAMPLE_POS_HALF;
pub const GPU_SAMPLE_POS_THREE_QUARTERS: f64 = crate::constants::GPU_SAMPLE_POS_THREE_QUARTERS;
pub const GPU_SAMPLE_POS_TAIL: f64 = crate::constants::GPU_SAMPLE_POS_TAIL;

/// Number of segments to sample in multi-segment GPU probing.
pub const GPU_SAMPLE_SEGMENTS: usize = crate::constants::GPU_SAMPLE_SEGMENTS;

/// Collects video filter arguments from `FFmpeg` command line arguments.
///
/// Parses the argument array to find -vf options and extracts
/// the corresponding filter strings for GPU acceleration analysis.
///
/// # Arguments
/// * `vf_args` - Array of `FFmpeg` command line arguments
///
/// # Returns
/// Vector of video filter strings
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

/// Builds a multi-segment sampling filter for GPU acceleration testing.
///
/// Creates a filter that samples multiple segments throughout the video
/// to test GPU performance more comprehensively than single-point sampling.
///
/// # Arguments
/// * `duration` - Total video duration in seconds
/// * `ultimate_mode` - Whether to use ultimate mode settings
///
/// # Returns
/// Filter string for multi-segment sampling, or None if video is too short
#[must_use]
pub(crate) fn build_multi_segment_sampling_filter(
    duration: f64,
    ultimate_mode: bool,
) -> Option<String> {
    if duration < crate::constants::GPU_MIN_DURATION_FOR_SAMPLING {
        return None;
    }

    let seg_dur = if ultimate_mode {
        crate::constants::GPU_SEGMENT_DURATION_ULTIMATE
    } else {
        crate::constants::GPU_SEGMENT_DURATION
    };
    let positions = [
        GPU_SAMPLE_POS_START,
        duration * GPU_SAMPLE_POS_QUARTER,
        duration * GPU_SAMPLE_POS_HALF,
        duration * GPU_SAMPLE_POS_THREE_QUARTERS,
        (duration * GPU_SAMPLE_POS_TAIL).max(duration - f64::from(seg_dur)),
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

/// Builds video filter arguments for GPU acceleration sampling.
///
/// Combines multi-segment sampling filters with existing video filters
/// to create comprehensive `FFmpeg` arguments for GPU performance testing.
///
/// # Arguments
/// * `vf_args` - Existing video filter arguments
/// * `duration` - Total video duration in seconds
/// * `ultimate_mode` - Whether to use ultimate mode settings
///
/// # Returns
/// Vector of `FFmpeg` arguments with sampling filters
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

const GPU_NEGATIVE_CACHE_TTL: std::time::Duration =
    std::time::Duration::from_secs(crate::constants::GPU_NEGATIVE_CACHE_TTL_SECS);

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

/// Maximum concurrent GPU encode tasks (probe/encode); follows
/// `performance_schedule` when env unset.
fn gpu_concurrency_max() -> usize {
    crate::media_conversion_gate::gpu_concurrency_max_or_default()
}

static GPU_CONCURRENCY_CURRENT: Mutex<usize> = Mutex::new(0);
static GPU_CONCURRENCY_CVAR: Condvar = Condvar::new();

/// Acquires a GPU processing slot, blocking if necessary.
///
/// Waits until the number of active GPU processes is below the maximum
/// concurrency limit, then increments the counter.
fn acquire_gpu_slot() {
    let max = gpu_concurrency_max();
    let mut g = crate::media_conversion_gate::mutex_guard_or_recover(
        "gpu_concurrency_acquire",
        GPU_CONCURRENCY_CURRENT.lock(),
    );
    while *g >= max {
        g = crate::media_conversion_gate::mutex_guard_or_recover(
            "gpu_concurrency_cvar",
            GPU_CONCURRENCY_CVAR.wait(g),
        );
    }
    *g += 1;
}

/// Releases a GPU processing slot and notifies waiting threads.
///
/// Decrements the active GPU process counter and wakes up
/// one thread that may be waiting for a slot.
fn release_gpu_slot() {
    let mut g = crate::media_conversion_gate::mutex_guard_or_recover(
        "gpu_concurrency_release",
        GPU_CONCURRENCY_CURRENT.lock(),
    );
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
        .get_or_init(crate::media_conversion_gate::gpu_vaapi_device_path_or_default)
        .as_str()
}

/// Creates a temporary extension string for GPU processing files.
///
/// Uses the original file extension with a custom suffix to create
/// temporary filenames for GPU acceleration testing.
///
/// # Arguments
/// * `output` - The output file path
/// * `suffix` - The suffix to use (e.g., `"gpu_temp"`, `"warmup"`)
///
/// # Returns
/// Temporary extension string
fn temp_extension_for(output: &std::path::Path, suffix: &str) -> String {
    let ext = crate::media_conversion_gate::gpu_output_extension_segment(output);
    format!("{suffix}.{ext}")
}

/// Returns a temp extension string (e.g. "`gpu_temp.mp4`") for the given output
/// path. Used by callers and by warmup encoding internally via
/// `temp_extension_for`(_, "warmup").
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

/// Represents a specific GPU hardware encoder with its configuration
/// parameters.
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
    #[must_use = "Return value should be used"]
    pub const fn ffmpeg_name(&self) -> &'static str {
        self.name
    }

    /// Converts a CRF value to encoder-specific arguments for this GPU encoder.
    ///
    /// For CRF-supporting encoders, returns the CRF parameter with clamping.
    /// For non-CRF encoders, falls back to bitrate-based arguments.
    #[must_use = "Result must be checked"]
    /// # Errors
    /// Returns an error if constructing encoder arguments fails (invalid CRF or
    /// unsupported codec conversions).
    pub fn get_crf_args(&self, crf: f32) -> anyhow::Result<Vec<String>> {
        if self.supports_crf {
            let quality_value = if self.gpu_type == GpuType::Apple {
                crf.mul_add(-2.0, 100.0).clamp(1.0, 100.0)
            } else {
                crf.clamp(f32::from(self.crf_range.0), f32::from(self.crf_range.1))
            };

            Ok(vec![
                format!("-{}", self.crf_param),
                format!("{:.0}", quality_value),
            ])
        } else {
            let bitrate = crf_to_estimated_bitrate(crf, self.codec)?;
            Ok(vec!["-b:v".to_string(), format!("{}k", bitrate)])
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
    /// Successful probes stay cached. Failed probes are soft-cached and
    /// automatically retried after a short TTL so transient startup or
    /// device-busy failures do not latch CPU mode for the lifetime of the
    /// process.
    #[must_use]
    pub fn detect() -> Self {
        let cached = Self::cached_state();
        if cached.should_refresh() {
            return Self::detect_fresh();
        }
        cached.accel
    }

    /// Detects available GPU acceleration and forces an immediate re-probe if
    /// the cached state is currently unavailable.
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
                // Log to file only (stderr layer filters out target "gpu_detection" for less
                // terminal noise).
                info!(target: "gpu_detection", "  GPU: {gpu_type}", gpu_type = self.gpu_type);
            } else {
                // Surface why detection failed so the user has context without needing
                // --verbose.
                let reason =
                    crate::media_conversion_gate::delivery_gpu_probe_failure_reason_or_default(
                        &diagnostics,
                    );
                let pattern = format!("{re}ason", re = "{");
                let msg =
                    crate::infra::static_logs::messages::GPU_PROBE_FAILED.replace(&pattern, reason);
                crate::media_conversion_gate::delivery_gpu_batch_audit("delivery_gpu", &msg);
            }
            return;
        }
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_GPU,
            crate::infra::static_logs::messages::GPU_PROBE_START
        );
        if self.enabled {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU,
                format!(
                    "GPU hardware acceleration detected and verified ({gpu_type})",
                    gpu_type = self.gpu_type,
                )
            );
            if let Some(enc) = &self.hevc_encoder {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_GPU_REPORT_HEVC
                        .replace("{}", enc.name)
                );
            }
            if let Some(enc) = &self.av1_encoder {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_GPU_REPORT_AV1
                        .replace("{}", enc.name)
                );
            }
            if let Some(enc) = &self.h264_encoder {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_GPU_REPORT_H264
                        .replace("{}", enc.name)
                );
            }
            for diagnostic in diagnostics.iter().take(3) {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_GPU_PROBE_NOTE
                        .replace("{}", diagnostic)
                );
            }
        } else {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_GPU,
                crate::infra::static_logs::messages::GPU_NOT_AVAILABLE
            );
            for diagnostic in diagnostics.iter().take(3) {
                crate::log_detail!(&format!("      • {diagnostic}"));
            }
        }
    }

    fn cached_state() -> CachedGpuAccel {
        let cache = GPU_ACCEL.get_or_init(|| Mutex::new(CachedGpuAccel::probe_now()));
        return crate::media_conversion_gate::mutex_guard_or_recover(
            "gpu_accel_cache",
            cache.lock(),
        )
        .clone();
    }

    fn store_cached_state(state: CachedGpuAccel) {
        let cache = GPU_ACCEL.get_or_init(|| Mutex::new(state.clone()));
        *crate::media_conversion_gate::mutex_guard_or_recover("gpu_accel_cache", cache.lock()) =
            state;
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
    fn try_vaapi(encoders: &[String], diagnostics: &mut Vec<String>) -> Option<Self> {
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

/// Gets the list of available video encoders from `FFmpeg`.
///
/// Executes `ffmpeg -encoders` and filters for video encoders (lines starting
/// with " V").
///
/// # Returns
/// Vector of available video encoder names, or error string if command fails
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

/// Summarizes `FFmpeg` failure output to a single line.
///
/// Extracts the most relevant error information from `FFmpeg` output
/// to provide concise error messages in GPU acceleration results.
///
/// # Arguments
/// * `text` - The `FFmpeg` error output text
///
/// # Returns
/// Summarized error message
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

    match lines.last() {
        Some(line) => (*line).to_string(),
        None => "unknown ffmpeg error".to_string(),
    }
}

/// Summarizes `FFmpeg` failure output from stdout and stderr.
///
/// Prioritizes stderr output for error messages, falls back to stdout
/// if stderr doesn't contain recognizable error patterns.
///
/// # Arguments
/// * `stdout` - The stdout bytes from `FFmpeg`
/// * `stderr` - The stderr bytes from `FFmpeg`
///
/// # Returns
/// Summarized error message
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

/// Tests a GPU encoder with a sample video segment.
///
/// Runs `FFmpeg` with the specified encoder on a short video segment
/// to verify that the encoder works correctly on the current system.
///
/// # Arguments
/// * `encoder` - The GPU encoder to test
///
/// # Returns
/// Ok(()) if successful, or error string if encoding fails
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
    #[cfg(target_os = "macos")]
    let attempts = if encoder.gpu_type == GpuType::Apple {
        vec![false, true]
    } else {
        vec![false]
    };

    #[cfg(not(target_os = "macos"))]
    let attempts = vec![false];

    let mut last_err = String::new();
    for allow_sw in &attempts {
        let mut builder = FfmpegBuilder::new();
        builder
            .hide_banner()
            .input_format("lavfi")
            .input("nullsrc=s=128x128:d=0.1")
            .codec_video(encoder.name);

        for arg in encoder.get_crf_args(mid_crf).map_err(|e| e.to_string())? {
            builder.arg(arg);
        }
        for arg in encoder.extra_args() {
            builder.arg(arg);
        }
        if *allow_sw {
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

/// Estimates bitrate from CRF value for a given codec.
///
/// Uses codec-specific formulas to estimate the bitrate that would
/// correspond to a given CRF value for rough performance calculations.
///
/// # Arguments
/// * `crf` - The CRF value
/// * `codec` - The codec name (e.g., "h264", "hevc", "av1")
///
/// # Returns
/// Estimated bitrate in kbps
fn crf_to_estimated_bitrate(crf: f32, codec: &str) -> anyhow::Result<u32> {
    let base_bitrate = match codec {
        "av1" => 4_000_i32,
        "h264" => 8_000_i32,
        _ => 5_000_i32,
    };

    let crf_factor = match codec {
        "hevc" | "h264" => 0.9_f32.powf((crf - 23.0) / 6.0),
        "av1" => 0.9_f32.powf((crf - 30.0) / 6.0),
        _ => 1.0,
    };

    crate::numeric_cast::f32_to_u32_strict(
        crate::numeric_cast::f64_to_f32_lossy(f64::from(base_bitrate)) * crf_factor,
        "estimated_bitrate",
    )
    .ok_or_else(|| anyhow::anyhow!("Failed to estimate bitrate for CRF {crf} and codec {codec}"))
}

/// Result of a smart sampling strategy for selecting representative video
/// segments.
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

    let scene_threshold = 0.3_f64;
    let entropy_threshold = 6.0_f64;

    let select_expr = if sample_ratio > 0.5 {
        format!(
            "gt(scene,{})+gt(entropy,{})",
            scene_threshold * 0.5_f64,
            entropy_threshold * 0.8_f64
        )
    } else if sample_ratio > 0.2 {
        format!("gt(scene,{scene_threshold})+gt(entropy,{entropy_threshold})")
    } else {
        format!(
            "gt(scene,{})*gt(entropy,{})",
            scene_threshold * 1.5_f64,
            entropy_threshold * 1.2_f64
        )
    };

    let test_output = FfmpegBuilder::new()
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

/// A quality score combining SSIM, compression ratio, and a weighted combined
/// score.
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
    /// Sanitize GPU search quality metrics before comparison gates.
    pub fn seal_algorithm_outputs(&mut self) {
        if let Some(v) = crate::algorithm_seal::exploration_unit_probability(self.ssim) {
            self.ssim = v;
        }
        if let Some(v) = crate::algorithm_seal::seal_non_negative_finite(self.compression_ratio) {
            self.compression_ratio = v;
        }
        if let Some(v) = crate::algorithm_seal::seal_finite_scalar(self.combined_score) {
            self.combined_score = v;
        }
    }

    /// Returns the SSIM score as a typed `Ssim` value, if valid.
    #[inline]
    #[must_use]
    pub fn ssim_typed(&self) -> Option<crate::types::Ssim> {
        match crate::types::Ssim::new(self.ssim) {
            Ok(value) => Some(value),
            Err(err) => {
                crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                    "gpu_ssim_typed_invalid",
                    format!("invalid GPU SSIM value {}: {err}", self.ssim),
                );
                None
            }
        }
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
    let compression_ratio = crate::media_conversion_gate::gpu_quality_compression_ratio_or_neutral(
        output_size,
        input_size,
        "gpu_accel calculate_quality_score",
    );

    let (ssim_weight, size_weight): (f64, f64) = match phase {
        SearchPhase::Gpu => (0.4, 0.6),
        SearchPhase::Cpu => (0.7, 0.3),
    };

    let size_score = (1.0 - compression_ratio).max(0.0);
    let combined_score = ssim_weight.mul_add(ssim, size_weight * size_score);

    let mut score = QualityScore {
        ssim,
        compression_ratio,
        combined_score,
    };
    score.seal_algorithm_outputs();
    score
}

/// Returns whether the new quality score is meaningfully better than the old
/// one.
#[must_use]
pub fn is_quality_better(
    new_score: &QualityScore,
    old_score: &QualityScore,
    min_ssim_threshold: f64,
) -> bool {
    if new_score.ssim < min_ssim_threshold {
        return false;
    }
    if old_score.combined_score <= 0.0_f64 {
        return new_score.combined_score > 0.0;
    }
    let improvement =
        (new_score.combined_score - old_score.combined_score) / old_score.combined_score;
    improvement > 0.005
}

/// Estimates the optimal CPU search center point dynamically.
///
/// Calculates the starting CRF value for CPU encoding based on GPU results,
/// compression potential, and GPU type-specific offsets.
///
/// # Arguments
/// * `gpu_boundary` - The GPU's optimal CRF boundary
/// * `gpu_type` - The type of GPU encoder used
/// * `compression_potential` - Optional compression ratio potential
///
/// # Returns
/// Estimated optimal CRF for CPU encoding
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

    let mut center = gpu_boundary + base_offset;
    if let Some(adjustment) =
        crate::media_conversion_gate::gpu_compression_potential_adjustment_optional(
            compression_potential,
            "gpu_accel estimate_cpu_search_center",
        )
    {
        center += adjustment;
    }
    center
}

/// Estimates the center of the CPU search range based on a GPU boundary CRF and
/// GPU type.
///
/// `codec` is reserved for future codec-specific GPU→CPU CRF mapping; it is
/// accepted for API stability and intentionally ignored until tuning data
/// exists.
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

/// Estimates a CPU search range from a GPU range, adjusting for GPU type and
/// codec.
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

/// Converts a GPU CRF to an estimated CPU CRF (deprecated, use
/// `estimate_cpu_search_center`).
#[deprecated(since = "5.0.1", note = "use estimate_cpu_search_center instead")]
#[must_use]
pub fn gpu_to_cpu_crf(gpu_crf: f32, gpu_type: GpuType, codec: &str) -> f32 {
    estimate_cpu_search_center(gpu_crf, gpu_type, codec)
}

/// Result of a GPU-based coarse search for optimal CRF.
#[derive(Debug, Clone)]
pub struct GpuCoarseResult {
    /// The CRF value at the compression boundary found by the GPU search.
    pub gpu_boundary_crf: Option<f32>,
    /// The output file size (bytes) at the best CRF found, if any compression
    /// point was found.
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
    /// The PSNR score at the quality ceiling, if detected.
    pub quality_ceiling_psnr: Option<f64>,
}

impl GpuCoarseResult {
    /// Returns the best SSIM score as a typed `Ssim` value, if available.
    #[inline]
    #[must_use]
    pub fn best_ssim_typed(&self) -> Option<crate::types::Ssim> {
        self.gpu_best_ssim
            .and_then(|v| match crate::types::Ssim::new(v) {
                Ok(value) => Some(value),
                Err(err) => {
                    crate::media_conversion_gate::explore_gpu_coarse_explore_audit(
                        "gpu_best_ssim_typed_invalid",
                        format!("invalid best GPU SSIM value {v}: {err}"),
                    );
                    None
                }
            })
    }

    /// Returns the quality ceiling PSNR value, if available.
    #[inline]
    #[must_use]
    pub const fn ceiling_psnr(&self) -> Option<f64> {
        self.quality_ceiling_psnr
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
        crate::log_detail!(
            "   {} GPU/CPU CRF Mapping ({} - {}):",
            crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]"),
            self.gpu_type,
            self.codec.to_uppercase()
        );
        if self.gpu_type == GpuType::Apple {
            crate::log_detail!(crate::infra::static_logs::messages::MSG_GPU_VT_INFO);
            crate::log_detail!(
                "      • SSIM ceiling: 0.91~{} (content-dependent, cannot reach {}+)",
                crate::constants::GPU_SEARCH_CEILING_SSIM_THRESHOLD,
                crate::constants::SSIM_GRADE_EXCELLENT
            );
            crate::log_detail!(
                "      • Best value: q:v 75-80 (SSIM ~{}, good compression)",
                crate::constants::GPU_SEARCH_CEILING_SSIM_THRESHOLD,
            );
        } else {
            crate::log_detail!(crate::infra::static_logs::messages::MSG_GPU_ACCURATE_BOUNDARY);
        }
        crate::log_detail!(
            "      • CPU offset: +{:.1} (CPU needs higher CRF for same compression)",
            self.offset
        );
        crate::log_detail!(
            "      • {} CPU fine-tunes for SSIM {}+ (GPU max ~{})",
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]"),
            crate::constants::SSIM_GRADE_EXCELLENT,
            crate::constants::GPU_SEARCH_CEILING_SSIM_THRESHOLD
        );
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
            step: crate::constants::GPU_COARSE_STEP,
            max_iterations: 10,
            ultimate_mode: false,
            preset: crate::types::EncoderPreset::Medium,
        }
    }
}

/// Calculates PSNR (Peak Signal-to-Noise Ratio) between two videos.
///
/// Uses `FFmpeg` to compute PSNR as a quality metric for comparing
/// encoded output with the original input video.
///
/// # Arguments
/// * `input` - Path to the original input video
/// * `output` - Path to the encoded output video
///
/// # Returns
/// PSNR value, or error string if calculation fails
fn calculate_psnr_fast(input: &str, output: &str) -> Result<f64, String> {
    let psnr_output = FfmpegBuilder::new()
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
        if line.contains("psnr_avg:")
            && let Some(pos) = line.find("psnr_avg:")
        {
            let after = &line[pos + 9..];
            let token = match after.find(char::is_whitespace) {
                Some(space_pos) => after[..space_pos].trim(),
                None => after.trim(),
            };
            match crate::video_explorer::precision::parse_explore_psnr_metric_token(token) {
                Ok(Some(psnr)) => return Ok(psnr),
                Ok(None) => {}
                Err(err) => return Err(format!("Malformed psnr_avg metric token: {err}")),
            }
        }

        // Strategy 2: Look for "average:" in stats output
        if line.contains("average:")
            && let Some(pos) = line.find("average:")
        {
            let after = &line[pos + 8..];
            let parts: Vec<&str> = after.split_whitespace().collect();
            if let Some(first) = parts.first() {
                match crate::video_explorer::precision::parse_explore_psnr_metric_token(
                    first.trim(),
                ) {
                    Ok(Some(psnr)) => return Ok(psnr),
                    Ok(None) => {}
                    Err(err) => return Err(format!("Malformed average PSNR metric token: {err}")),
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
            let Some(last) = self.samples.last().map(|s| s.1) else {
                return false;
            };
            let Some(prev) = self
                .samples
                .get(self.samples.len().saturating_sub(2))
                .map(|s| s.1)
            else {
                return false;
            };
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
                .max_by(|a, b| crate::media_conversion_gate::f64_sort_cmp(a.1, b.1))
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
        points.sort_by(|a, b| crate::media_conversion_gate::f64_sort_cmp(a.0, b.0));

        for i in 0..points.len() - 1 {
            let (psnr1, ssim1) = points[i];
            let (psnr2, ssim2) = points[i + 1];

            if psnr >= psnr1 && psnr <= psnr2 {
                let denom = psnr2 - psnr1;
                if crate::numeric_cast::is_effectively_zero(
                    denom,
                    crate::numeric_cast::FloatContext::Accumulation,
                ) {
                    return Some(f64::midpoint(ssim1, ssim2));
                }
                let ratio = (psnr - psnr1) / denom;
                let predicted_ssim = ratio.mul_add(ssim2 - ssim1, ssim1);
                return Some(predicted_ssim);
            }
        }

        if psnr < points[0].0 {
            let (psnr1, ssim1) = points[0];
            let (psnr2, ssim2) = points[1];
            let denom = psnr2 - psnr1;
            if crate::numeric_cast::is_effectively_zero(
                denom,
                crate::numeric_cast::FloatContext::Accumulation,
            ) {
                return Some(ssim1);
            }
            let slope = (ssim2 - ssim1) / denom;
            Some(slope.mul_add(psnr - psnr1, ssim1))
        } else {
            let n = points.len();
            let (psnr1, ssim1) = points[n - 2];
            let (psnr2, ssim2) = points[n - 1];
            let denom = psnr2 - psnr1;
            if crate::numeric_cast::is_effectively_zero(
                denom,
                crate::numeric_cast::FloatContext::Accumulation,
            ) {
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
            crate::log_detail!(crate::infra::static_logs::messages::MSG_GPU_MAPPING_UNCALIBRATED);
            return;
        }

        crate::log_detail!(crate::infra::static_logs::messages::MSG_GPU_MAPPING_REPORT);
        crate::log_detail!(
            "      Calibration points: {}",
            self.calibration_points.len()
        );
        crate::log_detail!(
            "      Mapping quality: {:.1}%",
            self.get_mapping_quality() * 100.0_f64
        );

        if self.calibration_points.len() >= 2 {
            let test_psnrs = vec![35.0_f64, 38.0_f64, 40.0_f64, 42.0_f64, 45.0_f64];
            crate::log_detail!(crate::infra::static_logs::messages::MSG_GPU_MAPPING_EXAMPLE);
            for psnr in test_psnrs {
                if let Some(ssim) = self.predict_ssim_from_psnr(psnr) {
                    crate::log_detail!(&format!("GPU Mapping: PSNR {psnr:.1} -> SSIM {ssim:.4}"));
                }
            }
        }
    }
}

/// Perform a GPU-based coarse search for optimal CRF.
///
/// # Errors
/// Returns an `anyhow::Result` if search fails.
pub fn gpu_coarse_search(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    vf_args: &[String],
    progress_cb: Option<&dyn Fn(f32, u64)>,
) -> anyhow::Result<GpuCoarseResult> {
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
pub fn gpu_coarse_search_with_log(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    vf_args: &[String],
    progress_cb: Option<&dyn Fn(f32, u64)>,
    log_cb: Option<&dyn Fn(&str)>,
) -> anyhow::Result<GpuCoarseResult> {
    let result = gpu_coarse_search_with_log_impl(
        input,
        output,
        encoder,
        input_size,
        config,
        vf_args,
        progress_cb,
        log_cb,
    )?;
    // Ensure temp output is always deleted, regardless of success/failure
    crate::media_conversion_gate::delivery_remove_file_or_audit("gpu_coarse_temp_output", output);
    Ok(result)
}

#[derive(Debug, Clone, Copy)]
enum GpuSamplingMode {
    Conservative,
    Sequential,
    Parallel,
}

#[derive(Debug, Clone, Copy)]
struct GpuSamplingPlan {
    sample_duration_limit: f32,
    skip_parallel: bool,
    mode: GpuSamplingMode,
}

impl GpuSamplingPlan {
    fn describe(self) -> anyhow::Result<String> {
        let label = match self.mode {
            GpuSamplingMode::Conservative => format!(
                "   {} Very large file detected → Conservative mode",
                crate::modern_ui::symbols::styled_warning_icon()
            ),
            GpuSamplingMode::Sequential => format!(
                "   {} Large file detected → Sequential mode",
                crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
            ),
            GpuSamplingMode::Parallel => format!(
                "   {} Normal file → Parallel mode",
                crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
            ),
        };
        let display_limit = crate::numeric_cast::f32_to_u32_strict(
            self.sample_duration_limit,
            "gpu_sampling_plan_limit",
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid GPU sampling plan limit: {}",
                self.sample_duration_limit
            )
        })?;
        Ok(format!("{label} ({display_limit}s sample)"))
    }
}

#[derive(Debug)]
struct InitialProbeOutcome {
    best_crf: Option<f32>,
    best_size: Option<u64>,
    boundary_low: f32,
    boundary_high: f32,
    prev_size: Option<u64>,
    found_compress_point: bool,
    messages: Vec<String>,
}

#[derive(Debug, Default)]
struct FinalGpuValidation {
    gpu_ssim: Option<f64>,
    gpu_psnr: Option<f64>,
    messages: Vec<String>,
}

#[derive(Debug, Clone)]
struct FinalGpuOutcome {
    gpu_boundary_crf: Option<f32>,
    gpu_best_size: Option<u64>,
    gpu_best_ssim: Option<f64>,
    gpu_best_psnr: Option<f64>,
    gpu_type: GpuType,
    iterations: u32,
    found_boundary: bool,
    fine_tuned: bool,
    sample_input_size: u64,
    quality_ceiling_crf: Option<f32>,
    quality_ceiling_psnr: Option<f64>,
    last_tested_crf: f32,
}

impl FinalGpuOutcome {
    fn from_search(
        state: &GpuSearchState,
        ceiling_detector: &QualityCeilingDetector,
        final_validation: &FinalGpuValidation,
        iterations: u32,
        sample_input_size: u64,
        gpu_type: GpuType,
        config: &GpuCoarseConfig,
    ) -> Self {
        let (last_tested_crf, found_boundary, fine_tuned) =
            crate::media_conversion_gate::explore_gpu_search_summary_from_best_crf(
                state.best_crf,
                config.max_crf,
                iterations,
            );
        let quality_ceiling_info = ceiling_detector
            .ceiling_detected
            .then(|| ceiling_detector.get_ceiling())
            .flatten();
        let quality_ceiling_crf = quality_ceiling_info.map(|(crf, _psnr)| crf);
        let quality_ceiling_psnr = quality_ceiling_info.map(|(_crf, psnr)| psnr);

        assert!(
            !(found_boundary && state.best_size.is_none()),
            "GPU coarse-search reached a boundary without recording best_size"
        );
        assert!(
            quality_ceiling_crf.is_some() == quality_ceiling_psnr.is_some(),
            "GPU quality ceiling CRF/PSNR became desynchronized"
        );

        Self {
            gpu_boundary_crf:
                crate::media_conversion_gate::explore_gpu_boundary_crf_from_search_optional(
                    found_boundary,
                    state.best_crf,
                    quality_ceiling_crf,
                    "gpu_accel coarse_search outcome",
                ),
            gpu_best_size: state.best_size,
            gpu_best_ssim: final_validation.gpu_ssim,
            gpu_best_psnr: final_validation.gpu_psnr,
            gpu_type,
            iterations,
            found_boundary,
            fine_tuned,
            sample_input_size,
            quality_ceiling_crf,
            quality_ceiling_psnr,
            last_tested_crf,
        }
    }

    fn append_summary_messages(
        &self,
        messages: &mut Vec<String>,
        encoder: &str,
        config: &GpuCoarseConfig,
        psnr_ssim_mapper: &PsnrSsimMapper,
    ) {
        if let Some(ceiling_crf) = self.quality_ceiling_crf {
            messages.push(format!(
                "   {} GPU Quality Ceiling Detected!",
                crate::media_conversion_gate::ui_icon_pick("🎯", "[TARGET]")
            ));
            messages.push(format!(
                "      └─ Ceiling CRF: {ceiling_crf:.1} (PSNR plateau)"
            ));
            messages.push(format!(
                "      └─ Last tested CRF: {:.1}",
                self.last_tested_crf
            ));
            if !crate::float_compare::approx_eq_crf(ceiling_crf, self.last_tested_crf) {
                messages.push(
                    "      └─ Boundary = Ceiling (lower CRFs are bloated, no quality gain)"
                        .to_string(),
                );
            }
        }

        messages.push("   ═══════════════════════════════════════════════════".to_string());
        if self.found_boundary {
            if let Some(boundary_crf) = self.gpu_boundary_crf {
                messages.push(format!(
                    "   {} GPU Boundary CRF: {:.1} (highest quality that compresses)",
                    crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]"),
                    boundary_crf
                ));
            } else {
                messages.push(format!(
                    "   {} GPU boundary CRF absent after search (refusing forged substitute)",
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
            }
            if let Some(size) = self.gpu_best_size {
                let ratio = crate::numeric_cast::u64_to_f64(size)
                    / crate::numeric_cast::u64_to_f64(self.sample_input_size.max(1))
                    * 100.0_f64;
                messages.push(format!(
                    "   {} GPU Best Size: {ratio:.1}% of input",
                    crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
                ));
            }
            if let Some(ssim) = self.gpu_best_ssim {
                let quality_hint = if ssim >= 0.97_f64 {
                    format!(
                        "{} Near ceiling",
                        crate::media_conversion_gate::ui_icon_pick("🟢", "[OK]")
                    )
                } else if ssim >= 0.95_f64 {
                    format!(
                        "{} Good",
                        crate::media_conversion_gate::ui_icon_pick("🟡", "[OK]")
                    )
                } else {
                    format!(
                        "{} Below expected",
                        crate::media_conversion_gate::ui_icon_pick("🟠", "[WARN]")
                    )
                };
                messages.push(format!(
                    "   {} GPU Best SSIM: {ssim:.6} {quality_hint}",
                    crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
                ));
            }
            if let Some(psnr) = self.gpu_best_psnr {
                messages.push(format!(
                    "   {} GPU Best PSNR: {psnr:.2}dB",
                    crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
                ));
            }
            if psnr_ssim_mapper.calibrated {
                messages.push("   ═══════════════════════════════════════════════════".to_string());
            }

            if let Some(boundary_crf) = self.gpu_boundary_crf {
                let mapping = match encoder {
                    "av1" => CrfMapping::av1(self.gpu_type),
                    _ => CrfMapping::hevc(self.gpu_type),
                };
                let (cpu_center, cpu_low, cpu_high) =
                    mapping.gpu_to_cpu_range(boundary_crf, config.min_crf, config.max_crf);
                messages.push(format!(
                    "   {} CPU Search Range: [{cpu_low:.1}, {cpu_high:.1}] (center: \
                     {cpu_center:.1})",
                    crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
                ));
            }
        } else {
            messages.push(format!(
                "   {} No compression boundary found (file may be already compressed)",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
        }

        messages.push(format!(
            "   {} GPU Iterations: {} (fine-tuned: {})",
            crate::media_conversion_gate::ui_icon_pick("📈", "[CHART]"),
            self.iterations,
            if self.fine_tuned { "yes" } else { "no" }
        ));
    }

    fn into_result(self, encoder: &str) -> GpuCoarseResult {
        GpuCoarseResult {
            gpu_boundary_crf: self.gpu_boundary_crf,
            gpu_best_size: self.gpu_best_size,
            gpu_best_ssim: self.gpu_best_ssim,
            gpu_type: self.gpu_type,
            codec: encoder.to_string(),
            iterations: self.iterations,
            found_boundary: self.found_boundary,
            fine_tuned: self.fine_tuned,
            log: Vec::new(),
            sample_input_size: self.sample_input_size,
            quality_ceiling_crf: self.quality_ceiling_crf,
            quality_ceiling_psnr: self.quality_ceiling_psnr,
        }
    }
}

#[derive(Debug, Clone)]
struct GpuSearchState {
    best_crf: Option<f32>,
    best_size: Option<u64>,
    boundary_low: f32,
    boundary_high: f32,
    prev_size: Option<u64>,
    found_compress_point: bool,
}

#[derive(Debug, Clone)]
struct GpuSearchSetup {
    gpu_type: GpuType,
    gpu_encoder: GpuEncoder,
    duration: f32,
    sampling_plan: GpuSamplingPlan,
    actual_sample_duration: f32,
    sample_input_size: u64,
    warmup_duration: f32,
    max_iterations_limit: u32,
}

#[derive(Debug)]
enum GpuSearchPreparation {
    Ready(GpuSearchSetup),
    EarlyResult(GpuCoarseResult),
}

fn calc_gpu_change_rate(prev: u64, curr: u64) -> f64 {
    if prev == 0 {
        return f64::MAX;
    }
    ((crate::numeric_cast::u64_to_f64(curr) - crate::numeric_cast::u64_to_f64(prev))
        / crate::numeric_cast::u64_to_f64(prev.max(1)))
    .abs()
}

fn base_gpu_coarse_result(
    gpu_type: GpuType,
    encoder: &str,
    iterations: u32,
    log: Vec<String>,
    sample_input_size: u64,
) -> GpuCoarseResult {
    GpuCoarseResult {
        gpu_boundary_crf: None,
        gpu_best_size: None,
        gpu_best_ssim: None,
        gpu_type,
        codec: encoder.to_string(),
        iterations,
        found_boundary: false,
        fine_tuned: false,
        log,
        sample_input_size,
        quality_ceiling_crf: None,
        quality_ceiling_psnr: None,
    }
}

fn gpu_unavailable_messages() -> Vec<String> {
    vec![
        "   ╔═══════════════════════════════════════════════════════════╗".to_string(),
        format!(
            "   ║  {}  FALLBACK: No GPU available!                          ║",
            crate::modern_ui::symbols::styled_warning_icon()
        ),
        "   ║  Skipping GPU coarse search, using CPU-only mode          ║".to_string(),
        "   ║  This may take longer but results will be accurate        ║".to_string(),
        "   ╚═══════════════════════════════════════════════════════════╝".to_string(),
    ]
}

fn gpu_encoder_missing_messages(encoder: &str) -> Vec<String> {
    vec![
        "   ╔═══════════════════════════════════════════════════════════╗".to_string(),
        format!(
            "   ║  {}  FALLBACK: No GPU encoder for {}!              ║",
            crate::modern_ui::symbols::styled_warning_icon(),
            encoder.to_uppercase()
        ),
        "   ║  Skipping GPU coarse search, using CPU-only mode          ║".to_string(),
        "   ║  This may take longer but results will be accurate        ║".to_string(),
        "   ╚═══════════════════════════════════════════════════════════╝".to_string(),
    ]
}

fn sampling_mode_messages(duration: f32, ultimate_mode: bool) -> Vec<String> {
    let seg_dur = if ultimate_mode {
        crate::constants::GPU_SEGMENT_DURATION_ULTIMATE
    } else {
        crate::constants::GPU_SEGMENT_DURATION
    };
    if duration >= 60.0 {
        vec![format!(
            "   {} Multi-segment sampling: 5 segments × {:.0}s = {:.0}s (0%, 25%, 50%, 75%, 90%)",
            crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]"),
            seg_dur,
            seg_dur * 5.0
        )]
    } else {
        vec![format!(
            "   {} Full video sampling: {duration:.1}s",
            crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
        )]
    }
}

fn append_gpu_log_message(
    log: &mut Vec<String>,
    silent_mode: bool,
    log_cb: Option<&dyn Fn(&str)>,
    msg: String,
) {
    if !silent_mode {
        if let Some(cb) = log_cb {
            cb(&msg);
        } else {
            crate::log_detail!("{}", msg);
        }
    }
    log.push(msg);
}

fn append_gpu_log_messages(
    log: &mut Vec<String>,
    silent_mode: bool,
    log_cb: Option<&dyn Fn(&str)>,
    messages: &[String],
) {
    for message in messages {
        append_gpu_log_message(log, silent_mode, log_cb, message.clone());
    }
}

fn validate_gpu_coarse_config(config: &GpuCoarseConfig) -> anyhow::Result<()> {
    use anyhow::bail;

    if !config.initial_crf.is_finite()
        || !config.min_crf.is_finite()
        || !config.max_crf.is_finite()
        || !config.step.is_finite()
    {
        bail!("GPU coarse config contains non-finite CRF/step values: {config:?}");
    }
    if config.min_crf > config.max_crf {
        bail!(
            "GPU coarse config is invalid: min_crf {:.2} > max_crf {:.2}",
            config.min_crf,
            config.max_crf
        );
    }
    if config.step <= 0.0 {
        bail!(
            "GPU coarse config is invalid: step must be > 0, got {:.3}",
            config.step
        );
    }
    if config.max_iterations == 0 {
        bail!("GPU coarse config is invalid: max_iterations must be > 0");
    }
    Ok(())
}

fn prepare_gpu_search(
    input: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
) -> anyhow::Result<(GpuSearchPreparation, Vec<String>)> {
    use anyhow::bail;

    let gpu = GpuAccel::detect_with_retry();
    if !gpu.is_available() {
        let messages = gpu_unavailable_messages();
        let result = base_gpu_coarse_result(GpuType::None, encoder, 0, Vec::new(), input_size);
        return Ok((GpuSearchPreparation::EarlyResult(result), messages));
    }

    let gpu_encoder = if let Some(gpu_encoder) = resolve_gpu_encoder(&gpu, encoder)? {
        gpu_encoder.clone()
    } else {
        let messages = gpu_encoder_missing_messages(encoder);
        let result = base_gpu_coarse_result(gpu.gpu_type, encoder, 0, Vec::new(), input_size);
        return Ok((GpuSearchPreparation::EarlyResult(result), messages));
    };

    let duration = probe_gpu_duration(input)?;
    if let Some(reason) = gpu_skip_reason(input_size, duration, config.ultimate_mode) {
        let result = base_gpu_coarse_result(gpu.gpu_type, encoder, 0, Vec::new(), input_size);
        return Ok((
            GpuSearchPreparation::EarlyResult(result),
            vec![format!(
                "   {} Skip GPU: {reason} → CPU-only mode",
                crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
            )],
        ));
    }

    let sampling_plan = build_gpu_sampling_plan(input_size, duration, config.ultimate_mode)?;
    let actual_sample_duration = duration.min(sampling_plan.sample_duration_limit);
    if !actual_sample_duration.is_finite() || actual_sample_duration <= 0.0 {
        bail!(
            "GPU actual sample duration became invalid: duration={duration}, limit={}",
            sampling_plan.sample_duration_limit
        );
    }

    let sample_input_size =
        calculate_gpu_sample_input_size(input_size, duration, config.ultimate_mode)?;
    let warmup_duration = duration.min(crate::constants::WARMUP_DURATION_SECS);
    if !warmup_duration.is_finite() || warmup_duration <= 0.0 {
        bail!("GPU warmup duration became invalid: {warmup_duration}");
    }

    let header_messages = vec![
        sampling_plan.describe()?,
        format!(
            "GPU Search ({}, {:.2}MB, {:.1}s)",
            gpu.gpu_type,
            crate::numeric_cast::u64_to_f64(input_size) / 1_024.0_f64 / 1_024.0_f64,
            duration
        ),
    ];

    Ok((
        GpuSearchPreparation::Ready(GpuSearchSetup {
            gpu_type: gpu.gpu_type,
            gpu_encoder,
            duration,
            sampling_plan,
            actual_sample_duration,
            sample_input_size,
            warmup_duration,
            max_iterations_limit: crate::constants::GPU_ABSOLUTE_MAX_ITERATIONS,
        }),
        header_messages,
    ))
}

fn resolve_gpu_encoder<'a>(
    gpu: &'a GpuAccel,
    encoder: &str,
) -> anyhow::Result<Option<&'a GpuEncoder>> {
    use anyhow::bail;

    let selected = match encoder {
        "hevc" => gpu.get_hevc_encoder(),
        "av1" => gpu.get_av1_encoder(),
        "h264" => gpu.get_h264_encoder(),
        _ => bail!("Unsupported GPU coarse-search codec: {encoder}"),
    };
    if let Some(gpu_encoder) = selected {
        assert_eq!(
            gpu_encoder.codec, encoder,
            "GPU encoder codec mapping corruption: requested {encoder}, resolved {}",
            gpu_encoder.codec
        );
    }
    Ok(selected)
}

fn probe_gpu_duration(input: &std::path::Path) -> anyhow::Result<f32> {
    use anyhow::bail;

    let duration_output = FfprobeBuilder::new()
        .loglevel("error")
        .show_entries("format=duration")
        .print_format("default=noprint_wrappers=1:nokey=1")
        .arg("--")
        .input(input)
        .build()
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run ffprobe for duration: {e}"))?;

    if !duration_output.status.success() {
        bail!(
            "ffprobe duration failed: {}",
            String::from_utf8_lossy(&duration_output.stderr)
        );
    }

    let duration = String::from_utf8_lossy(&duration_output.stdout)
        .trim()
        .parse::<f32>()
        .map_err(|e| anyhow::anyhow!("Failed to parse duration: {e}"))?;

    if !duration.is_finite() || duration < 0.0 {
        bail!(
            "ffprobe returned invalid GPU duration {duration} for {}",
            input.display()
        );
    }
    if crate::float_compare::approx_zero_f32(duration) {
        bail!(
            "ffprobe returned zero GPU duration for {}; refusing coarse-search on empty timeline",
            input.display()
        );
    }
    Ok(duration)
}

fn gpu_skip_reason(input_size: u64, duration: f32, ultimate_mode: bool) -> Option<String> {
    let skip_gpu_size_threshold: u64 = if ultimate_mode {
        100 * 1024
    } else {
        500 * 1024
    };
    let skip_gpu_duration_threshold: f32 = if ultimate_mode { 1.0 } else { 3.0 };

    if input_size < skip_gpu_size_threshold {
        return Some(format!(
            "file too small ({:.1}KB < {}KB)",
            crate::numeric_cast::u64_to_f64(input_size) / 1_024.0_f64,
            skip_gpu_size_threshold / 1024
        ));
    }
    if duration < skip_gpu_duration_threshold {
        return Some(format!(
            "duration too short ({duration:.1}s < {skip_gpu_duration_threshold:.1}s)"
        ));
    }
    None
}

fn build_gpu_sampling_plan(
    input_size: u64,
    duration: f32,
    ultimate_mode: bool,
) -> anyhow::Result<GpuSamplingPlan> {
    use anyhow::bail;

    if !duration.is_finite() || duration <= 0.0 {
        bail!("GPU sampling plan requires positive finite duration, got {duration}");
    }

    let tier = crate::performance_schedule::current_perf_tier();
    let very_large_bytes = crate::performance_schedule::gpu_very_large_file_threshold_bytes(tier);
    let large_bytes = crate::performance_schedule::gpu_large_file_threshold_bytes(tier);
    let very_long_secs = crate::performance_schedule::gpu_very_long_duration_threshold_secs(tier);
    let long_secs = crate::performance_schedule::gpu_long_duration_threshold_secs(tier);

    if input_size >= very_large_bytes || duration >= very_long_secs {
        return Ok(GpuSamplingPlan {
            sample_duration_limit: if ultimate_mode { 50.0 } else { 30.0 },
            skip_parallel: true,
            mode: GpuSamplingMode::Conservative,
        });
    }

    if input_size >= large_bytes || duration >= long_secs {
        return Ok(GpuSamplingPlan {
            sample_duration_limit: if ultimate_mode { 70.0 } else { 45.0 },
            skip_parallel: true,
            mode: GpuSamplingMode::Sequential,
        });
    }

    Ok(GpuSamplingPlan {
        sample_duration_limit: if ultimate_mode {
            crate::constants::GPU_SAMPLE_DURATION_ULTIMATE
        } else {
            crate::constants::GPU_SAMPLE_DURATION
        },
        skip_parallel: false,
        mode: GpuSamplingMode::Parallel,
    })
}

fn calculate_gpu_sample_input_size(
    input_size: u64,
    duration: f32,
    ultimate_mode: bool,
) -> anyhow::Result<u64> {
    use anyhow::bail;

    if !duration.is_finite() || duration <= 0.0 {
        bail!("Cannot calculate GPU sample input size from duration {duration}");
    }
    if duration < 60.0 {
        return Ok(input_size);
    }

    let multi_segment_duration = if ultimate_mode {
        crate::constants::GPU_SAMPLE_DURATION_ULTIMATE
    } else {
        crate::constants::GPU_SAMPLE_DURATION
    };
    let ratio = multi_segment_duration / duration;
    if !ratio.is_finite() || ratio <= 0.0 || ratio > 1.0 {
        bail!("Invalid GPU sample-size ratio {ratio} for duration {duration}");
    }

    crate::numeric_cast::f64_to_u64_strict(
        crate::numeric_cast::u64_to_f64(input_size) * f64::from(ratio),
        "sample_input_size",
    )
    .ok_or_else(|| anyhow::anyhow!("Failed to calculate GPU sample input size"))
}

fn calculate_gpu_warmup_input_size(
    input_size: u64,
    duration: f32,
    warmup_duration: f32,
) -> anyhow::Result<u64> {
    use anyhow::bail;

    if !duration.is_finite() || duration <= 0.0 {
        bail!("Cannot calculate GPU warmup input size from duration {duration}");
    }
    if !warmup_duration.is_finite() || warmup_duration <= 0.0 || warmup_duration > duration {
        bail!("Invalid GPU warmup duration {warmup_duration} for input duration {duration}");
    }
    if duration <= crate::constants::WARMUP_DURATION_SECS {
        return Ok(input_size);
    }

    crate::numeric_cast::f64_to_u64_strict(
        crate::numeric_cast::u64_to_f64(input_size) * f64::from(warmup_duration)
            / f64::from(duration),
        "warmup_input_size",
    )
    .ok_or_else(|| anyhow::anyhow!("Failed to calculate GPU warmup input size"))
}

fn encode_gpu_warmup(
    input: &std::path::Path,
    output: &std::path::Path,
    gpu_encoder: &GpuEncoder,
    warmup_duration: f32,
    crf: f32,
) -> anyhow::Result<u64> {
    use anyhow::Context;

    let crf_args = gpu_encoder.get_crf_args(crf)?;
    let extra_args = gpu_encoder.extra_args();
    let warmup_output = output.with_extension(temp_extension_for(output, "warmup"));

    let mut builder = FfmpegBuilder::new();
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
        crate::stream_size::measure_strict_pure_media(&warmup_output)
            .map(|measurement| measurement.pure_media_size())
            .map_err(|e| anyhow::anyhow!("Failed to measure warmup pure media: {e}"))
    } else {
        Err(anyhow::anyhow!(
            "GPU warmup encode failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ))
    };
    crate::media_conversion_gate::delivery_remove_file_or_audit(
        "gpu_warmup_output",
        &warmup_output,
    );
    size
}

fn encode_gpu_sample(
    input: &std::path::Path,
    output: &std::path::Path,
    gpu_encoder: &GpuEncoder,
    duration: f32,
    actual_sample_duration: f32,
    sample_input_size: u64,
    vf_args: &[String],
    ultimate_mode: bool,
    progress_cb: Option<&dyn Fn(f32, u64)>,
    crf: f32,
) -> anyhow::Result<u64> {
    use anyhow::{Context, bail};
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::Instant;

    let crf_args = gpu_encoder.get_crf_args(crf)?;
    let extra_args = gpu_encoder.extra_args();

    let mut builder = FfmpegBuilder::new();
    builder.overwrite();

    let use_multi_segment = duration >= 60.0;
    let sampling_vf_args = build_sampling_vf_args(vf_args, f64::from(duration), ultimate_mode);

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

    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_GPU_START_TIME.replace("{}", &beijing_time_now())
    );

    let mut last_progress_time = Instant::now();
    let mut fallback_logged = false;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if let Some(val) = line.strip_prefix("out_time_us=") {
                        let time_us = match val.parse::<u64>() {
                            Ok(time_us) => time_us,
                            Err(err) => {
                                crate::media_conversion_gate::delivery_gpu_batch_audit(
                                    "gpu_progress_time_parse_failed",
                                    format!(
                                        "failed to parse GPU ffmpeg out_time_us token {val:?}: \
                                         {err}"
                                    ),
                                );
                                continue;
                            }
                        };
                        if last_progress_time.elapsed().as_secs_f64() < 1.0_f64 {
                            continue;
                        }
                        let current_secs =
                            crate::numeric_cast::u64_to_f64(time_us) / 1_000_000.0_f64;
                        let pct =
                            (current_secs / f64::from(actual_sample_duration) * 100.0).min(100.0);
                        let elapsed_secs = start_time.elapsed().as_secs_f64();
                        let eta =
                            if pct > 0.1_f64 && current_secs > 0.0_f64 && elapsed_secs > 0.0_f64 {
                                let speed = current_secs / elapsed_secs;
                                if speed > 0.0_f64 {
                                    crate::numeric_cast::f64_to_u64_strict(
                                        ((f64::from(actual_sample_duration) - current_secs)
                                            / speed)
                                            .max(0.0),
                                        "gpu_eta",
                                    )
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("Failed to calculate GPU progress ETA")
                                    })?
                                } else {
                                    0
                                }
                            } else {
                                0
                            };
                        let speed = if current_secs > 0.0_f64 {
                            start_time.elapsed().as_secs_f64() / current_secs
                        } else {
                            0.0_f64
                        };

                        let estimated_final_size = match std::fs::metadata(output) {
                            Ok(metadata) => {
                                let current_size = metadata.len();
                                fallback_logged = false;
                                crate::numeric_cast::f64_to_u64_strict(
                                    crate::numeric_cast::u64_to_f64(current_size) / pct.max(1.0)
                                        * 100.0,
                                    "estimated_final_size",
                                )
                                .ok_or_else(|| {
                                    anyhow::anyhow!("Failed to estimate GPU final output size")
                                })?
                            }
                            Err(err) => {
                                if !fallback_logged {
                                    crate::media_conversion_gate::delivery_gpu_batch_audit(
                                        "gpu_progress_metadata_unavailable",
                                        format!(
                                            "metadata unavailable for {}: {err}; using linear \
                                             estimate",
                                            output.display()
                                        ),
                                    );
                                    crate::log_detail!(
                                        "Using linear estimation (metadata unavailable)"
                                    );
                                    fallback_logged = true;
                                }
                                crate::numeric_cast::f64_to_u64_strict(
                                    (crate::numeric_cast::u64_to_f64(sample_input_size)
                                        * (1.0 / pct.max(0.1)))
                                    .min(crate::numeric_cast::u64_to_f64(sample_input_size) * 10.0),
                                    "linear_estimated_size",
                                )
                                .ok_or_else(|| {
                                    anyhow::anyhow!("Failed to estimate GPU linear output size")
                                })?
                            }
                        };

                        crate::log_detail!(
                            "⏳ Progress: {:.1}% ({:.1}s / {:.1}s) - ETA: {}s - Speed: {:.2}x",
                            pct,
                            current_secs,
                            actual_sample_duration,
                            eta,
                            speed
                        );

                        if let Some(cb) = progress_cb {
                            cb(crf, estimated_final_size);
                        }
                        last_progress_time = Instant::now();
                    }
                }
                Err(err) => {
                    crate::log_detail!(
                        "{} Failed to read GPU encoder stdout progress stream: {}",
                        crate::modern_ui::symbols::styled_warning_icon(),
                        err
                    );
                    break;
                }
            }
        }
    }

    let status = child.wait().context("Failed to wait for ffmpeg")?;

    if let Some(handle) = stderr_handle
        && let Err(payload) = handle.join()
    {
        crate::log_detail!(
            "{} GPU stderr capture thread panicked: {}",
            crate::modern_ui::symbols::styled_warning_icon(),
            describe_thread_panic(payload)
        );
    }

    if !status.success() {
        let stderr_lines = stderr_capture.get_lines();
        let stderr_text = if stderr_lines.is_empty() {
            "<stderr was empty; ffmpeg emitted no diagnostic lines>".to_string()
        } else {
            stderr_lines.join("\n")
        };
        bail!(
            "GPU encoding failed (exit code: {:?})\nInput: {}\nOutput: {}\nEncoder: {}\nCRF: \
             {:.1}\nSample duration: {:.1}s\nStderr:\n{}",
            status.code(),
            input.display(),
            output.display(),
            gpu_encoder.name,
            crf,
            actual_sample_duration,
            stderr_text
        );
    }

    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_GPU_DONE_TIME.replace("{}", &beijing_time_now())
    );
    Ok(crate::stream_size::measure_strict_pure_media(output)?.pure_media_size())
}

fn encode_gpu_parallel_probe(
    input: &std::path::Path,
    output: &std::path::Path,
    gpu_encoder: &GpuEncoder,
    actual_sample_duration: f32,
    crfs: &[f32],
) -> Vec<(f32, anyhow::Result<u64>)> {
    crfs.iter()
        .enumerate()
        .map(|(i, &crf)| {
            let crf_args = match gpu_encoder.get_crf_args(crf) {
                Ok(args) => args,
                Err(err) => return Err((crf, err)),
            };
            let extra_args: Vec<String> = gpu_encoder
                .extra_args()
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            let input_path = input.to_path_buf();
            let output_path = output.with_extension(format!("tmp{i}.mp4"));
            let encoder_name = gpu_encoder.name.to_string();
            let sample_dur = actual_sample_duration;

            Ok(thread::spawn(move || {
                let _gpu_slot_guard = GpuSlotGuard;
                acquire_gpu_slot();
                let mut builder = FfmpegBuilder::new();
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
                    Ok(out) if out.status.success() => {
                        crate::stream_size::measure_strict_pure_media(&output_path)
                            .map(|measurement| measurement.pure_media_size())
                    }
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        Err(anyhow::anyhow!(
                            "GPU encoding failed: {}",
                            crate::io_utils::tail_error_lines(&stderr, 5)
                        ))
                    }
                    Err(e) => Err(anyhow::anyhow!("{e}")),
                };

                crate::media_conversion_gate::delivery_remove_file_or_audit(
                    "gpu_probe_output",
                    &output_path,
                );

                (crf, size)
            }))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|handle| match handle {
            Err((crf, err)) => (crf, Err(err)),
            Ok(join_handle) => match join_handle.join() {
                Ok(result) => result,
                Err(payload) => (
                    f32::NAN,
                    Err(anyhow::anyhow!(
                        "GPU probe thread panicked: {}",
                        describe_thread_panic(payload)
                    )),
                ),
            },
        })
        .collect()
}

fn build_probe_crfs(config: &GpuCoarseConfig) -> ([f32; 3], String) {
    let use_initial =
        config.initial_crf >= config.min_crf + 5.0 && config.initial_crf <= config.max_crf - 5.0;
    let mid_crf = f32::midpoint(config.min_crf, config.max_crf);

    if use_initial {
        (
            [config.initial_crf, config.max_crf, config.min_crf],
            format!(
                "   {} [GPU] Using initial_crf {:.0} as search anchor",
                crate::media_conversion_gate::ui_icon_pick("🎯", "[TARGET]"),
                config.initial_crf
            ),
        )
    } else {
        (
            [mid_crf, config.max_crf, config.min_crf],
            format!(
                "   {} [GPU] initial_crf {:.0} out of range, using mid_crf {:.0}",
                crate::modern_ui::symbols::styled_warning_icon(),
                config.initial_crf,
                mid_crf
            ),
        )
    }
}

fn find_probe_result(
    probe_results: &[(f32, anyhow::Result<u64>)],
    target: f32,
) -> Option<&(f32, anyhow::Result<u64>)> {
    probe_results
        .iter()
        .find(|(crf, _result)| (*crf - target).abs() < 0.1)
}

fn analyze_initial_probe_results(
    probe_crfs: [f32; 3],
    probe_results: &[(f32, anyhow::Result<u64>)],
    sample_input_size: u64,
    config: &GpuCoarseConfig,
) -> anyhow::Result<InitialProbeOutcome> {
    use anyhow::{anyhow, bail};

    let [initial_crf_target, max_crf_target, min_crf_target] = probe_crfs;
    let initial_result = find_probe_result(probe_results, initial_crf_target)
        .ok_or_else(|| anyhow!("Initial GPU probe result missing for CRF {initial_crf_target}"))?;
    let max_result = find_probe_result(probe_results, max_crf_target);
    let min_result = find_probe_result(probe_results, min_crf_target);

    let mut outcome = InitialProbeOutcome {
        best_crf: None,
        best_size: None,
        boundary_low: config.min_crf,
        boundary_high: config.max_crf,
        prev_size: None,
        found_compress_point: false,
        messages: Vec::new(),
    };

    let (_, initial_size_result) = initial_result;
    let Ok(initial_size) = initial_size_result else {
        return Ok(outcome);
    };

    if *initial_size < sample_input_size {
        outcome.best_crf = Some(initial_crf_target);
        outcome.best_size = Some(*initial_size);
        outcome.found_compress_point = true;
        outcome.boundary_low = initial_crf_target;
        outcome.boundary_high = config.max_crf;
        outcome.messages.push(format!(
            "   {} initial_crf {:.0} compresses! Searching higher CRF [{:.0}, {:.0}]",
            crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
            initial_crf_target,
            outcome.boundary_low,
            outcome.boundary_high
        ));

        if let Some((_crf, Ok(max_size))) = max_result
            && *max_size < sample_input_size
            && *max_size < *initial_size
        {
            outcome.best_crf = Some(config.max_crf);
            outcome.best_size = Some(*max_size);
            outcome.messages.push(format!(
                "   {} max_crf {:.0} is better: {:.1}% smaller",
                crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]"),
                config.max_crf,
                (1.0_f64
                    - crate::numeric_cast::u64_to_f64(*max_size)
                        / crate::numeric_cast::u64_to_f64(*initial_size))
                    * 100.0_f64
            ));
        }
        return Ok(outcome);
    }

    outcome.boundary_low = config.min_crf;
    outcome.boundary_high = initial_crf_target;
    outcome.prev_size = Some(*initial_size);
    outcome.messages.push(format!(
        "   {} initial_crf {:.0} cannot compress! Searching lower CRF [{:.0}, {:.0}]",
        crate::modern_ui::symbols::styled_warning_icon(),
        initial_crf_target,
        outcome.boundary_low,
        outcome.boundary_high
    ));

    if let Some((_crf, Ok(min_size))) = min_result
        && *min_size < sample_input_size
    {
        outcome.best_crf = Some(config.min_crf);
        outcome.best_size = Some(*min_size);
        outcome.found_compress_point = true;
        outcome.messages.push(format!(
            "   {} min_crf {:.0} compresses!",
            crate::media_conversion_gate::ui_icon_pick("✅", "[OK]"),
            config.min_crf
        ));
    }

    if outcome.boundary_low > outcome.boundary_high {
        bail!(
            "GPU probe corrupted search boundaries: low {:.2} > high {:.2}",
            outcome.boundary_low,
            outcome.boundary_high
        );
    }

    Ok(outcome)
}

fn parse_ffmpeg_ssim(stderr: &str) -> Result<Option<f64>, String> {
    for line in stderr
        .lines()
        .filter(|line| line.contains("SSIM") && line.contains("All:"))
    {
        if let Some(pos) = line.find("All:") {
            let after_all = &line[pos + 4..];
            let value = match after_all.find(' ') {
                Some(pos) => &after_all[..pos],
                None => after_all,
            };
            return crate::video_explorer::precision::parse_explore_ssim_metric_token(value.trim());
        }
    }
    Ok(None)
}

fn validate_final_gpu_quality<F>(
    best_crf: Option<f32>,
    input: &std::path::Path,
    output: &std::path::Path,
    encode_gpu: &mut F,
    psnr_ssim_mapper: &mut PsnrSsimMapper,
    ultimate_mode: bool,
) -> FinalGpuValidation
where
    F: FnMut(f32) -> anyhow::Result<u64>,
{
    let Some(last_tested_crf) = best_crf else {
        return FinalGpuValidation::default();
    };

    let mut validation = FinalGpuValidation {
        gpu_ssim: None,
        gpu_psnr: None,
        messages: vec![format!(
            "   {} Final quality validation at CRF {:.1}",
            crate::media_conversion_gate::ui_icon_pick("📍", "[TARGET]"),
            last_tested_crf
        )],
    };

    if ultimate_mode {
        validation.messages.push(format!(
            "      {} Ultimate mode: skipping final GPU SSIM (3D quality gate owns perceptual \
             validation)",
            crate::media_conversion_gate::ui_icon_pick("ℹ️", "[INFO]")
        ));
        return validation;
    }

    match encode_gpu(last_tested_crf) {
        Ok(_) => {
            let ssim_output = FfmpegBuilder::new()
                .input(input)
                .input(output)
                .filter_complex("ssim")
                .format("null")
                .output_pipe()
                .build()
                .output();

            let psnr_result =
                calculate_psnr_fast(&input.to_string_lossy(), &output.to_string_lossy());

            validation.gpu_ssim = match ssim_output {
                Ok(out) if out.status.success() => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    match parse_ffmpeg_ssim(&stderr) {
                        Ok(Some(ssim)) => {
                            validation.messages.push(format!(
                                "      {} Final GPU SSIM: {ssim:.6}",
                                crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
                            ));
                            Some(ssim)
                        }
                        Ok(None) => {
                            validation.messages.push(format!(
                                "      {} Final GPU SSIM unavailable: unable to parse ffmpeg SSIM \
                                 output",
                                crate::modern_ui::symbols::styled_warning_icon()
                            ));
                            None
                        }
                        Err(err) => {
                            validation.messages.push(format!(
                                "      {} Final GPU SSIM parse failed: {err}",
                                crate::modern_ui::symbols::styled_warning_icon()
                            ));
                            None
                        }
                    }
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    validation.messages.push(format!(
                        "      {} Final GPU SSIM command failed: {}",
                        crate::modern_ui::symbols::styled_warning_icon(),
                        crate::io_utils::tail_error_lines(&stderr, 5)
                    ));
                    None
                }
                Err(err) => {
                    validation.messages.push(format!(
                        "      {} Failed to run final GPU SSIM command: {err}",
                        crate::modern_ui::symbols::styled_warning_icon()
                    ));
                    None
                }
            };

            validation.gpu_psnr = match psnr_result {
                Ok(psnr) => {
                    validation.messages.push(format!(
                        "      {} Final GPU PSNR: {psnr:.2}dB",
                        crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
                    ));
                    Some(psnr)
                }
                Err(err) => {
                    validation.messages.push(format!(
                        "      {} Final GPU PSNR unavailable: {err}",
                        crate::modern_ui::symbols::styled_warning_icon()
                    ));
                    None
                }
            };

            if let (Some(psnr), Some(ssim)) = (validation.gpu_psnr, validation.gpu_ssim) {
                psnr_ssim_mapper.add_calibration_point(psnr, ssim);
                validation.messages.push(format!(
                    "      {} Added PSNR-SSIM calibration point: {psnr:.2}dB → {ssim:.6}",
                    crate::media_conversion_gate::ui_icon_pick("✅", "[OK]")
                ));
            }
        }
        Err(err) => validation.messages.push(format!(
            "      {} Final GPU validation encode failed at CRF {last_tested_crf:.1}: {err}",
            crate::modern_ui::symbols::styled_warning_icon()
        )),
    }

    validation
}

fn execute_initial_gpu_probe<F, G>(
    config: &GpuCoarseConfig,
    sampling_plan: GpuSamplingPlan,
    sample_input_size: u64,
    progress_cb: Option<&dyn Fn(f32, u64)>,
    size_cache: &mut CrfCache<u64>,
    iterations: &mut u32,
    encode_gpu: &mut F,
    encode_parallel: &G,
) -> anyhow::Result<(GpuSearchState, Vec<String>)>
where
    F: FnMut(f32) -> anyhow::Result<u64>,
    G: Fn(&[f32]) -> Vec<(f32, anyhow::Result<u64>)>,
{
    let (probe_crfs, probe_message) = build_probe_crfs(config);
    let mut messages = vec![probe_message];

    let probe_results = if sampling_plan.skip_parallel {
        messages.push(format!(
            "   {} Skip parallel probe (large file mode)",
            crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
        ));
        let test_crf = probe_crfs[0];
        messages.push(format!(
            "   {} [GPU] Testing CRF {test_crf:.0} (anchor point)...",
            crate::media_conversion_gate::ui_icon_pick("🔄", "~")
        ));
        let single_result = encode_gpu(test_crf);
        match &single_result {
            Ok(size) => {
                size_cache.insert(test_crf, *size);
                *iterations += 1;
                if let Some(cb) = progress_cb {
                    cb(test_crf, *size);
                }
            }
            Err(err) => messages.push(format!(
                "   {} Anchor probe failed: {err}",
                crate::modern_ui::symbols::styled_warning_icon()
            )),
        }
        vec![(test_crf, single_result)]
    } else {
        messages.push(format!(
            "   {} [GPU] Parallel probe: CRF {:.0}, {:.0}, {:.0}",
            crate::media_conversion_gate::ui_icon_pick("🚀", "[LAUNCH]"),
            probe_crfs[0],
            probe_crfs[1],
            probe_crfs[2]
        ));
        encode_parallel(&probe_crfs)
    };

    if !sampling_plan.skip_parallel {
        for (crf, result) in &probe_results {
            match result {
                Ok(size) => {
                    size_cache.insert(*crf, *size);
                    *iterations += 1;
                    if let Some(cb) = progress_cb {
                        cb(*crf, *size);
                    }
                }
                Err(err) => messages.push(format!(
                    "   {} Parallel probe CRF {crf:.1} failed: {err}",
                    crate::modern_ui::symbols::styled_warning_icon()
                )),
            }
        }
    }

    let probe_outcome =
        analyze_initial_probe_results(probe_crfs, &probe_results, sample_input_size, config)?;
    messages.extend(probe_outcome.messages.iter().cloned());

    Ok((
        GpuSearchState {
            best_crf: probe_outcome.best_crf,
            best_size: probe_outcome.best_size,
            boundary_low: probe_outcome.boundary_low,
            boundary_high: probe_outcome.boundary_high,
            prev_size: probe_outcome.prev_size,
            found_compress_point: probe_outcome.found_compress_point,
        },
        messages,
    ))
}

fn run_gpu_stage1<F>(
    state: &mut GpuSearchState,
    config: &GpuCoarseConfig,
    sample_input_size: u64,
    max_iterations_limit: u32,
    iterations: &mut u32,
    progress_cb: Option<&dyn Fn(f32, u64)>,
    size_cache: &mut CrfCache<u64>,
    encode_cached: &mut F,
) -> Vec<String>
where
    F: FnMut(f32, &mut CrfCache<u64>) -> anyhow::Result<u64>,
{
    let gpu_decay_factor: f32 = if config.ultimate_mode { 0.6 } else { 0.5 };
    let gpu_max_wall_hits: u32 = if config.ultimate_mode { 6 } else { 4 };
    let gpu_min_step: f32 = if config.ultimate_mode { 0.1 } else { 0.5 };
    let stage1_threshold = if config.ultimate_mode { 2.0 } else { 4.0 };
    let mut messages = Vec::new();

    if (state.boundary_high - state.boundary_low) <= stage1_threshold {
        return messages;
    }

    if state.found_compress_point {
        let crf_range = config.max_crf - state.boundary_low;
        let initial_step = (crf_range / 2.0).clamp(4.0, 15.0);

        messages.push(format!(
            "   {} Stage 1A: Curve model search upward (v6.0)",
            crate::media_conversion_gate::ui_icon_pick("📈", "[CHART]")
        ));
        messages.push(format!(
            "      CRF range: {crf_range:.1} → Initial step: {initial_step:.1}"
        ));
        messages.push(format!(
            "      Strategy: step × {gpu_decay_factor:.1} per wall hit, max {gpu_max_wall_hits} \
             hits"
        ));

        let mut stagnation_count = 0u32;
        let mut last_size = crate::media_conversion_gate::delivery_gpu_phase_best_size_required(
            state.best_size,
            "Logic error: best_size is missing in Phase 1A",
        );

        let mut current_step = initial_step;
        let mut wall_hits: u32 = 0;
        let mut test_crf = state.boundary_low + current_step;
        let mut last_compressible_crf = state.boundary_low;
        let mut last_compressible_size =
            crate::media_conversion_gate::delivery_gpu_phase_best_size_required(
                state.best_size,
                "Logic error: best_size is missing in Phase 1A (last_compressible)",
            );

        while test_crf <= config.max_crf && *iterations < max_iterations_limit {
            let cached = size_cache.get(test_crf).copied();
            let size_result = if let Some(size) = cached {
                Ok(size)
            } else {
                encode_cached(test_crf, size_cache)
            };

            match size_result {
                Ok(size) => {
                    if cached.is_none() {
                        *iterations += 1;
                        if let Some(cb) = progress_cb {
                            cb(test_crf, size);
                        }
                    }

                    let size_delta_pct = if last_size > 0 {
                        (crate::numeric_cast::u64_to_f64(size)
                            - crate::numeric_cast::u64_to_f64(last_size))
                        .abs()
                            / crate::numeric_cast::u64_to_f64(last_size.max(1))
                            * 100.0_f64
                    } else {
                        100.0_f64
                    };
                    last_size = size;

                    if size_delta_pct < 0.5_f64 {
                        stagnation_count += 1;
                    } else {
                        stagnation_count = 0;
                    }

                    if size < sample_input_size {
                        last_compressible_crf = test_crf;
                        last_compressible_size = size;
                        state.best_crf = Some(test_crf);
                        state.best_size = Some(size);
                        state.boundary_low = test_crf;
                        messages.push(format!(
                            "   {} CRF {:.1}: {:.1}% (step {:.1}) → continue",
                            crate::media_conversion_gate::ui_icon_pick("✓", "Y"),
                            test_crf,
                            (crate::numeric_cast::u64_to_f64(size)
                                / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                                - 1.0_f64)
                                * 100.0_f64,
                            current_step
                        ));

                        if stagnation_count >= 3 {
                            messages.push(format!(
                                "   {} [GPU] Size plateau detected ({stagnation_count} stagnant \
                                 iterations). Stopping Stage 1A.",
                                crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
                            ));
                            break;
                        }

                        test_crf += current_step;
                    } else {
                        wall_hits += 1;
                        messages.push(format!(
                            "   {} CRF {:.1}: WALL HIT #{} (size +{:.1}%)",
                            crate::media_conversion_gate::ui_icon_pick("✗", "N"),
                            test_crf,
                            wall_hits,
                            (crate::numeric_cast::u64_to_f64(size)
                                / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                                - 1.0_f64)
                                * 100.0_f64
                        ));

                        if wall_hits >= gpu_max_wall_hits {
                            messages.push(format!(
                                "   {} MAX WALL HITS ({gpu_max_wall_hits})! Stopping at CRF \
                                 {last_compressible_crf:.1}",
                                crate::media_conversion_gate::ui_icon_pick("🧱", "[FIX]")
                            ));
                            state.boundary_high = test_crf;
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
                        messages.push(format!(
                            "   {} Curve backtrack: step {current_step:.1} → {new_step:.1} \
                             ({phase_info})",
                            crate::media_conversion_gate::ui_icon_pick("↩️", "<")
                        ));

                        current_step = new_step;
                        state.boundary_high = test_crf;
                        test_crf = last_compressible_crf + current_step;
                        stagnation_count = 0;
                    }
                }
                Err(err) => {
                    messages.push(format!(
                        "   {} Encoding failed at CRF {test_crf:.1} ({err}), stopping climb",
                        crate::modern_ui::symbols::styled_warning_icon()
                    ));
                    break;
                }
            }
        }

        if last_compressible_crf > 0.0 {
            state.best_crf = Some(last_compressible_crf);
            state.best_size = Some(last_compressible_size);
        }
        return messages;
    }

    let crf_range = state.boundary_high - config.min_crf;
    let initial_step = (crf_range / 2.0).clamp(4.0, 15.0);

    messages.push(format!(
        "   {} Stage 1B: Curve model search downward (v6.0)",
        crate::media_conversion_gate::ui_icon_pick("📉", "[CHART]")
    ));
    messages.push(format!(
        "      CRF range: {crf_range:.1} → Initial step: {initial_step:.1}"
    ));

    let mut current_step = initial_step;
    let mut wall_hits: u32 = 0;
    let mut test_crf = state.boundary_high - current_step;
    let mut last_fail_crf = state.boundary_high;

    while test_crf >= config.min_crf && *iterations < max_iterations_limit {
        let cached = size_cache.get(test_crf).copied();
        let size_result = if let Some(size) = cached {
            Ok(size)
        } else {
            encode_cached(test_crf, size_cache)
        };

        match size_result {
            Ok(size) => {
                if cached.is_none() {
                    *iterations += 1;
                    if let Some(cb) = progress_cb {
                        cb(test_crf, size);
                    }
                }

                if size < sample_input_size {
                    state.best_crf = Some(test_crf);
                    state.best_size = Some(size);
                    state.found_compress_point = true;
                    state.boundary_low = test_crf;
                    messages.push(format!(
                        "   {} CRF {:.1}: {:.1}% (step {:.1}) → found compress point",
                        crate::media_conversion_gate::ui_icon_pick("✓", "Y"),
                        test_crf,
                        (crate::numeric_cast::u64_to_f64(size)
                            / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                            - 1.0_f64)
                            * 100.0_f64,
                        current_step
                    ));
                    break;
                }
                wall_hits += 1;
                messages.push(format!(
                    "   {} CRF {:.1}: WALL HIT #{} (size +{:.1}%)",
                    crate::media_conversion_gate::ui_icon_pick("✗", "N"),
                    test_crf,
                    wall_hits,
                    (crate::numeric_cast::u64_to_f64(size)
                        / crate::numeric_cast::u64_to_f64(sample_input_size.max(1))
                        - 1.0_f64)
                        * 100.0_f64
                ));

                if wall_hits >= gpu_max_wall_hits {
                    messages.push(format!(
                        "   {} MAX WALL HITS ({gpu_max_wall_hits})! Cannot find compress point",
                        crate::media_conversion_gate::ui_icon_pick("🧱", "[FIX]")
                    ));
                    break;
                }

                let curve_step = initial_step * gpu_decay_factor.powi(wall_hits.cast_signed());
                let new_step = if curve_step < 1.0 {
                    gpu_min_step
                } else {
                    curve_step
                };

                messages.push(format!(
                    "   {} Curve backtrack: step {current_step:.1} → {new_step:.1}",
                    crate::media_conversion_gate::ui_icon_pick("↩️", "<")
                ));

                current_step = new_step;
                last_fail_crf = test_crf;
                state.prev_size = Some(size);
                test_crf -= current_step;
            }
            Err(err) => {
                messages.push(format!(
                    "   {} Encoding failed at CRF {test_crf:.1} ({err}), stopping descent",
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
                break;
            }
        }
    }

    // Tighten the unexplored gap for Stage2: every CRF at or above the last wall
    // hit is a known non-compressing point, so binary refinement need not
    // revisit it.
    if state.found_compress_point && last_fail_crf < state.boundary_high {
        state.boundary_high = last_fail_crf;
    }
    messages
}

fn run_gpu_stage2<F>(
    state: &mut GpuSearchState,
    sample_input_size: u64,
    max_iterations_limit: u32,
    iterations: &mut u32,
    progress_cb: Option<&dyn Fn(f32, u64)>,
    size_cache: &mut CrfCache<u64>,
    encode_cached: &mut F,
) -> Vec<String>
where
    F: FnMut(f32, &mut CrfCache<u64>) -> anyhow::Result<u64>,
{
    let mut messages = Vec::new();
    let skip_stage2 = state.best_crf.is_some_and(|b| {
        let fract = (b * 2.0).fract();
        fract.abs() < 0.01 || (fract - 1.0).abs() < 0.01
    });

    if skip_stage2 {
        messages.push(format!(
            "   {} Skip Stage2: boundary at 0.5 precision",
            crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
        ));
        return messages;
    }
    if !state.found_compress_point || (state.boundary_high - state.boundary_low) <= 1.0 {
        return messages;
    }

    let Some(mut lo) = crate::numeric_cast::f32_to_i32_strict(state.boundary_low.ceil(), "lo")
    else {
        crate::media_conversion_gate::delivery_gpu_batch_audit(
            "delivery_gpu",
            format!(
                "Skipping GPU Stage2 because low boundary {} could not be represented as i32",
                state.boundary_low.ceil()
            ),
        );
        messages.push(format!(
            "   {} Skip Stage2: invalid low boundary prevented honest binary refinement",
            crate::modern_ui::symbols::styled_warning_icon()
        ));
        return messages;
    };
    let Some(mut hi) =
        crate::numeric_cast::f32_to_i32_strict(state.boundary_high.floor(), "gpu_search_hi")
    else {
        crate::media_conversion_gate::delivery_gpu_batch_audit(
            "delivery_gpu",
            format!(
                "Skipping GPU Stage2 because high boundary {} could not be represented as i32",
                state.boundary_high.floor()
            ),
        );
        messages.push(format!(
            "   {} Skip Stage2: invalid high boundary prevented honest binary refinement",
            crate::modern_ui::symbols::styled_warning_icon()
        ));
        return messages;
    };
    let max_binary_iter = 5_i32;
    let mut binary_iter = 0_i32;

    while lo < hi && *iterations < max_iterations_limit && binary_iter < max_binary_iter {
        binary_iter += 1_i32;
        let mid = lo + (hi - lo) / 2_i32;
        let test_crf =
            crate::media_conversion_gate::delivery_gpu_binary_search_crf_from_mid(mid, hi);

        if let Some(&cached_size) = size_cache.get(test_crf) {
            if cached_size < sample_input_size {
                hi = mid;
                state.best_crf = Some(test_crf);
                state.best_size = Some(cached_size);
            } else {
                lo = mid + 1_i32;
            }
            continue;
        }

        match encode_cached(test_crf, size_cache) {
            Ok(size) => {
                *iterations += 1;
                if let Some(cb) = progress_cb {
                    cb(test_crf, size);
                }
                if let Some(prev) = state.prev_size {
                    let rate = calc_gpu_change_rate(prev, size);
                    if rate < crate::constants::CHANGE_RATE_THRESHOLD {
                        messages.push(format!(
                            "   {} Stage2 early stop: Δ{:.3}%",
                            crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]"),
                            rate * 100.0_f64
                        ));
                        break;
                    }
                }

                if size < sample_input_size {
                    hi = mid;
                    state.best_crf = Some(test_crf);
                    state.best_size = Some(size);
                    state.prev_size = Some(size);
                } else {
                    lo = mid + 1_i32;
                }
            }
            Err(err) => {
                messages.push(format!(
                    "   {} Encoding failed at CRF {test_crf:.1} ({err}), stopping binary \
                     refinement",
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
                break;
            }
        }
    }
    messages
}

fn run_gpu_stage3<F>(
    state: &mut GpuSearchState,
    config: &GpuCoarseConfig,
    sample_input_size: u64,
    max_iterations_limit: u32,
    iterations: &mut u32,
    progress_cb: Option<&dyn Fn(f32, u64)>,
    size_cache: &mut CrfCache<u64>,
    encode_cached: &mut F,
    input: &std::path::Path,
    output: &std::path::Path,
) -> (QualityCeilingDetector, Vec<String>)
where
    F: FnMut(f32, &mut CrfCache<u64>) -> anyhow::Result<u64>,
{
    let mut ceiling_detector = QualityCeilingDetector::new();
    let mut messages = Vec::new();

    if let Some(mut current_best) = state.best_crf {
        if *iterations >= max_iterations_limit {
            messages.push(format!(
                "   {} Skip Stage3: reached absolute limit ({max_iterations_limit})",
                crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
            ));
            return (ceiling_detector, messages);
        }

        let stage3_step = if config.ultimate_mode { 0.1 } else { 0.5 };
        messages.push(format!(
            "   {} Stage 3: Fine-tune with {stage3_step:.1} step (quality ceiling detection)",
            crate::media_conversion_gate::ui_icon_pick("📍", "[TARGET]")
        ));

        let mut offset = stage3_step;
        let mut consecutive_small_improvements = 0_i32;
        let mut stage3_spins = 0u32;
        let stage3_spin_cap = max_iterations_limit.saturating_mul(8).max(512);

        while *iterations < max_iterations_limit {
            stage3_spins += 1;
            if stage3_spins > stage3_spin_cap {
                messages.push(format!(
                    "   {} Stage 3: stopping after spin safety cap ({stage3_spin_cap})",
                    crate::modern_ui::symbols::styled_warning_icon()
                ));
                break;
            }

            let test_crf = current_best - offset;
            if test_crf < config.min_crf {
                messages.push(format!(
                    "   {} Stop: reached min_crf {:.1}",
                    crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]"),
                    config.min_crf
                ));
                break;
            }

            let result = if let Some(&cached_size) = size_cache.get(test_crf) {
                messages.push(format!(
                    "   {} Cache hit: CRF {test_crf:.1}",
                    crate::media_conversion_gate::ui_icon_pick("📦", "[CACHE]")
                ));
                Ok(cached_size)
            } else {
                let encode_result = encode_cached(test_crf, size_cache);
                if encode_result.is_ok() {
                    *iterations += 1;
                }
                encode_result
            };

            match result {
                Ok(size) => {
                    if let Some(cb) = progress_cb {
                        cb(test_crf, size);
                    }

                    if size < sample_input_size {
                        let improvement = crate::media_conversion_gate::explore_encode_size_improvement_pct_optional(
                            state.best_size,
                            size,
                            "gpu coarse encode size improvement",
                        );
                        if let Some(improvement) = improvement {
                            messages.push(format!(
                                "   {} CRF {test_crf:.1}: {improvement:.1}% improvement",
                                crate::media_conversion_gate::ui_icon_pick("✓", "Y")
                            ));
                        } else {
                            messages.push(format!(
                                "   {} CRF {test_crf:.1}: new best size {size}",
                                crate::media_conversion_gate::ui_icon_pick("✓", "Y")
                            ));
                        }

                        state.best_crf = Some(test_crf);
                        state.best_size = Some(size);
                        current_best = test_crf;

                        let input_str = input.to_string_lossy();
                        let output_str = output.to_string_lossy();
                        match calculate_psnr_fast(&input_str, &output_str) {
                            Ok(psnr) => {
                                messages.push(format!(
                                    "      {} PSNR: {psnr:.2}dB",
                                    crate::media_conversion_gate::ui_icon_pick("📊", "[STATS]")
                                ));

                                if ceiling_detector.add_sample(test_crf, psnr)
                                    && let Some((ceiling_crf, ceiling_psnr)) =
                                        ceiling_detector.get_ceiling()
                                {
                                    messages.push(format!(
                                        "   {} GPU Quality Ceiling Detected!",
                                        crate::media_conversion_gate::ui_icon_pick(
                                            "🎯", "[TARGET]"
                                        )
                                    ));
                                    messages.push(format!(
                                        "      └─ CRF {ceiling_crf:.1}, PSNR {ceiling_psnr:.2}dB \
                                         (PSNR plateau)"
                                    ));
                                    messages.push(
                                        "      └─ Further CRF reduction won't improve quality"
                                            .to_string(),
                                    );
                                    messages.push(format!(
                                        "   {} Stop: GPU reached its quality limit",
                                        crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
                                    ));
                                    break;
                                }
                            }
                            Err(err) => {
                                messages.push(format!(
                                    "      {} PSNR calc failed ({err}), fallback to size-only",
                                    crate::modern_ui::symbols::styled_warning_icon()
                                ));
                            }
                        }

                        if let Some(improvement) = improvement {
                            if improvement < 0.5_f64 {
                                consecutive_small_improvements += 1_i32;
                                messages.push(format!(
                                    "      {} Small improvement \
                                     ({consecutive_small_improvements}/2)",
                                    crate::modern_ui::symbols::styled_warning_icon()
                                ));
                                if consecutive_small_improvements >= 2_i32 {
                                    messages.push(format!(
                                        "   {} Stop: 2 consecutive improvements < 0.5%",
                                        crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
                                    ));
                                    break;
                                }
                            } else if improvement < 1.0_f64 {
                                messages.push(format!(
                                    "      {} Improvement < 1%, may stop soon",
                                    crate::modern_ui::symbols::styled_warning_icon()
                                ));
                                consecutive_small_improvements += 1_i32;
                                if consecutive_small_improvements >= 3_i32 {
                                    messages.push(format!(
                                        "   {} Stop: 3 consecutive improvements < 1%",
                                        crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]")
                                    ));
                                    break;
                                }
                            } else {
                                consecutive_small_improvements = 0_i32;
                            }
                        } else {
                            consecutive_small_improvements = 0_i32;
                        }

                        offset += 0.5;
                    } else {
                        messages.push(format!(
                            "   {} CRF {test_crf:.1} cannot compress → boundary reached",
                            crate::media_conversion_gate::ui_icon_pick("✗", "N")
                        ));
                        break;
                    }
                }
                Err(err) => {
                    messages.push(format!(
                        "   {} Encoding failed at CRF {test_crf:.1} ({err}), stopping",
                        crate::modern_ui::symbols::styled_warning_icon()
                    ));
                    break;
                }
            }
        }

        if *iterations >= max_iterations_limit {
            messages.push(format!(
                "   {} Reached absolute iteration limit ({max_iterations_limit}) in Stage 3",
                crate::modern_ui::symbols::styled_warning_icon()
            ));
        }

        if ceiling_detector.ceiling_detected
            && let Some((ceiling_crf, ceiling_psnr)) = ceiling_detector.get_ceiling()
        {
            messages.push("   ═══════════════════════════════════════════════════".to_string());
            messages.push(format!(
                "   {} GPU Quality Ceiling Summary:",
                crate::media_conversion_gate::ui_icon_pick("🎯", "[TARGET]")
            ));
            messages.push(format!("      CRF: {ceiling_crf:.1}"));
            messages.push(format!("      PSNR: {ceiling_psnr:.2}dB"));
            messages.push("      Note: GPU encoder reached its quality limit".to_string());
            messages.push("      CPU encoding can break through this ceiling".to_string());
        }
    }

    (ceiling_detector, messages)
}

fn finalize_gpu_search_result(
    state: &GpuSearchState,
    ceiling_detector: &QualityCeilingDetector,
    final_validation: &FinalGpuValidation,
    psnr_ssim_mapper: &PsnrSsimMapper,
    iterations: u32,
    sample_input_size: u64,
    gpu_type: GpuType,
    encoder: &str,
    config: &GpuCoarseConfig,
) -> (GpuCoarseResult, Vec<String>) {
    let mut messages = Vec::new();
    let outcome = FinalGpuOutcome::from_search(
        state,
        ceiling_detector,
        final_validation,
        iterations,
        sample_input_size,
        gpu_type,
        config,
    );
    outcome.append_summary_messages(&mut messages, encoder, config, psnr_ssim_mapper);
    (outcome.into_result(encoder), messages)
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
/// Performs GPU coarse search with detailed logging.
///
/// Implements the main GPU acceleration testing logic with comprehensive
/// logging and progress reporting. Tests multiple CRF values to find
/// the optimal GPU encoding settings.
///
/// # Arguments
/// * `input` - Input video file path
/// * `output` - Output directory path
/// * `encoder` - Encoder name to test
/// * `input_size` - Size of input file in bytes
/// * `config` - Coarse search configuration
/// * `vf_args` - Video filter arguments
/// * `progress_cb` - Optional progress callback
/// * `log_cb` - Optional logging callback
///
/// # Returns
/// GPU coarse search results with optimal settings
fn gpu_coarse_search_with_log_impl(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    vf_args: &[String],
    progress_cb: Option<&dyn Fn(f32, u64)>,
    log_cb: Option<&dyn Fn(&str)>,
) -> anyhow::Result<GpuCoarseResult> {
    let mut log = Vec::new();
    validate_gpu_coarse_config(config)?;

    // Always show GPU search logs in ultimate mode for transparency
    let silent_mode = progress_cb.is_some() && !config.ultimate_mode;

    let (preparation, setup_messages) = prepare_gpu_search(input, encoder, input_size, config)?;
    append_gpu_log_messages(&mut log, silent_mode, log_cb, &setup_messages);
    append_gpu_log_message(
        &mut log,
        silent_mode,
        log_cb,
        format!(
            "GPU: {} | Input: {:.2}MB | Duration: {:.1}s",
            match &preparation {
                GpuSearchPreparation::Ready(setup) => setup.gpu_type,
                GpuSearchPreparation::EarlyResult(result) => result.gpu_type,
            },
            crate::numeric_cast::u64_to_f64(input_size) / 1_024.0_f64 / 1_024.0_f64,
            match &preparation {
                GpuSearchPreparation::Ready(setup) => setup.duration,
                GpuSearchPreparation::EarlyResult(_) => 0.0,
            }
        ),
    );
    let setup = match preparation {
        GpuSearchPreparation::Ready(setup) => setup,
        GpuSearchPreparation::EarlyResult(mut result) => {
            result.log = log;
            return Ok(result);
        }
    };

    let mut iterations = 0u32;

    let encode_warmup = |crf: f32| {
        encode_gpu_warmup(
            input,
            output,
            &setup.gpu_encoder,
            setup.warmup_duration,
            crf,
        )
    };
    let warmup_input_size =
        calculate_gpu_warmup_input_size(input_size, setup.duration, setup.warmup_duration)?;

    let warmup_result = encode_warmup(config.max_crf)?;
    if warmup_result >= warmup_input_size {
        append_gpu_log_message(
            &mut log,
            silent_mode,
            log_cb,
            format!(
                "   {} Warmup: max_crf={:.0} cannot compress → skip GPU search",
                crate::media_conversion_gate::ui_icon_pick("⚡", "[FAST]"),
                config.max_crf
            ),
        );
        let mut result =
            base_gpu_coarse_result(setup.gpu_type, encoder, 1, log, setup.sample_input_size);
        result.gpu_boundary_crf = Some(config.max_crf);
        result.gpu_best_size = Some(warmup_result);
        return Ok(result);
    }
    append_gpu_log_message(
        &mut log,
        silent_mode,
        log_cb,
        format!(
            "   Warmup: max_crf={:.0} can compress → continue search",
            config.max_crf
        ),
    );

    append_gpu_log_messages(
        &mut log,
        silent_mode,
        log_cb,
        &sampling_mode_messages(setup.duration, config.ultimate_mode),
    );

    let mut encode_gpu = |crf: f32| {
        encode_gpu_sample(
            input,
            output,
            &setup.gpu_encoder,
            setup.duration,
            setup.actual_sample_duration,
            setup.sample_input_size,
            vf_args,
            config.ultimate_mode,
            progress_cb,
            crf,
        )
    };

    let encode_parallel = |crfs: &[f32]| {
        encode_gpu_parallel_probe(
            input,
            output,
            &setup.gpu_encoder,
            setup.actual_sample_duration,
            crfs,
        )
    };

    let mut size_cache: CrfCache<u64> = CrfCache::new();

    let (mut state, probe_messages) = execute_initial_gpu_probe(
        config,
        setup.sampling_plan,
        setup.sample_input_size,
        progress_cb,
        &mut size_cache,
        &mut iterations,
        &mut encode_gpu,
        &encode_parallel,
    )?;
    append_gpu_log_messages(&mut log, silent_mode, log_cb, &probe_messages);

    let mut encode_cached = |crf: f32, cache: &mut CrfCache<u64>| -> anyhow::Result<u64> {
        if let Some(&size) = cache.get(crf) {
            return Ok(size);
        }
        let size = encode_gpu(crf)?;
        cache.insert(crf, size);
        Ok(size)
    };

    append_gpu_log_messages(
        &mut log,
        silent_mode,
        log_cb,
        &run_gpu_stage1(
            &mut state,
            config,
            setup.sample_input_size,
            setup.max_iterations_limit,
            &mut iterations,
            progress_cb,
            &mut size_cache,
            &mut encode_cached,
        ),
    );
    append_gpu_log_messages(
        &mut log,
        silent_mode,
        log_cb,
        &run_gpu_stage2(
            &mut state,
            setup.sample_input_size,
            setup.max_iterations_limit,
            &mut iterations,
            progress_cb,
            &mut size_cache,
            &mut encode_cached,
        ),
    );

    let mut psnr_ssim_mapper = PsnrSsimMapper::new();
    let (ceiling_detector, stage3_messages) = run_gpu_stage3(
        &mut state,
        config,
        setup.sample_input_size,
        setup.max_iterations_limit,
        &mut iterations,
        progress_cb,
        &mut size_cache,
        &mut encode_cached,
        input,
        output,
    );
    append_gpu_log_messages(&mut log, silent_mode, log_cb, &stage3_messages);

    let mut final_encode_gpu = |crf: f32| {
        encode_gpu_sample(
            input,
            output,
            &setup.gpu_encoder,
            setup.duration,
            setup.actual_sample_duration,
            setup.sample_input_size,
            vf_args,
            config.ultimate_mode,
            progress_cb,
            crf,
        )
    };
    let final_validation = validate_final_gpu_quality(
        state.best_crf,
        input,
        output,
        &mut final_encode_gpu,
        &mut psnr_ssim_mapper,
        config.ultimate_mode,
    );
    append_gpu_log_messages(&mut log, silent_mode, log_cb, &final_validation.messages);
    let (mut result, final_messages) = finalize_gpu_search_result(
        &state,
        &ceiling_detector,
        &final_validation,
        &psnr_ssim_mapper,
        iterations,
        setup.sample_input_size,
        setup.gpu_type,
        encoder,
        config,
    );
    append_gpu_log_messages(&mut log, silent_mode, log_cb, &final_messages);
    if psnr_ssim_mapper.calibrated {
        append_gpu_log_message(
            &mut log,
            silent_mode,
            log_cb,
            "   ═══════════════════════════════════════════════════".to_string(),
        );
        psnr_ssim_mapper.print_report();
    }

    crate::media_conversion_gate::delivery_remove_file_or_audit(
        "gpu_coarse_final_temp_output",
        output,
    );

    result.log = log;
    Ok(result)
}

/// Derives the CPU search range from a GPU coarse search result.
#[must_use]
pub fn get_cpu_search_range_from_gpu(
    gpu_result: &GpuCoarseResult,
    original_min_crf: f32,
    original_max_crf: f32,
) -> (f32, f32, f32) {
    let Some(gpu_crf) = gpu_result.gpu_boundary_crf else {
        let center = f32::midpoint(original_min_crf, original_max_crf);
        return (original_min_crf, original_max_crf, center);
    };

    let mapping = match gpu_result.codec.as_str() {
        "av1" => CrfMapping::av1(gpu_result.gpu_type),
        _ => CrfMapping::hevc(gpu_result.gpu_type),
    };

    mapping.gpu_to_cpu_range(gpu_crf, original_min_crf, original_max_crf)
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
    #[serial_test::serial]
    fn calculate_quality_score_seals_non_finite_inputs() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL,
            "0",
        );
        let score = calculate_quality_score(f64::NAN, 1000, 2000, SearchPhase::Gpu);
        assert!(
            score.ssim.is_nan(),
            "non-finite SSIM must not be forged to 0.0 when exploration seal rejects"
        );
        assert!(score.compression_ratio.is_finite());
        assert!(score.combined_score.is_finite() || score.combined_score.is_nan());
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

        let args = encoder
            .get_crf_args(0.0)
            .expect("CRF args generation failed");
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

        let args = encoder
            .get_crf_args(51.0)
            .expect("CRF args generation failed");
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

        let args = encoder
            .get_crf_args(1.0)
            .expect("CRF args generation failed");
        assert_eq!(args, vec!["-q:v", "98"], "CRF 1 should map to q:v 98");

        let args = encoder
            .get_crf_args(25.0)
            .expect("CRF args generation failed");
        assert_eq!(args, vec!["-q:v", "50"], "CRF 25 should map to q:v 50");

        let args = encoder
            .get_crf_args(50.0)
            .expect("CRF args generation failed");
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
            let args = encoder
                .get_crf_args(crf)
                .expect("CRF args generation failed in test");
            let qv: f32 = args
                .get(1)
                .expect("Required string property missing")
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
Error while opening encoder - maybe incorrect parameters\n[hevc_videotoolbox @ 0x123] Cannot \
                      create compression session: -12908\nConversion failed!";

        assert_eq!(
            summarize_ffmpeg_failure_line(stderr),
            "[hevc_videotoolbox @ 0x123] Cannot create compression session: -12908"
        );
    }

    #[test]
    fn test_collect_vf_filters() {
        let args = vec![
            "-i".to_string(),
            "in.mp4".to_string(),
            "-vf".to_string(),
            "scale=1280:720".to_string(),
            "-c:v".to_string(),
            "libx264".to_string(),
            "-vf".to_string(),
            "fps=30".to_string(),
        ];
        let filters = collect_vf_filters(&args);
        assert_eq!(filters, vec!["scale=1280:720", "fps=30"]);
    }

    #[test]
    fn test_beijing_time_now() {
        let now = beijing_time_now();
        assert!(now.contains("(UTC+8)"));
        assert!(now.contains('-')); // Date part
        assert!(now.contains(':')); // Time part
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

#[cfg(test)]
mod advanced_tests {
    include!("../../tests/internal/gpu_behavior.rs");
}
