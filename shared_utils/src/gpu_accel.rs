//! GPU 加速模块 - 统一的硬件编码器检测和选择
//!
//! 🔥 v4.9: 为四个工具提供统一的 GPU 加速支持
//!
//! ## 支持的硬件编码器
//!
//! | 平台 | HEVC 编码器 | AV1 编码器 | H.264 编码器 |
//! |------|------------|-----------|--------------|
//! | NVIDIA | hevc_nvenc | av1_nvenc | h264_nvenc |
//! | Apple Silicon | hevc_videotoolbox | - | h264_videotoolbox |
//! | Intel QSV | hevc_qsv | av1_qsv | h264_qsv |
//! | AMD AMF | hevc_amf | av1_amf | h264_amf |
//! | VAAPI (Linux) | hevc_vaapi | av1_vaapi | h264_vaapi |
//!
//! ## 使用方式
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
use std::collections::VecDeque;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use crate::explore_strategy::CrfCache;


fn beijing_time_now() -> String {
    // UTC+8 (28800 seconds) is always a valid offset
    let beijing = FixedOffset::east_opt(8 * 3600).expect("UTC+8 is a valid fixed offset");
    let now: DateTime<Utc> = Utc::now();
    now.with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
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
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut buf) = lines.lock() {
                    if buf.len() >= max {
                        buf.pop_front();
                    }
                    buf.push_back(line);
                }
            }
        })
    }

    fn get_lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}


struct HeartbeatMonitor {
    last_activity: Arc<Mutex<std::time::Instant>>,
    stop_signal: Arc<AtomicBool>,
    child_pid: u32,
    timeout: std::time::Duration,
}

impl HeartbeatMonitor {
    fn new(
        last_activity: Arc<Mutex<std::time::Instant>>,
        stop_signal: Arc<AtomicBool>,
        child_pid: u32,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            last_activity,
            stop_signal,
            child_pid,
            timeout,
        }
    }

    fn spawn(self) -> JoinHandle<()> {
        std::thread::spawn(move || {
            const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

            loop {
                std::thread::sleep(CHECK_INTERVAL);

                if self.stop_signal.load(Ordering::Relaxed) {
                    break;
                }

                let elapsed = self.last_activity
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed();
                let elapsed_secs = elapsed.as_secs();

                crate::log_eprintln!(
                    "Heartbeat: {}s ago (Beijing: {})",
                    elapsed_secs,
                    beijing_time_now()
                );

                if elapsed > self.timeout {
                    crate::log_eprintln!(
                        "⚠️  FREEZE DETECTED: No activity for {} seconds!",
                        elapsed_secs
                    );
                    crate::log_eprintln!(
                        "   Terminating frozen ffmpeg process (PID: {})...",
                        self.child_pid
                    );

                    #[cfg(unix)]
                    unsafe {
                        libc::kill(self.child_pid as i32, libc::SIGKILL);
                    }

                    #[cfg(windows)]
                    {
                        let _ = std::process::Command::new("taskkill")
                            .args(&["/PID", &self.child_pid.to_string(), "/F"])
                            .output();
                    }

                    break;
                }
            }
        })
    }
}


pub const GPU_SAMPLE_DURATION: f32 = 50.0;

pub const GPU_SEGMENT_DURATION: f32 = 10.0;

pub const GPU_SAMPLE_SEGMENTS: usize = 5;

pub const GPU_COARSE_STEP: f32 = 2.0;

pub const GPU_ABSOLUTE_MAX_ITERATIONS: u32 = 750;

pub const GPU_MAX_ITERATIONS: u32 = GPU_ABSOLUTE_MAX_ITERATIONS;

pub const GPU_DEFAULT_MIN_CRF: f32 = 1.0;

pub const GPU_DEFAULT_MAX_CRF: f32 = 48.0;

static GPU_ACCEL: OnceLock<GpuAccel> = OnceLock::new();


pub fn derive_gpu_temp_extension(output: &std::path::Path) -> String {
    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    format!("gpu_temp.{}", ext)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuType {
    Nvidia,
    Apple,
    IntelQsv,
    AmdAmf,
    Vaapi,
    None,
}

impl std::fmt::Display for GpuType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuType::Nvidia => write!(f, "NVIDIA NVENC"),
            GpuType::Apple => write!(f, "Apple VideoToolbox"),
            GpuType::IntelQsv => write!(f, "Intel QSV"),
            GpuType::AmdAmf => write!(f, "AMD AMF"),
            GpuType::Vaapi => write!(f, "VA-API"),
            GpuType::None => write!(f, "None (CPU)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GpuEncoder {
    pub gpu_type: GpuType,
    pub name: &'static str,
    pub codec: &'static str,
    pub supports_crf: bool,
    pub crf_param: &'static str,
    pub crf_range: (u8, u8),
    pub extra_args: Vec<&'static str>,
}

impl GpuEncoder {
    pub fn ffmpeg_name(&self) -> &'static str {
        self.name
    }

    pub fn get_crf_args(&self, crf: f32) -> Vec<String> {
        if self.supports_crf {
            let quality_value = if self.gpu_type == GpuType::Apple {
                (100.0 - crf * 2.0).clamp(1.0, 100.0)
            } else {
                crf.clamp(self.crf_range.0 as f32, self.crf_range.1 as f32)
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

    pub fn get_extra_args(&self) -> Vec<&'static str> {
        self.extra_args.clone()
    }
}

#[derive(Debug, Clone)]
pub struct GpuAccel {
    pub gpu_type: GpuType,
    pub hevc_encoder: Option<GpuEncoder>,
    pub av1_encoder: Option<GpuEncoder>,
    pub h264_encoder: Option<GpuEncoder>,
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
    pub fn detect() -> &'static GpuAccel {
        GPU_ACCEL.get_or_init(|| {
            Self::detect_internal()
        })
    }

    pub fn detect_fresh() -> GpuAccel {
        Self::detect_internal()
    }

    pub fn print_detection_info(&self) {
        if !crate::progress_mode::is_verbose_mode() {
            if self.enabled {
                crate::log_eprintln!("GPU: {}", self.gpu_type);
            } else {
                crate::log_eprintln!("⚠️ No GPU acceleration, using CPU encoding");
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
        } else {
            crate::log_eprintln!("   ⚠️ No GPU acceleration available, using CPU encoding");
        }
    }

    fn detect_internal() -> GpuAccel {
        let encoders = get_available_encoders();


        #[cfg(target_os = "macos")]
        {
            if let Some(accel) = Self::try_videotoolbox(&encoders) {
                return accel;
            }
        }

        if let Some(accel) = Self::try_nvenc(&encoders) {
            return accel;
        }

        if let Some(accel) = Self::try_qsv(&encoders) {
            return accel;
        }

        #[cfg(target_os = "windows")]
        if let Some(accel) = Self::try_amf(&encoders) {
            return accel;
        }

        #[cfg(target_os = "linux")]
        if let Some(accel) = Self::try_vaapi(&encoders) {
            return accel;
        }

        GpuAccel::default()
    }

    fn try_videotoolbox(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_videotoolbox"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_videotoolbox"));

        if !has_hevc && !has_h264 {
            return None;
        }

        if has_hevc && !test_encoder("hevc_videotoolbox") {
            return None;
        }

        Some(GpuAccel {
            gpu_type: GpuType::Apple,
            hevc_encoder: if has_hevc {
                Some(GpuEncoder {
                    gpu_type: GpuType::Apple,
                    name: "hevc_videotoolbox",
                    codec: "hevc",
                    supports_crf: true,
                    crf_param: "q:v",
                    crf_range: (0, 100),
                    extra_args: vec![
                        "-profile:v",
                        "main",
                        "-tag:v",
                        "hvc1",
                    ],
                })
            } else {
                None
            },
            av1_encoder: None,
            h264_encoder: if has_h264 {
                Some(GpuEncoder {
                    gpu_type: GpuType::Apple,
                    name: "h264_videotoolbox",
                    codec: "h264",
                    supports_crf: true,
                    crf_param: "q:v",
                    crf_range: (0, 100),
                    extra_args: vec!["-profile:v", "high"],
                })
            } else {
                None
            },
            enabled: true,
        })
    }

    fn try_nvenc(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_nvenc"));
        let has_av1 = encoders.iter().any(|e| e.contains("av1_nvenc"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_nvenc"));

        if !has_hevc && !has_av1 && !has_h264 {
            return None;
        }

        if has_hevc && !test_encoder("hevc_nvenc") {
            return None;
        }

        Some(GpuAccel {
            gpu_type: GpuType::Nvidia,
            hevc_encoder: if has_hevc {
                Some(GpuEncoder {
                    gpu_type: GpuType::Nvidia,
                    name: "hevc_nvenc",
                    codec: "hevc",
                    supports_crf: true,
                    crf_param: "cq",
                    crf_range: (0, 51),
                    extra_args: vec![
                        "-preset",
                        "p4",
                        "-tune",
                        "hq",
                        "-rc",
                        "vbr",
                        "-profile:v",
                        "main",
                    ],
                })
            } else {
                None
            },
            av1_encoder: if has_av1 {
                Some(GpuEncoder {
                    gpu_type: GpuType::Nvidia,
                    name: "av1_nvenc",
                    codec: "av1",
                    supports_crf: true,
                    crf_param: "cq",
                    crf_range: (0, 63),
                    extra_args: vec!["-preset", "p4", "-tune", "hq", "-rc", "vbr"],
                })
            } else {
                None
            },
            h264_encoder: if has_h264 {
                Some(GpuEncoder {
                    gpu_type: GpuType::Nvidia,
                    name: "h264_nvenc",
                    codec: "h264",
                    supports_crf: true,
                    crf_param: "cq",
                    crf_range: (0, 51),
                    extra_args: vec![
                        "-preset",
                        "p4",
                        "-tune",
                        "hq",
                        "-rc",
                        "vbr",
                        "-profile:v",
                        "high",
                    ],
                })
            } else {
                None
            },
            enabled: true,
        })
    }

    fn try_qsv(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_qsv"));
        let has_av1 = encoders.iter().any(|e| e.contains("av1_qsv"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_qsv"));

        if !has_hevc && !has_av1 && !has_h264 {
            return None;
        }

        if has_hevc && !test_encoder("hevc_qsv") {
            return None;
        }

        Some(GpuAccel {
            gpu_type: GpuType::IntelQsv,
            hevc_encoder: if has_hevc {
                Some(GpuEncoder {
                    gpu_type: GpuType::IntelQsv,
                    name: "hevc_qsv",
                    codec: "hevc",
                    supports_crf: true,
                    crf_param: "global_quality",
                    crf_range: (1, 51),
                    extra_args: vec!["-preset", "medium", "-profile:v", "main"],
                })
            } else {
                None
            },
            av1_encoder: if has_av1 {
                Some(GpuEncoder {
                    gpu_type: GpuType::IntelQsv,
                    name: "av1_qsv",
                    codec: "av1",
                    supports_crf: true,
                    crf_param: "global_quality",
                    crf_range: (1, 63),
                    extra_args: vec!["-preset", "medium"],
                })
            } else {
                None
            },
            h264_encoder: if has_h264 {
                Some(GpuEncoder {
                    gpu_type: GpuType::IntelQsv,
                    name: "h264_qsv",
                    codec: "h264",
                    supports_crf: true,
                    crf_param: "global_quality",
                    crf_range: (1, 51),
                    extra_args: vec!["-preset", "medium", "-profile:v", "high"],
                })
            } else {
                None
            },
            enabled: true,
        })
    }

    #[cfg(target_os = "windows")]
    fn try_amf(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_amf"));
        let has_av1 = encoders.iter().any(|e| e.contains("av1_amf"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_amf"));

        if !has_hevc && !has_av1 && !has_h264 {
            return None;
        }

        if has_hevc && !test_encoder("hevc_amf") {
            return None;
        }

        Some(GpuAccel {
            gpu_type: GpuType::AmdAmf,
            hevc_encoder: if has_hevc {
                Some(GpuEncoder {
                    gpu_type: GpuType::AmdAmf,
                    name: "hevc_amf",
                    codec: "hevc",
                    supports_crf: true,
                    crf_param: "qp_i",
                    crf_range: (0, 51),
                    extra_args: vec!["-quality", "quality", "-profile:v", "main"],
                })
            } else {
                None
            },
            av1_encoder: if has_av1 {
                Some(GpuEncoder {
                    gpu_type: GpuType::AmdAmf,
                    name: "av1_amf",
                    codec: "av1",
                    supports_crf: true,
                    crf_param: "qp_i",
                    crf_range: (0, 63),
                    extra_args: vec!["-quality", "quality"],
                })
            } else {
                None
            },
            h264_encoder: if has_h264 {
                Some(GpuEncoder {
                    gpu_type: GpuType::AmdAmf,
                    name: "h264_amf",
                    codec: "h264",
                    supports_crf: true,
                    crf_param: "qp_i",
                    crf_range: (0, 51),
                    extra_args: vec!["-quality", "quality", "-profile:v", "high"],
                })
            } else {
                None
            },
            enabled: true,
        })
    }

    #[cfg(target_os = "linux")]
    fn try_vaapi(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_vaapi"));
        let has_av1 = encoders.iter().any(|e| e.contains("av1_vaapi"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_vaapi"));

        if !has_hevc && !has_av1 && !has_h264 {
            return None;
        }

        if has_hevc && !test_encoder("hevc_vaapi") {
            return None;
        }

        Some(GpuAccel {
            gpu_type: GpuType::Vaapi,
            hevc_encoder: if has_hevc {
                Some(GpuEncoder {
                    gpu_type: GpuType::Vaapi,
                    name: "hevc_vaapi",
                    codec: "hevc",
                    supports_crf: true,
                    crf_param: "qp",
                    crf_range: (0, 52),
                    extra_args: vec!["-vaapi_device", "/dev/dri/renderD128", "-profile:v", "main"],
                })
            } else {
                None
            },
            av1_encoder: if has_av1 {
                Some(GpuEncoder {
                    gpu_type: GpuType::Vaapi,
                    name: "av1_vaapi",
                    codec: "av1",
                    supports_crf: true,
                    crf_param: "qp",
                    crf_range: (0, 63),
                    extra_args: vec!["-vaapi_device", "/dev/dri/renderD128"],
                })
            } else {
                None
            },
            h264_encoder: if has_h264 {
                Some(GpuEncoder {
                    gpu_type: GpuType::Vaapi,
                    name: "h264_vaapi",
                    codec: "h264",
                    supports_crf: true,
                    crf_param: "qp",
                    crf_range: (0, 52),
                    extra_args: vec!["-vaapi_device", "/dev/dri/renderD128", "-profile:v", "high"],
                })
            } else {
                None
            },
            enabled: true,
        })
    }

    pub fn get_hevc_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.hevc_encoder.as_ref()
        } else {
            None
        }
    }

    pub fn get_av1_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.av1_encoder.as_ref()
        } else {
            None
        }
    }

    pub fn get_h264_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.h264_encoder.as_ref()
        } else {
            None
        }
    }

    pub fn is_available(&self) -> bool {
        self.enabled
    }

    pub fn description(&self) -> String {
        if self.enabled {
            format!("{} (Hardware Accelerated)", self.gpu_type)
        } else {
            "CPU (Software Encoding)".to_string()
        }
    }
}

fn get_available_encoders() -> Vec<String> {
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-encoders")
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .filter(|line| line.starts_with(" V"))
                .map(|line| line.to_string())
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

fn test_encoder(encoder: &str) -> bool {
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg("nullsrc=s=64x64:d=0.1")
        .arg("-c:v")
        .arg(encoder)
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();

    match output {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

fn crf_to_estimated_bitrate(crf: f32, codec: &str) -> u32 {
    let base_bitrate = match codec {
        "hevc" => 5000,
        "av1" => 4000,
        "h264" => 8000,
        _ => 5000,
    };

    let crf_factor = match codec {
        "hevc" | "h264" => 0.9_f32.powf((crf - 23.0) / 6.0),
        "av1" => 0.9_f32.powf((crf - 30.0) / 6.0),
        _ => 1.0,
    };

    (base_bitrate as f32 * crf_factor) as u32
}


#[derive(Debug, Clone)]
pub struct SmartSampleResult {
    pub sample_filter: String,
    pub actual_duration: f32,
    pub strategy: String,
}

pub fn calculate_smart_sample(
    input: &std::path::Path,
    total_duration: f32,
    target_sample_duration: f32,
) -> anyhow::Result<SmartSampleResult> {
    use anyhow::Context;
    use std::process::Command;

    if total_duration <= target_sample_duration * 1.2 {
        return Ok(SmartSampleResult {
            sample_filter: String::new(),
            actual_duration: total_duration,
            strategy: format!(
                "Full video ({:.1}s, close to target {:.1}s)",
                total_duration, target_sample_duration
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
        format!(
            "gt(scene,{})+gt(entropy,{})",
            scene_threshold, entropy_threshold
        )
    } else {
        format!(
            "gt(scene,{})*gt(entropy,{})",
            scene_threshold * 1.5,
            entropy_threshold * 1.2
        )
    };

    let test_output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-t")
        .arg("10")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-vf")
        .arg(format!("select='{}',showinfo", select_expr))
        .arg("-f")
        .arg("null")
        .arg("-")
        .output()
        .context("Failed to test smart sample filter")?;

    let stderr = String::from_utf8_lossy(&test_output.stderr);
    let frame_count = stderr.matches("n:").count();

    if frame_count == 0 {
        return Ok(SmartSampleResult {
            sample_filter: String::new(),
            actual_duration: target_sample_duration,
            strategy: format!(
                "Uniform sampling ({:.1}s, {:.1}%)",
                target_sample_duration, sample_percentage
            ),
        });
    }

    Ok(SmartSampleResult {
        sample_filter: format!("select='{}',setpts=N/FRAME_RATE/TB", select_expr),
        actual_duration: target_sample_duration,
        strategy: format!(
            "Smart sampling ({:.1}s, {:.1}%, scene+entropy)",
            target_sample_duration, sample_percentage
        ),
    })
}


#[derive(Debug, Clone, Copy)]
pub struct QualityScore {
    pub ssim: f64,
    pub compression_ratio: f64,
    pub combined_score: f64,
}

impl QualityScore {

    #[inline]
    pub fn ssim_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.ssim).ok()
    }

    #[inline]
    pub fn ssim_meets(&self, threshold: f64) -> bool {
        crate::float_compare::ssim_meets_threshold(self.ssim, threshold)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPhase {
    Gpu,
    Cpu,
}

pub fn calculate_quality_score(
    ssim: f64,
    output_size: u64,
    input_size: u64,
    phase: SearchPhase,
) -> QualityScore {
    let compression_ratio = if input_size == 0 {
        1.0
    } else {
        output_size as f64 / input_size as f64
    };

    let (ssim_weight, size_weight) = match phase {
        SearchPhase::Gpu => (0.4, 0.6),
        SearchPhase::Cpu => (0.7, 0.3),
    };

    let size_score = (1.0 - compression_ratio).max(0.0);
    let combined_score = ssim_weight * ssim + size_weight * size_score;

    QualityScore {
        ssim,
        compression_ratio,
        combined_score,
    }
}

pub fn is_quality_better(
    new_score: &QualityScore,
    old_score: &QualityScore,
    min_ssim_threshold: f64,
) -> bool {
    if new_score.ssim < min_ssim_threshold {
        return false;
    }

    let improvement =
        (new_score.combined_score - old_score.combined_score) / old_score.combined_score;
    improvement > 0.005
}


pub fn estimate_cpu_search_center_dynamic(
    gpu_boundary: f32,
    gpu_type: GpuType,
    _codec: &str,
    compression_potential: Option<f64>,
) -> f32 {
    let base_offset = match gpu_type {
        GpuType::Apple => 5.0,
        GpuType::Nvidia => 4.0,
        GpuType::IntelQsv => 3.5,
        GpuType::AmdAmf => 5.0,
        GpuType::Vaapi => 4.0,
        GpuType::None => 0.0,
    };

    let adjustment = if let Some(potential) = compression_potential {
        if potential < 0.3 {
            0.3
        } else if potential > 0.7 {
            -0.2
        } else {
            0.0
        }
    } else {
        0.0
    };

    gpu_boundary + base_offset + adjustment
}

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

pub fn estimate_cpu_search_center(gpu_boundary: f32, gpu_type: GpuType, codec: &str) -> f32 {
    estimate_cpu_search_center_dynamic(gpu_boundary, gpu_type, codec, None)
}

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

#[deprecated(since = "5.0.1", note = "use estimate_cpu_search_center instead")]
pub fn gpu_to_cpu_crf(gpu_crf: f32, gpu_type: GpuType, codec: &str) -> f32 {
    estimate_cpu_search_center(gpu_crf, gpu_type, codec)
}


#[derive(Debug, Clone)]
pub struct GpuCoarseResult {
    pub gpu_boundary_crf: f32,
    pub gpu_best_size: Option<u64>,
    pub gpu_best_ssim: Option<f64>,
    pub gpu_type: GpuType,
    pub codec: String,
    pub iterations: u32,
    pub found_boundary: bool,
    pub fine_tuned: bool,
    pub log: Vec<String>,
    pub sample_input_size: u64,
    pub quality_ceiling_crf: Option<f32>,
    pub quality_ceiling_ssim: Option<f64>,
}

impl GpuCoarseResult {

    #[inline]
    pub fn best_ssim_typed(&self) -> Option<crate::types::Ssim> {
        self.gpu_best_ssim
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

    #[inline]
    pub fn ceiling_ssim_typed(&self) -> Option<crate::types::Ssim> {
        self.quality_ceiling_ssim
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

    #[inline]
    pub fn best_size_typed(&self) -> Option<crate::types::FileSize> {
        self.gpu_best_size.map(crate::types::FileSize::new)
    }
}

#[derive(Debug, Clone)]
pub struct CrfMapping {
    pub gpu_type: GpuType,
    pub codec: &'static str,
    pub offset: f32,
    pub uncertainty: f32,
}

impl CrfMapping {
    pub fn hevc(gpu_type: GpuType) -> Self {
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

    pub fn av1(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple => (0.0, 0.0),
            GpuType::Nvidia => (3.8, 0.4),
            GpuType::IntelQsv => (3.5, 0.3),
            GpuType::AmdAmf => (4.5, 0.5),
            GpuType::Vaapi => (3.8, 0.4),
            GpuType::None => (0.0, 0.0),
        };
        Self {
            gpu_type,
            codec: "av1",
            offset,
            uncertainty,
        }
    }

    pub fn gpu_to_cpu_range(&self, gpu_crf: f32, min_crf: f32, max_crf: f32) -> (f32, f32, f32) {
        let center = (gpu_crf + self.offset).min(max_crf);
        let low = gpu_crf.max(min_crf);
        let high = (center + self.uncertainty).min(max_crf);
        (center, low, high)
    }

    pub fn cpu_to_gpu(&self, cpu_crf: f32) -> f32 {
        cpu_crf - self.offset
    }

    pub fn print_mapping_info(&self) {
        crate::log_eprintln!(
            "   📊 GPU/CPU CRF Mapping ({} - {}):",
            self.gpu_type,
            self.codec.to_uppercase()
        );
        if self.gpu_type == GpuType::Apple {
            crate::log_eprintln!("      • VideoToolbox q:v: 1=lowest, 100=highest quality");
            crate::log_eprintln!("      • SSIM ceiling: 0.91~0.97 (content-dependent, cannot reach 0.98+)");
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

#[derive(Debug, Clone)]
pub struct GpuCoarseConfig {
    pub initial_crf: f32,
    pub min_crf: f32,
    pub max_crf: f32,
    pub step: f32,
    pub max_iterations: u32,
}

impl Default for GpuCoarseConfig {
    fn default() -> Self {
        Self {
            initial_crf: 18.0,
            min_crf: GPU_DEFAULT_MIN_CRF,
            max_crf: GPU_DEFAULT_MAX_CRF,
            step: GPU_COARSE_STEP,
            max_iterations: GPU_MAX_ITERATIONS,
        }
    }
}


fn calculate_psnr_fast(input: &str, output: &str) -> Result<f64, String> {
    let psnr_output = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(std::path::Path::new(input)).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(std::path::Path::new(output)).as_ref())
        .arg("-lavfi")
        .arg("psnr")
        .arg("-f")
        .arg("null")
        .arg("-")
        .output()
        .map_err(|e| format!("PSNR calculation failed: {}", e))?;

    let stderr = String::from_utf8_lossy(&psnr_output.stderr);

    for line in stderr.lines() {
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
    }

    Err("Failed to parse PSNR from ffmpeg output".to_string())
}


#[derive(Debug)]
struct QualityCeilingDetector {
    samples: Vec<(f32, f64)>,
    plateau_threshold: f64,
    plateau_count: usize,
    ceiling_detected: bool,
}

impl QualityCeilingDetector {
    fn new() -> Self {
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
            let last = self.samples[self.samples.len() - 1].1;
            let prev = self.samples[self.samples.len() - 2].1;
            let improvement = last - prev;

            if improvement < self.plateau_threshold {
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
    fn new() -> Self {
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

        for i in 0..points.len() - 1 {
            let (psnr1, ssim1) = points[i];
            let (psnr2, ssim2) = points[i + 1];

            if psnr >= psnr1 && psnr <= psnr2 {
                let denom = psnr2 - psnr1;
                if denom.abs() < f64::EPSILON {
                    return Some((ssim1 + ssim2) / 2.0);
                }
                let ratio = (psnr - psnr1) / denom;
                let predicted_ssim = ssim1 + ratio * (ssim2 - ssim1);
                return Some(predicted_ssim);
            }
        }

        if psnr < points[0].0 {
            let (psnr1, ssim1) = points[0];
            let (psnr2, ssim2) = points[1];
            let denom = psnr2 - psnr1;
            if denom.abs() < f64::EPSILON {
                return Some(ssim1);
            }
            let slope = (ssim2 - ssim1) / denom;
            Some(ssim1 + slope * (psnr - psnr1))
        } else {
            let n = points.len();
            let (psnr1, ssim1) = points[n - 2];
            let (psnr2, ssim2) = points[n - 1];
            let denom = psnr2 - psnr1;
            if denom.abs() < f64::EPSILON {
                return Some(ssim2);
            }
            let slope = (ssim2 - ssim1) / denom;
            Some(ssim2 + slope * (psnr - psnr2))
        }
    }

    fn get_mapping_quality(&self) -> f64 {
        if self.calibration_points.len() < 3 {
            return 0.5;
        }

        let n = self.calibration_points.len() as f64;
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

pub fn gpu_coarse_search(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    progress_cb: Option<&dyn Fn(f32, u64)>,
) -> anyhow::Result<GpuCoarseResult> {
    gpu_coarse_search_with_log(
        input,
        output,
        encoder,
        input_size,
        config,
        progress_cb,
        None,
    )
}

pub fn gpu_coarse_search_with_log(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,
    input_size: u64,
    config: &GpuCoarseConfig,
    progress_cb: Option<&dyn Fn(f32, u64)>,
    log_cb: Option<&dyn Fn(&str)>,
) -> anyhow::Result<GpuCoarseResult> {
    use anyhow::{bail, Context};
    use std::process::Command;

    let mut log = Vec::new();

    let silent_mode = progress_cb.is_some();

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

    let gpu = GpuAccel::detect();

    if !gpu.is_available() {
        log_msg!("   ╔═══════════════════════════════════════════════════════════╗");
        log_msg!("   ║  ⚠️  FALLBACK: No GPU available!                          ║");
        log_msg!("   ║  Skipping GPU coarse search, using CPU-only mode          ║");
        log_msg!("   ║  This may take longer but results will be accurate        ║");
        log_msg!("   ╚═══════════════════════════════════════════════════════════╝");
        return Ok(GpuCoarseResult {
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
        });
    }

    let gpu_encoder = match encoder {
        "hevc" => gpu.get_hevc_encoder(),
        "av1" => gpu.get_av1_encoder(),
        "h264" => gpu.get_h264_encoder(),
        _ => None,
    };

    let gpu_encoder = match gpu_encoder {
        Some(enc) => enc,
        None => {
            log_msg!("   ╔═══════════════════════════════════════════════════════════╗");
            log_msg!(
                "   ║  ⚠️  FALLBACK: No GPU encoder for {}!              ║",
                encoder.to_uppercase()
            );
            log_msg!("   ║  Skipping GPU coarse search, using CPU-only mode          ║");
            log_msg!("   ║  This may take longer but results will be accurate        ║");
            log_msg!("   ╚═══════════════════════════════════════════════════════════╝");
            return Ok(GpuCoarseResult {
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
            });
        }
    };


    const SKIP_GPU_SIZE_THRESHOLD: u64 = 500 * 1024;
    const SKIP_GPU_DURATION_THRESHOLD: f32 = 3.0;

    const LARGE_FILE_THRESHOLD: u64 = 500 * 1024 * 1024;
    const VERY_LARGE_FILE_THRESHOLD: u64 = 2 * 1024 * 1024 * 1024;
    const LONG_DURATION_THRESHOLD: f32 = 600.0;
    const VERY_LONG_DURATION_THRESHOLD: f32 = 3600.0;

    let quick_duration: f32 = {
        let duration_output = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                "--",
            ])
            .arg(crate::safe_path_arg(input).as_ref())
            .output();

        duration_output
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(GPU_SAMPLE_DURATION)
    };

    let skip_gpu =
        input_size < SKIP_GPU_SIZE_THRESHOLD || quick_duration < SKIP_GPU_DURATION_THRESHOLD;

    if skip_gpu {
        let reason = if input_size < SKIP_GPU_SIZE_THRESHOLD {
            format!(
                "file too small ({:.1}KB < 500KB)",
                input_size as f64 / 1024.0
            )
        } else {
            format!("duration too short ({:.1}s < 3s)", quick_duration)
        };
        log_msg!("   ⚡ Skip GPU: {} → CPU-only mode", reason);
        return Ok(GpuCoarseResult {
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
        });
    }

    let is_large_file = input_size >= LARGE_FILE_THRESHOLD;
    let is_very_large_file = input_size >= VERY_LARGE_FILE_THRESHOLD;
    let is_long_video = quick_duration >= LONG_DURATION_THRESHOLD;
    let is_very_long_video = quick_duration >= VERY_LONG_DURATION_THRESHOLD;

    let (sample_duration_limit, skip_parallel) = if is_very_large_file || is_very_long_video {
        log_msg!("   ⚠️ Very large file detected → Conservative mode (30s sample)");
        (30.0_f32, true)
    } else if is_large_file || is_long_video {
        log_msg!("   📊 Large file detected → Sequential mode (45s sample)");
        (45.0_f32, true)
    } else {
        log_msg!(
            "   ✅ Normal file → Sequential mode ({}s sample)",
            GPU_SAMPLE_DURATION
        );
        (GPU_SAMPLE_DURATION, true)
    };

    let max_iterations_limit = GPU_ABSOLUTE_MAX_ITERATIONS;

    log_msg!(
        "GPU搜索 ({}, {:.2}MB, {:.1}s)",
        gpu.gpu_type,
        input_size as f64 / 1024.0 / 1024.0,
        quick_duration
    );
    log.push(format!(
        "GPU: {} | Input: {:.2}MB | Duration: {:.1}s",
        gpu.gpu_type,
        input_size as f64 / 1024.0 / 1024.0,
        quick_duration
    ));

    let mut iterations = 0u32;

    let duration = quick_duration;
    let actual_sample_duration = duration.min(sample_duration_limit);

    let sample_input_size = if duration < 60.0 {
        input_size
    } else {
        let multi_segment_duration = GPU_SAMPLE_DURATION;
        let ratio = multi_segment_duration / duration;
        (input_size as f64 * ratio as f64) as u64
    };

    const WARMUP_DURATION: f32 = 5.0;
    let warmup_duration = duration.min(WARMUP_DURATION);

    let encode_warmup = |crf: f32| -> anyhow::Result<u64> {
        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.get_extra_args();
        let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
        let warmup_output = output.with_extension(format!("warmup.{}", ext));

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-t")
            .arg(format!("{}", warmup_duration))
            .arg("-i")
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-c:v")
            .arg(gpu_encoder.name);

        for arg in &crf_args {
            cmd.arg(arg);
        }
        for arg in &extra_args {
            cmd.arg(*arg);
        }

        cmd.arg("-an")
            .arg(crate::safe_path_arg(&warmup_output).as_ref());

        let result = cmd.output().context("Failed to run warmup encode")?;
        let size = if result.status.success() {
            std::fs::metadata(&warmup_output)
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };
        let _ = std::fs::remove_file(&warmup_output);
        Ok(size)
    };

    let warmup_input_size = if duration <= WARMUP_DURATION || duration == 0.0 {
        input_size
    } else {
        (input_size as f64 * warmup_duration as f64 / duration as f64) as u64
    };

    let warmup_result = encode_warmup(config.max_crf);
    let can_compress_at_max = match &warmup_result {
        Ok(size) => *size < warmup_input_size,
        Err(_) => true,
    };

    if !can_compress_at_max {
        log_msg!(
            "   ⚡ Warmup: max_crf={:.0} cannot compress → skip GPU search",
            config.max_crf
        );
        return Ok(GpuCoarseResult {
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
        });
    }
    log_msg!(
        "   🔥 Warmup: max_crf={:.0} can compress → continue search",
        config.max_crf
    );

    if duration >= 60.0 {
        log_msg!("   📊 Multi-segment sampling: 5 segments × 10s = 50s (0%, 25%, 50%, 75%, 90%)");
    } else {
        log_msg!("   📊 Full video sampling: {:.1}s", duration);
    }

    let encode_gpu = |crf: f32| -> anyhow::Result<u64> {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.get_extra_args();

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y");

        let use_multi_segment = duration >= 60.0;

        if !use_multi_segment {
            cmd.arg("-t").arg(format!("{}", actual_sample_duration));
        }

        cmd.arg("-i")
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-c:v")
            .arg(gpu_encoder.name);

        if use_multi_segment {
            let seg_dur = GPU_SEGMENT_DURATION;
            let positions = [
                0.0,
                duration * 0.25,
                duration * 0.50,
                duration * 0.75,
                (duration * 0.90).max(duration - seg_dur),
            ];

            let select_expr: Vec<String> = positions
                .iter()
                .map(|&pos| format!("between(t,{:.1},{:.1})", pos, pos + seg_dur))
                .collect();
            let select_filter =
                format!("select='{}',setpts=N/FRAME_RATE/TB", select_expr.join("+"));

            cmd.arg("-vf").arg(&select_filter);
        }

        for arg in &crf_args {
            cmd.arg(arg);
        }
        for arg in &extra_args {
            cmd.arg(*arg);
        }

        cmd.arg("-an")
            .arg("-progress")
            .arg("pipe:1")
            .arg(crate::safe_path_arg(output).as_ref())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;
        let start_time = Instant::now();
        let absolute_timeout = Duration::from_secs(12 * 3600);
        let child_pid = child.id();

        let stderr_capture = StderrCapture::new(100);
        let stderr_handle = child
            .stderr
            .take()
            .map(|stderr| stderr_capture.spawn_capture_thread(stderr));

        let last_activity = Arc::new(Mutex::new(Instant::now()));
        let stop_signal = Arc::new(AtomicBool::new(false));
        let heartbeat = HeartbeatMonitor::new(
            Arc::clone(&last_activity),
            Arc::clone(&stop_signal),
            child_pid,
            Duration::from_secs(300),
        );
        let heartbeat_handle = heartbeat.spawn();

        let first_output = Arc::new(AtomicBool::new(false));
        let first_output_clone = Arc::clone(&first_output);
        let stop_clone = Arc::clone(&stop_signal);
        let startup_handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            if !first_output_clone.load(Ordering::Relaxed) && !stop_clone.load(Ordering::Relaxed) {
                crate::log_eprintln!(
                    "❌ STARTUP FAILED: No output in 30s (Beijing: {})",
                    beijing_time_now()
                );
                #[cfg(unix)]
                unsafe {
                    libc::kill(child_pid as i32, libc::SIGKILL);
                }
            }
        });

        crate::verbose_eprintln!(
            "GPU encoding started - Beijing: {}",
            beijing_time_now()
        );

        let mut last_progress_time = Instant::now();
        let mut fallback_logged = false;

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);

            for line in reader.lines() {
                if !first_output.load(Ordering::Relaxed) {
                    first_output.store(true, Ordering::Relaxed);
                }

                if let Ok(mut guard) = last_activity.lock() {
                    *guard = Instant::now();
                }

                if let Ok(line) = line {
                    if let Some(val) = line.strip_prefix("out_time_us=") {
                        if let Ok(time_us) = val.parse::<u64>() {
                            if last_progress_time.elapsed().as_secs_f64() >= 1.0 {
                                let current_secs = time_us as f64 / 1_000_000.0;
                                let pct = (current_secs / actual_sample_duration as f64 * 100.0)
                                    .min(100.0);
                                let elapsed_secs = start_time.elapsed().as_secs_f64();
                                let eta = if pct > 0.1 && current_secs > 0.0 && elapsed_secs > 0.0 {
                                    let speed = current_secs / elapsed_secs;
                                    if speed > 0.0 {
                                        ((actual_sample_duration as f64 - current_secs) / speed)
                                            .max(0.0) as u64
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

                                let estimated_final_size = match std::fs::metadata(output) {
                                    Ok(metadata) => {
                                        let current_size = metadata.len();
                                        fallback_logged = false;
                                        (current_size as f64 / pct.max(1.0) * 100.0) as u64
                                    }
                                    Err(_) => {
                                        if !fallback_logged {
                        crate::log_eprintln!(
                                "Using linear estimation (metadata unavailable)"
                            );
                                            fallback_logged = true;
                                        }
                                        (sample_input_size as f64 * (1.0 / pct.max(0.1)))
                                            .min(sample_input_size as f64 * 10.0)
                                            as u64
                                    }
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
            }
        }

        let status = child.wait().context("Failed to wait for ffmpeg")?;

        stop_signal.store(true, Ordering::Relaxed);
        let _ = heartbeat_handle.join();
        let _ = startup_handle.join();
        if let Some(handle) = stderr_handle {
            let _ = handle.join();
        }

        if start_time.elapsed() > absolute_timeout {
            crate::log_eprintln!("⏰ WARNING: GPU encoding took longer than 12 hours!");
            bail!("GPU encoding exceeded 12-hour timeout");
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

        crate::verbose_eprintln!(
            "Encoding completed - Beijing: {}",
            beijing_time_now()
        );

        Ok(std::fs::metadata(output)?.len())
    };

    let encode_parallel = |crfs: &[f32]| -> Vec<(f32, anyhow::Result<u64>)> {
        use std::thread;

        let handles: Vec<_> = crfs
            .iter()
            .enumerate()
            .map(|(i, &crf)| {
                let crf_args = gpu_encoder.get_crf_args(crf);
                let extra_args: Vec<String> = gpu_encoder
                    .get_extra_args()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let input_path = input.to_path_buf();
                let output_path = output.with_extension(format!("tmp{}.mp4", i));
                let encoder_name = gpu_encoder.name.to_string();
                let sample_dur = actual_sample_duration;

                thread::spawn(move || {
                    let mut cmd = Command::new("ffmpeg");
                    cmd.arg("-y")
                        .arg("-t")
                        .arg(format!("{}", sample_dur))
                        .arg("-i")
                        .arg(crate::safe_path_arg(&input_path).as_ref())
                        .arg("-c:v")
                        .arg(&encoder_name);

                    for arg in &crf_args {
                        cmd.arg(arg);
                    }
                    for arg in &extra_args {
                        cmd.arg(arg);
                    }

                    cmd.arg("-an")
                        .arg(crate::safe_path_arg(&output_path).as_ref());

                    let result = cmd.output();

                    let size = match result {
                        Ok(out) if out.status.success() => std::fs::metadata(&output_path)
                            .map(|m| m.len())
                            .map_err(|e| anyhow::anyhow!("{}", e)),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            Err(anyhow::anyhow!(
                                "GPU encoding failed: {}",
                                stderr.lines().last().unwrap_or("unknown")
                            ))
                        }
                        Err(e) => Err(anyhow::anyhow!("{}", e)),
                    };

                    let _ = std::fs::remove_file(&output_path);

                    (crf, size)
                })
            })
            .collect();

        handles
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


    const WINDOW_SIZE: usize = 3;
    const _VARIANCE_THRESHOLD: f64 = 0.0001;
    const CHANGE_RATE_THRESHOLD: f64 = 0.02;

    let mut size_history: Vec<(f32, u64)> = Vec::new();

    let _calc_window_variance = |history: &[(f32, u64)], input_size: u64| -> f64 {
        if history.len() < WINDOW_SIZE {
            return f64::MAX;
        }
        let recent: Vec<f64> = history
            .iter()
            .rev()
            .take(WINDOW_SIZE)
            .map(|(_, s)| *s as f64 / input_size as f64)
            .collect();
        let mean = recent.iter().sum::<f64>() / recent.len() as f64;
        recent.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / recent.len() as f64
    };

    let calc_change_rate = |prev: u64, curr: u64| -> f64 {
        if prev == 0 {
            return f64::MAX;
        }
        ((curr as f64 - prev as f64) / prev as f64).abs()
    };

    let mut boundary_low: f32 = config.min_crf;
    let mut boundary_high: f32 = config.max_crf;
    let mut prev_size: Option<u64> = None;
    let mut found_compress_point = false;

    let use_initial =
        config.initial_crf >= config.min_crf + 5.0 && config.initial_crf <= config.max_crf - 5.0;

    let probe_crfs = if use_initial {
        log_msg!(
            "   🎯 Using initial_crf {:.0} as search anchor",
            config.initial_crf
        );
        vec![config.initial_crf, config.max_crf, config.min_crf]
    } else {
        let mid_crf = (config.min_crf + config.max_crf) / 2.0;
        log_msg!(
            "   ⚠️ initial_crf {:.0} out of range, using mid_crf {:.0}",
            config.initial_crf,
            mid_crf
        );
        vec![mid_crf, config.max_crf, config.min_crf]
    };

    let probe_results = if skip_parallel {
        log_msg!("   ⚡ Skip parallel probe (large file mode)");
        let test_crf = probe_crfs[0];
        log_msg!("   🔄 Testing CRF {:.0} (anchor point)...", test_crf);
        let single_result = encode_gpu(test_crf);
        if let Ok(size) = &single_result {
            size_cache.insert(test_crf, *size);
            iterations += 1;
            size_history.push((test_crf, *size));
            if let Some(cb) = progress_cb {
                cb(test_crf, *size);
            }
        }
        vec![(test_crf, single_result)]
    } else {
        log_msg!(
            "   🚀 Parallel probe: CRF {:.0}, {:.0}, {:.0}",
            probe_crfs[0],
            probe_crfs[1],
            probe_crfs[2]
        );
        encode_parallel(&probe_crfs)
    };

    if !skip_parallel {
        for (crf, result) in &probe_results {
            if let Ok(size) = result {
                size_cache.insert(*crf, *size);
                iterations += 1;
                size_history.push((*crf, *size));
                if let Some(cb) = progress_cb {
                    cb(*crf, *size);
                }
            }
        }
    }

    let initial_result = probe_results
        .iter()
        .find(|(c, _)| (*c - probe_crfs[0]).abs() < 0.1);
    let max_result = if probe_crfs.len() > 1 {
        probe_results
            .iter()
            .find(|(c, _)| (*c - probe_crfs[1]).abs() < 0.1)
    } else {
        None
    };
    let min_result = if probe_crfs.len() > 2 {
        probe_results
            .iter()
            .find(|(c, _)| (*c - probe_crfs[2]).abs() < 0.1)
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
                        (1.0 - *max_size as f64 / *initial_size as f64) * 100.0
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


    const GPU_DECAY_FACTOR: f32 = 0.5;
    const GPU_MAX_WALL_HITS: u32 = 4;
    const GPU_MIN_STEP: f32 = 0.5;

    if (boundary_high - boundary_low) > 4.0 {
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
                GPU_DECAY_FACTOR,
                GPU_MAX_WALL_HITS
            );

            let mut current_step = initial_step;
            let mut wall_hits: u32 = 0;
            let mut test_crf = boundary_low + current_step;
            let mut last_compressible_crf = boundary_low;
            let mut last_compressible_size = best_size.unwrap_or(0);

            while test_crf <= config.max_crf && iterations < max_iterations_limit {
                let cached = size_cache.get(test_crf).copied();
                let size_result = match cached {
                    Some(s) => Ok(s),
                    None => encode_cached(test_crf, &mut size_cache),
                };

                match size_result {
                    Ok(size) => {
                        if cached.is_none() {
                            iterations += 1;
                            if let Some(cb) = progress_cb {
                                cb(test_crf, size);
                            }
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
                                (size as f64 / sample_input_size as f64 - 1.0) * 100.0,
                                current_step
                            );
                            test_crf += current_step;
                        } else {
                            wall_hits += 1;
                            log_msg!(
                                "   ✗ CRF {:.1}: WALL HIT #{} (size +{:.1}%)",
                                test_crf,
                                wall_hits,
                                (size as f64 / sample_input_size as f64 - 1.0) * 100.0
                            );

                            if wall_hits >= GPU_MAX_WALL_HITS {
                                log_msg!(
                                    "   🧱 MAX WALL HITS ({})! Stopping at CRF {:.1}",
                                    GPU_MAX_WALL_HITS,
                                    last_compressible_crf
                                );
                                boundary_high = test_crf;
                                break;
                            }

                            let curve_step = initial_step * GPU_DECAY_FACTOR.powi(wall_hits as i32);
                            let new_step = if curve_step < 1.0 {
                                GPU_MIN_STEP
                            } else {
                                curve_step
                            };

                            let phase_info = if new_step <= GPU_MIN_STEP + 0.01 {
                                "→ FINE TUNING".to_string()
                            } else {
                                format!("decay ×{:.1}^{}", GPU_DECAY_FACTOR, wall_hits)
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
                let size_result = match cached {
                    Some(s) => Ok(s),
                    None => encode_cached(test_crf, &mut size_cache),
                };

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
                                (size as f64 / sample_input_size as f64 - 1.0) * 100.0,
                                current_step
                            );
                            break;
                        } else {
                            wall_hits += 1;
                            log_msg!(
                                "   ✗ CRF {:.1}: WALL HIT #{} (size +{:.1}%)",
                                test_crf,
                                wall_hits,
                                (size as f64 / sample_input_size as f64 - 1.0) * 100.0
                            );

                            if wall_hits >= GPU_MAX_WALL_HITS {
                                log_msg!(
                                    "   🧱 MAX WALL HITS ({})! Cannot find compress point",
                                    GPU_MAX_WALL_HITS
                                );
                                break;
                            }

                            let curve_step = initial_step * GPU_DECAY_FACTOR.powi(wall_hits as i32);
                            let new_step = if curve_step < 1.0 {
                                GPU_MIN_STEP
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
                    }
                    Err(_) => break,
                }
            }

            let _ = last_fail_crf;
        }
    }

    let skip_stage2 = if let Some(b) = best_crf {
        let fract = (b * 2.0).fract();
        fract.abs() < 0.01 || (fract - 1.0).abs() < 0.01
    } else {
        false
    };

    if found_compress_point && !skip_stage2 && (boundary_high - boundary_low) > 1.0 {
        let mut lo = boundary_low.ceil() as i32;
        let mut hi = boundary_high.floor() as i32;

        let max_binary_iter = 5;
        let mut binary_iter = 0;

        while lo < hi && iterations < max_iterations_limit && binary_iter < max_binary_iter {
            binary_iter += 1;
            let mid = lo + (hi - lo) / 2;
            let test_crf = mid as f32;

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
            log_msg!("   📍 Stage 3: Fine-tune with 0.5 step (quality ceiling detection)");

            let mut offset = 0.5_f32;
            let mut consecutive_small_improvements = 0;

            #[allow(clippy::while_immutable_condition)]
            while iterations < max_iterations_limit {
                let test_crf = current_best - offset;

                if test_crf < config.min_crf {
                    log_msg!("   ⚡ Stop: reached min_crf {:.1}", config.min_crf);
                    break;
                }

                let result = if let Some(&cached_size) = size_cache.get(test_crf) {
                    log_msg!("   📦 Cache hit: CRF {:.1}", test_crf);
                    Ok(cached_size)
                } else {
                    encode_cached(test_crf, &mut size_cache)
                };

                match result {
                    Ok(size) => {
                        if let Some(cb) = progress_cb {
                            cb(test_crf, size);
                        }

                        if size < sample_input_size {
                            let improvement = best_size
                                .map(|b| (b as f64 - size as f64) / b as f64 * 100.0)
                                .unwrap_or(0.0);
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
                    }
                    Err(_) => {
                        log_msg!("   ⚠️ Encoding failed at CRF {:.1}, stopping", test_crf);
                        break;
                    }
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

    let (last_tested_crf, found, fine_tuned) = if let Some(b) = best_crf {
        (b, true, iterations > 8)
    } else {
        (config.max_crf, false, false)
    };

    let quality_ceiling_info = if ceiling_detector.ceiling_detected {
        ceiling_detector.get_ceiling()
    } else {
        None
    };

    let (quality_ceiling_crf, _quality_ceiling_psnr) = quality_ceiling_info
        .map(|(crf, psnr)| (Some(crf), if psnr > 0.0 { Some(psnr) } else { None }))
        .unwrap_or((None, None));

    let (gpu_ssim, gpu_psnr) = if found {
        log_msg!(
            "   📍 Final quality validation at CRF {:.1}",
            last_tested_crf
        );
        match encode_gpu(last_tested_crf) {
            Ok(_) => {
                let ssim_output = Command::new("ffmpeg")
                    .arg("-i")
                    .arg(crate::safe_path_arg(input).as_ref())
                    .arg("-i")
                    .arg(crate::safe_path_arg(output).as_ref())
                    .arg("-lavfi")
                    .arg("ssim")
                    .arg("-f")
                    .arg("null")
                    .arg("-")
                    .output();

                let psnr_result =
                    calculate_psnr_fast(&input.to_string_lossy(), &output.to_string_lossy());

                let ssim = match ssim_output {
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        if let Some(line) = stderr
                            .lines()
                            .find(|l| l.contains("SSIM") && l.contains("All:"))
                        {
                            if let Some(all_pos) = line.find("All:") {
                                let after_all = &line[all_pos + 4..];
                                if let Some(space_pos) = after_all.find(' ') {
                                    if let Ok(ssim) = after_all[..space_pos].parse::<f64>() {
                                        log_msg!("      📊 Final GPU SSIM: {:.6}", ssim);
                                        Some(ssim)
                                    } else {
                                        None
                                    }
                                } else if let Ok(ssim) = after_all.trim().parse::<f64>() {
                                    log_msg!("      📊 Final GPU SSIM: {:.6}", ssim);
                                    Some(ssim)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };

                let psnr = match psnr_result {
                    Ok(p) => {
                        log_msg!("      📊 Final GPU PSNR: {:.2}dB", p);
                        Some(p)
                    }
                    Err(_) => None,
                };

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

    let gpu_boundary_crf = if let Some(ceiling_crf) = quality_ceiling_info.map(|(crf, _)| crf) {
        log_msg!("   🎯 GPU Quality Ceiling Detected!");
        log_msg!("      └─ Ceiling CRF: {:.1} (PSNR plateau)", ceiling_crf);
        log_msg!("      └─ Last tested CRF: {:.1}", last_tested_crf);
        if ceiling_crf != last_tested_crf {
            log_msg!("      └─ Boundary = Ceiling (lower CRFs are bloated, no quality gain)");
        }
        ceiling_crf
    } else {
        last_tested_crf
    };

    log_msg!("   ═══════════════════════════════════════════════════");
    if found {
        log_msg!(
            "   📊 GPU Boundary CRF: {:.1} (highest quality that compresses)",
            gpu_boundary_crf
        );
        if let Some(size) = best_size {
            let ratio = size as f64 / sample_input_size as f64 * 100.0;
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
            "hevc" => CrfMapping::hevc(gpu.gpu_type),
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

    let _ = std::fs::remove_file(output);

    Ok(GpuCoarseResult {
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
    })
}

pub fn get_cpu_search_range_from_gpu(
    gpu_result: &GpuCoarseResult,
    original_min_crf: f32,
    original_max_crf: f32,
) -> (f32, f32, f32) {
    if !gpu_result.found_boundary {
        let center = (original_min_crf + original_max_crf) / 2.0;
        return (original_min_crf, original_max_crf, center);
    }

    let mapping = match gpu_result.codec.as_str() {
        "hevc" => CrfMapping::hevc(gpu_result.gpu_type),
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
            "Expected ~15.0, got {}",
            cpu_center
        );

        let cpu_center = estimate_cpu_search_center(10.0, GpuType::Nvidia, "hevc");
        assert!(
            (cpu_center - 14.0).abs() < 0.1,
            "Expected ~14.0, got {}",
            cpu_center
        );

        let cpu_center = estimate_cpu_search_center(10.0, GpuType::None, "hevc");
        assert!(
            (cpu_center - 10.0).abs() < 0.1,
            "Expected ~10.0, got {}",
            cpu_center
        );
    }

    #[test]
    fn test_gpu_boundary_to_cpu_range() {
        let (low, high) = gpu_boundary_to_cpu_range(10.0, GpuType::Apple, "hevc", 8.0, 28.0);
        assert!(
            (low - 10.0).abs() < 0.1,
            "low={} should be ~10.0 (GPU boundary)",
            low
        );
        assert!(
            (15.0..=22.0).contains(&high),
            "high={} should be in [15, 22]",
            high
        );

        let (low, _high) = gpu_boundary_to_cpu_range(12.0, GpuType::Nvidia, "hevc", 10.0, 28.0);
        assert!((low - 12.0).abs() < 0.1, "low should be GPU boundary");
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
            let qv: f32 = args[1].parse().unwrap();
            assert!(qv >= 1.0, "q:v should be >= 1, got {} for CRF {}", qv, crf);
            assert!(
                qv <= 100.0,
                "q:v should be <= 100, got {} for CRF {}",
                qv,
                crf
            );
        }
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
            let output = PathBuf::from(format!("/path/to/output.{}", ext));
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
            let ext = formats[format_idx];
            let output = PathBuf::from(format!("/video/output.{}", ext));
            let temp_ext = derive_gpu_temp_extension(&output);

            prop_assert_eq!(temp_ext, format!("gpu_temp.{}", ext),
                "Format {} should derive correctly", ext);
        }
    }
}
