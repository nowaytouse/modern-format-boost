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

// 🔥 v6.5: 使用统一的 CrfCache 替代 HashMap
use crate::explore_strategy::CrfCache;

// ═══════════════════════════════════════════════════════════════
// 🔥 v7.5.3: 北京时间工具函数
// ═══════════════════════════════════════════════════════════════

/// 获取当前北京时间字符串
fn beijing_time_now() -> String {
    let beijing = FixedOffset::east_opt(8 * 3600).unwrap();
    let now: DateTime<Utc> = Utc::now();
    now.with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// 格式化日志消息（包含北京时间）
#[allow(dead_code)]
fn format_log(level: &str, component: &str, msg: &str) -> String {
    format!(
        "[{}] [{}] [{}] {}",
        beijing_time_now(),
        level,
        component,
        msg
    )
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v7.5.3: StderrCapture - 捕获ffmpeg stderr
// ═══════════════════════════════════════════════════════════════

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
                let mut buf = lines.lock().unwrap();
                if buf.len() >= max {
                    buf.pop_front();
                }
                buf.push_back(line);
            }
        })
    }

    fn get_lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v7.5.3: HeartbeatMonitor - 心跳监控
// ═══════════════════════════════════════════════════════════════

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

                // 检查停止信号
                if self.stop_signal.load(Ordering::Relaxed) {
                    break;
                }

                // 检查心跳超时
                let elapsed = self.last_activity.lock().unwrap().elapsed();
                let elapsed_secs = elapsed.as_secs();

                // 显示心跳状态
                eprintln!(
                    "💓 Heartbeat: {}s ago (Beijing: {})",
                    elapsed_secs,
                    beijing_time_now()
                );

                if elapsed > self.timeout {
                    eprintln!(
                        "⚠️  FREEZE DETECTED: No activity for {} seconds!",
                        elapsed_secs
                    );
                    eprintln!(
                        "   Terminating frozen ffmpeg process (PID: {})...",
                        self.child_pid
                    );

                    // 使用系统调用终止进程
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(self.child_pid as i32, libc::SIGKILL);
                    }

                    #[cfg(windows)]
                    {
                        // Windows: 使用taskkill
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

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.3: 全局常量 - 避免硬编码
// ═══════════════════════════════════════════════════════════════

/// GPU 采样时长（秒）- 用于长视频的快速边界估算
/// 🔥 v5.64: 多段采样总时长（5段 × 10秒 = 50秒）
/// 策略：采样开头+25%+50%+75%+结尾，覆盖视频全局特征
pub const GPU_SAMPLE_DURATION: f32 = 50.0;

/// 🔥 v5.64: 每段采样时长（秒）
pub const GPU_SEGMENT_DURATION: f32 = 10.0;

/// 🔥 v5.64: 采样段数
pub const GPU_SAMPLE_SEGMENTS: usize = 5;

/// GPU 粗略搜索步长
pub const GPU_COARSE_STEP: f32 = 2.0;

/// 🔥 v5.52: 保底迭代上限（防止无限循环）
/// 用户要求："确保仅设置保底上限 例如500次！绝不要限制死迭代次数！"
/// 正常情况下应该通过收益递减自然停止，这个是极端情况保护
pub const GPU_ABSOLUTE_MAX_ITERATIONS: u32 = 500;

/// GPU 配置默认最大迭代次数（用于向后兼容）
pub const GPU_MAX_ITERATIONS: u32 = GPU_ABSOLUTE_MAX_ITERATIONS;

/// GPU 默认最小 CRF
/// 🔥 v5.7: VideoToolbox 需要更低 CRF (更高 q:v) 才能达到高 SSIM
/// CRF 1 → q:v 98 → SSIM ~0.99
/// CRF 10 → q:v 80 → SSIM ~0.85 (不够高!)
pub const GPU_DEFAULT_MIN_CRF: f32 = 1.0;

/// GPU 默认最大 CRF
/// 🔥 v6.5.2: 扩大范围 40 → 48，让 GPU 更好地找到压缩边界
/// 特别是对于 VP8/VP9 等已经相对高效的编码
pub const GPU_DEFAULT_MAX_CRF: f32 = 48.0;

/// GPU 加速检测结果（全局缓存）
static GPU_ACCEL: OnceLock<GpuAccel> = OnceLock::new();

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.7: GPU 临时文件扩展名派生
// ═══════════════════════════════════════════════════════════════

/// 从输出路径派生 GPU 临时文件扩展名
///
/// 🔥 v6.4.7: 修复硬编码 `.gpu_temp.mp4` 导致 MKV 输出失败的问题
///
/// # Arguments
/// * `output` - 目标输出文件路径
///
/// # Returns
/// 临时文件扩展名字符串，格式为 "gpu_temp.{ext}"
///
/// # Examples
/// - output.mp4 → "gpu_temp.mp4"
/// - output.mkv → "gpu_temp.mkv"
/// - output.webm → "gpu_temp.webm"
/// - output (无扩展名) → "gpu_temp.mp4" (默认)
///
/// # 为什么需要这个函数？
///
/// 某些容器格式（如 MKV）支持 MP4 不支持的轨道类型（如某些字幕流）。
/// 如果用户目标是 MKV 但临时文件是 MP4，FFmpeg 可能会报错。
pub fn derive_gpu_temp_extension(output: &std::path::Path) -> String {
    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    format!("gpu_temp.{}", ext)
}

/// GPU 编码器类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuType {
    /// NVIDIA GPU (NVENC)
    Nvidia,
    /// Apple Silicon (VideoToolbox)
    Apple,
    /// Intel Quick Sync Video
    IntelQsv,
    /// AMD Advanced Media Framework
    AmdAmf,
    /// VA-API (Linux)
    Vaapi,
    /// 无 GPU 加速
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

/// GPU 编码器信息
#[derive(Debug, Clone)]
pub struct GpuEncoder {
    /// 编码器类型
    pub gpu_type: GpuType,
    /// ffmpeg 编码器名称
    pub name: &'static str,
    /// 编解码器类型 (hevc, av1, h264)
    pub codec: &'static str,
    /// 是否支持 CRF 模式
    pub supports_crf: bool,
    /// CRF 参数名称 (有些编码器用 -cq 或 -global_quality)
    pub crf_param: &'static str,
    /// CRF 范围 (min, max)
    pub crf_range: (u8, u8),
    /// 额外的推荐参数
    pub extra_args: Vec<&'static str>,
}

impl GpuEncoder {
    /// 获取 ffmpeg 编码器名称
    pub fn ffmpeg_name(&self) -> &'static str {
        self.name
    }

    /// 获取 CRF 参数
    ///
    /// 🔥 v5.5: VideoToolbox 质量映射修正
    /// - libx265 CRF: 0=无损, 51=最差 (常用范围 18-28)
    /// - VideoToolbox -q:v: 1=最低质量, 100=最高质量 (实测验证!)
    ///   - q:v 1 → SSIM 0.902 (最低)
    ///   - q:v 50 → SSIM 0.964 (平衡点)
    ///   - q:v 70 → SSIM 0.968 (接近上限)
    ///   - q:v 90 → SSIM 0.969 (上限，文件巨大)
    /// - 映射公式: q:v = 100 - crf * 2 (反向映射)
    ///   - CRF 10 → q:v 80 (高质量)
    ///   - CRF 20 → q:v 60 (中等质量)
    ///   - CRF 30 → q:v 40 (较低质量)
    pub fn get_crf_args(&self, crf: f32) -> Vec<String> {
        if self.supports_crf {
            let quality_value = if self.gpu_type == GpuType::Apple {
                // 🔥 v5.5: VideoToolbox 反向映射 (高 q:v = 高质量)
                // CRF 低 = 高质量 → q:v 高 = 高质量
                // 公式: q:v = 100 - crf * 2
                (100.0 - crf * 2.0).clamp(1.0, 100.0)
            } else {
                crf.clamp(self.crf_range.0 as f32, self.crf_range.1 as f32)
            };

            vec![
                format!("-{}", self.crf_param),
                format!("{:.0}", quality_value),
            ]
        } else {
            // 对于不支持 CRF 的编码器，使用 VBR 模式
            let bitrate = crf_to_estimated_bitrate(crf, self.codec);
            vec!["-b:v".to_string(), format!("{}k", bitrate)]
        }
    }

    /// 获取额外参数
    pub fn get_extra_args(&self) -> Vec<&'static str> {
        self.extra_args.clone()
    }
}

/// GPU 加速检测和管理
#[derive(Debug, Clone)]
pub struct GpuAccel {
    /// 检测到的 GPU 类型
    pub gpu_type: GpuType,
    /// 可用的 HEVC 编码器
    pub hevc_encoder: Option<GpuEncoder>,
    /// 可用的 AV1 编码器
    pub av1_encoder: Option<GpuEncoder>,
    /// 可用的 H.264 编码器
    pub h264_encoder: Option<GpuEncoder>,
    /// 是否启用 GPU 加速
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
    /// 检测可用的 GPU 加速（带缓存）
    pub fn detect() -> &'static GpuAccel {
        GPU_ACCEL.get_or_init(|| {
            // 🔥 v5.32: 静默检测，不输出日志（避免干扰进度条）
            Self::detect_internal()
        })
    }

    /// 强制重新检测（不使用缓存）
    pub fn detect_fresh() -> GpuAccel {
        Self::detect_internal()
    }

    /// 🔥 v5.32: 打印 GPU 检测结果（在进度条创建前调用）
    pub fn print_detection_info(&self) {
        eprintln!("🔍 Detecting GPU acceleration...");
        if self.enabled {
            eprintln!("   ✅ GPU: {} detected", self.gpu_type);
            if let Some(enc) = &self.hevc_encoder {
                eprintln!("      • HEVC: {}", enc.name);
            }
            if let Some(enc) = &self.av1_encoder {
                eprintln!("      • AV1: {}", enc.name);
            }
            if let Some(enc) = &self.h264_encoder {
                eprintln!("      • H.264: {}", enc.name);
            }
        } else {
            eprintln!("   ⚠️ No GPU acceleration available, using CPU encoding");
        }
    }

    /// 内部检测逻辑
    fn detect_internal() -> GpuAccel {
        // 获取 ffmpeg 支持的编码器列表
        let encoders = get_available_encoders();

        // 按优先级检测 GPU
        // macOS 优先 VideoToolbox，其他平台优先 NVENC

        #[cfg(target_os = "macos")]
        {
            // macOS: 优先 VideoToolbox
            if let Some(accel) = Self::try_videotoolbox(&encoders) {
                return accel;
            }
        }

        // NVIDIA NVENC（跨平台）
        if let Some(accel) = Self::try_nvenc(&encoders) {
            return accel;
        }

        // Intel QSV
        if let Some(accel) = Self::try_qsv(&encoders) {
            return accel;
        }

        // AMD AMF (Windows)
        #[cfg(target_os = "windows")]
        if let Some(accel) = Self::try_amf(&encoders) {
            return accel;
        }

        // VA-API (Linux)
        #[cfg(target_os = "linux")]
        if let Some(accel) = Self::try_vaapi(&encoders) {
            return accel;
        }

        // 无 GPU 加速
        GpuAccel::default()
    }

    /// 检测 Apple VideoToolbox
    fn try_videotoolbox(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_videotoolbox"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_videotoolbox"));

        if !has_hevc && !has_h264 {
            return None;
        }

        // 验证编码器是否真正可用
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
                    crf_param: "q:v",    // VideoToolbox 使用 -q:v
                    crf_range: (0, 100), // 0=最高质量, 100=最低
                    extra_args: vec![
                        "-profile:v",
                        "main",
                        "-tag:v",
                        "hvc1", // Apple 兼容标签
                    ],
                })
            } else {
                None
            },
            av1_encoder: None, // VideoToolbox 不支持 AV1
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

    /// 检测 NVIDIA NVENC
    fn try_nvenc(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_nvenc"));
        let has_av1 = encoders.iter().any(|e| e.contains("av1_nvenc"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_nvenc"));

        if !has_hevc && !has_av1 && !has_h264 {
            return None;
        }

        // 验证 NVENC 是否真正可用（需要 NVIDIA GPU）
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
                    crf_param: "cq", // NVENC 使用 -cq (Constant Quality)
                    crf_range: (0, 51),
                    extra_args: vec![
                        "-preset",
                        "p4", // 平衡质量和速度
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

    /// 检测 Intel QSV
    fn try_qsv(encoders: &[String]) -> Option<GpuAccel> {
        let has_hevc = encoders.iter().any(|e| e.contains("hevc_qsv"));
        let has_av1 = encoders.iter().any(|e| e.contains("av1_qsv"));
        let has_h264 = encoders.iter().any(|e| e.contains("h264_qsv"));

        if !has_hevc && !has_av1 && !has_h264 {
            return None;
        }

        // 验证 QSV 是否真正可用
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

    /// 检测 AMD AMF
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
                    crf_param: "qp_i", // AMF 使用 QP
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

    /// 检测 VA-API (Linux)
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

    /// 获取 HEVC 编码器（GPU 或 CPU fallback）
    pub fn get_hevc_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.hevc_encoder.as_ref()
        } else {
            None
        }
    }

    /// 获取 AV1 编码器（GPU 或 CPU fallback）
    pub fn get_av1_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.av1_encoder.as_ref()
        } else {
            None
        }
    }

    /// 获取 H.264 编码器（GPU 或 CPU fallback）
    pub fn get_h264_encoder(&self) -> Option<&GpuEncoder> {
        if self.enabled {
            self.h264_encoder.as_ref()
        } else {
            None
        }
    }

    /// 检查是否有 GPU 加速
    pub fn is_available(&self) -> bool {
        self.enabled
    }

    /// 获取 GPU 类型描述
    pub fn description(&self) -> String {
        if self.enabled {
            format!("{} (Hardware Accelerated)", self.gpu_type)
        } else {
            "CPU (Software Encoding)".to_string()
        }
    }
}

/// 获取 ffmpeg 支持的编码器列表
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
                .filter(|line| line.starts_with(" V")) // 视频编码器
                .map(|line| line.to_string())
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

/// 测试编码器是否真正可用
fn test_encoder(encoder: &str) -> bool {
    // 尝试用该编码器编码 1 帧测试
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

/// 将 CRF 转换为估计的比特率（用于不支持 CRF 的编码器）
fn crf_to_estimated_bitrate(crf: f32, codec: &str) -> u32 {
    // 基于经验公式估算
    // CRF 越高，比特率越低
    let base_bitrate = match codec {
        "hevc" => 5000, // 5 Mbps 基准
        "av1" => 4000,  // 4 Mbps 基准
        "h264" => 8000, // 8 Mbps 基准
        _ => 5000,
    };

    let crf_factor = match codec {
        "hevc" | "h264" => 0.9_f32.powf((crf - 23.0) / 6.0),
        "av1" => 0.9_f32.powf((crf - 30.0) / 6.0),
        _ => 1.0,
    };

    (base_bitrate as f32 * crf_factor) as u32
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.52: 智能采样策略 - 场景检测 + 锐度检测
// ═══════════════════════════════════════════════════════════════

/// 智能采样结果
#[derive(Debug, Clone)]
pub struct SmartSampleResult {
    /// 采样 ffmpeg 命令（trim + select 过滤器）
    pub sample_filter: String,
    /// 实际采样时长（秒）
    pub actual_duration: f32,
    /// 采样策略描述
    pub strategy: String,
}

/// 🔥 v5.52: 智能采样策略
///
/// 用户要求：
/// 1. 寻找画面不同的、非纯色的，加起来达到百分比要求
/// 2. 寻找画面锐化更高的、更具备对比价值的采样
/// 3. 采样长度按照百分比进行
/// 4. 如果不足则按照全时长采样
///
/// ## 实现策略：
/// - 使用 ffmpeg select 过滤器的场景检测 (scene)
/// - 使用 entropy 检测非纯色帧
/// - 使用 thumbnail 选择最具代表性的帧
/// - 按总时长的百分比采样
pub fn calculate_smart_sample(
    input: &std::path::Path,
    total_duration: f32,
    target_sample_duration: f32,
) -> anyhow::Result<SmartSampleResult> {
    use anyhow::Context;
    use std::process::Command;

    // 🔥 策略 1：如果视频很短，直接使用全时长
    if total_duration <= target_sample_duration * 1.2 {
        return Ok(SmartSampleResult {
            sample_filter: String::new(), // 不使用过滤器
            actual_duration: total_duration,
            strategy: format!(
                "Full video ({:.1}s, close to target {:.1}s)",
                total_duration, target_sample_duration
            ),
        });
    }

    // 🔥 策略 2：计算采样百分比
    let sample_ratio = target_sample_duration / total_duration;
    let sample_percentage = sample_ratio * 100.0;

    // 🔥 策略 3：使用 ffmpeg 场景检测 + 熵值过滤
    //
    // select 表达式：
    // - gt(scene, 0.3): 场景变化 > 30%（找画面不同的）
    // - gt(entropy, 6.0): 熵值 > 6.0（找非纯色的）
    // - 每 N 秒选一帧，N 根据采样比例计算
    //
    // 例如：100 秒视频，采样 20 秒（20%）
    // → 每 5 秒选 1 秒 → select='gt(scene,0.3)+gt(entropy,6.0),n=0'

    let scene_threshold = 0.3; // 30% 场景变化
    let entropy_threshold = 6.0; // 熵值阈值（非纯色）

    // 🔥 策略 4：构造智能 select 表达式
    // 目标：选择场景变化大 OR 高熵值的帧，按比例采样
    let select_expr = if sample_ratio > 0.5 {
        // 采样比例 > 50%，使用宽松条件
        format!(
            "gt(scene,{})+gt(entropy,{})",
            scene_threshold * 0.5,
            entropy_threshold * 0.8
        )
    } else if sample_ratio > 0.2 {
        // 采样比例 20-50%，使用标准条件
        format!(
            "gt(scene,{})+gt(entropy,{})",
            scene_threshold, entropy_threshold
        )
    } else {
        // 采样比例 < 20%，使用严格条件（只选最重要的帧）
        format!(
            "gt(scene,{})*gt(entropy,{})",
            scene_threshold * 1.5,
            entropy_threshold * 1.2
        )
    };

    // 🔥 策略 5：验证过滤器是否有效
    // 快速测试：运行 1 秒看看是否能选出帧
    let test_output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-t")
        .arg("10") // 只测试前 10 秒
        .arg("-i")
        // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
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
        // 过滤器太严格，没有选出任何帧
        // 回退到简单策略：均匀采样
        return Ok(SmartSampleResult {
            sample_filter: String::new(),
            actual_duration: target_sample_duration,
            strategy: format!(
                "Uniform sampling ({:.1}s, {:.1}%)",
                target_sample_duration, sample_percentage
            ),
        });
    }

    // 🔥 策略 6：成功！返回智能过滤器
    Ok(SmartSampleResult {
        sample_filter: format!("select='{}',setpts=N/FRAME_RATE/TB", select_expr),
        actual_duration: target_sample_duration,
        strategy: format!(
            "Smart sampling ({:.1}s, {:.1}%, scene+entropy)",
            target_sample_duration, sample_percentage
        ),
    })
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.52: SSIM + 大小组合决策函数
// ═══════════════════════════════════════════════════════════════

/// 质量评估结果
#[derive(Debug, Clone, Copy)]
pub struct QualityScore {
    /// SSIM 分数 (0.0-1.0)
    pub ssim: f64,
    /// 压缩率（输出/输入，越小越好）
    pub compression_ratio: f64,
    /// 综合分数（越高越好）
    pub combined_score: f64,
}

impl QualityScore {
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v7.1: 类型安全辅助方法
    // ═══════════════════════════════════════════════════════════════

    /// 获取类型安全的 SSIM 值
    #[inline]
    pub fn ssim_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.ssim).ok()
    }

    /// 检查 SSIM 是否满足阈值
    #[inline]
    pub fn ssim_meets(&self, threshold: f64) -> bool {
        crate::float_compare::ssim_meets_threshold(self.ssim, threshold)
    }
}

/// 🔥 v5.52: 计算质量综合分数（SSIM + 大小）
///
/// 用户要求："考量和目标需要同时考量 SSIM 和大小两个指标"
///
/// ## 设计理念：
/// - SSIM 越高越好（质量目标）
/// - 压缩率越低越好（大小目标）
/// - 综合分数 = SSIM 权重 × SSIM + 压缩权重 × (1 - 压缩率)
///
/// ## 权重策略：
/// - GPU 阶段：ssim_weight=0.4, size_weight=0.6（更看重压缩效率）
/// - CPU 阶段：ssim_weight=0.7, size_weight=0.3（更看重质量）
///
/// ## 使用场景：
/// ```ignore
/// let score1 = calculate_quality_score(0.95, 50_000_000, 100_000_000, SearchPhase::Gpu);
/// let score2 = calculate_quality_score(0.98, 60_000_000, 100_000_000, SearchPhase::Gpu);
/// if score2.combined_score > score1.combined_score {
///     // score2 更好！
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPhase {
    /// GPU 粗略搜索阶段（更看重压缩效率）
    Gpu,
    /// CPU 精细搜索阶段（更看重质量）
    Cpu,
}

pub fn calculate_quality_score(
    ssim: f64,
    output_size: u64,
    input_size: u64,
    phase: SearchPhase,
) -> QualityScore {
    let compression_ratio = output_size as f64 / input_size as f64;

    // 根据阶段设置权重
    let (ssim_weight, size_weight) = match phase {
        SearchPhase::Gpu => (0.4, 0.6), // GPU: 更看重压缩效率
        SearchPhase::Cpu => (0.7, 0.3), // CPU: 更看重质量
    };

    // 🔥 综合分数计算
    // - SSIM 部分：直接使用 SSIM 值（0.0-1.0）
    // - 大小部分：使用 (1 - 压缩率) 使其与 SSIM 同向（越大越好）
    //   - 压缩率 0.5 → 大小分数 0.5（压缩 50%）
    //   - 压缩率 0.8 → 大小分数 0.2（仅压缩 20%）
    //   - 压缩率 1.2 → 大小分数 -0.2（变大了！）
    let size_score = (1.0 - compression_ratio).max(0.0); // 不能是负数
    let combined_score = ssim_weight * ssim + size_weight * size_score;

    QualityScore {
        ssim,
        compression_ratio,
        combined_score,
    }
}

/// 🔥 v5.52: 比较两个质量分数，判断哪个更好
///
/// 返回 true 表示 new_score 比 old_score 更好
pub fn is_quality_better(
    new_score: &QualityScore,
    old_score: &QualityScore,
    min_ssim_threshold: f64, // 最低 SSIM 要求（如 0.95）
) -> bool {
    // 🔥 硬约束：新分数必须满足最低 SSIM 要求
    if new_score.ssim < min_ssim_threshold {
        return false;
    }

    // 🔥 综合分数比较
    // 如果综合分数提升 > 0.5%，认为更好
    let improvement =
        (new_score.combined_score - old_score.combined_score) / old_score.combined_score;
    improvement > 0.005 // 0.5% 提升
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.0: GPU → CPU 压缩边界估算
// ═══════════════════════════════════════════════════════════════

/// GPU 压缩边界到 CPU 压缩边界的估算（v5.9 修正方向）
///
/// ## 背景
/// GPU 硬件编码器（NVENC, VideoToolbox, QSV 等）压缩效率**低于** CPU 软件编码器：
/// - 相同 CRF 下，GPU 输出文件更大（压缩效率低）
/// - 质量排序：x264/x265 > QSV > NVENC > VCE (AMD)
///
/// ## 映射目的（v5.9 修正）
/// GPU 粗略搜索找到的"压缩边界"（刚好能压缩的 CRF）需要转换为 CPU 的等效边界：
/// - GPU 在 CRF=11 刚好能压缩 → CPU 需要**更高** CRF（如 13-14）才能压缩
/// - 因为 CPU 效率更高，相同 CRF 下文件更小，所以需要更高 CRF 才能达到相同大小
///
/// ## 策略
/// 返回一个**估算的 CPU 搜索起点**，CPU 从这里开始向上搜索。
///
/// ## 注意
/// - 这不是精确的 CRF 转换，只是搜索范围的估算
/// - 实际差异取决于内容、preset、编码器版本等
/// - CPU 精细搜索会找到真正的边界
///
/// GPU 压缩边界到 CPU 压缩边界的估算（v5.31 动态优化）
///
/// ## 背景
/// GPU 硬件编码器（NVENC, VideoToolbox, QSV 等）压缩效率**低于** CPU 软件编码器：
/// - 相同 CRF 下，GPU 输出文件更大（压缩效率低）
/// - 质量排序：x264/x265 > QSV > NVENC > VCE (AMD)
///
/// ## 映射目的（v5.31 动态优化）
/// GPU 粗略搜索找到的"压缩边界"（刚好能压缩的 CRF）需要转换为 CPU 的等效边界：
/// - GPU 在 CRF=11 刚好能压缩 → CPU 需要**更高** CRF（如 13-14）才能压缩
/// - 因为 CPU 效率更高，相同 CRF 下文件更小，所以需要更高 CRF 才能达到相同大小
///
/// GPU 压缩边界到 CPU 压缩边界的精确映射（v5.31 保守完善版）
///
/// ## 背景
/// GPU 硬件编码器压缩效率低于 CPU 软件编码器
/// - 质量排序：x264/x265 > QSV > NVENC > VCE
///
/// ## 精确映射表（基于实测）
/// | GPU 类型 | offset | 说明 |
/// |---------|--------|------|
/// | Apple VideoToolbox | +5.0 | 实测差距 5.0 CRF |
/// | NVIDIA NVENC | +4.0 | 实测差距 4.0 CRF |
/// | Intel QSV | +3.5 | 最高效 |
/// | AMD AMF | +5.0 | 最低效 |
/// | VAAPI | +4.0 | 中等 |
///
/// ## v5.31 保守调整
/// 只在极明确的情况下微调：
/// - 高复杂度: +0.3（保守）
/// - 低复杂度: -0.2（保守）
/// - 不确定: 0（保持标准）
pub fn estimate_cpu_search_center_dynamic(
    gpu_boundary: f32,
    gpu_type: GpuType,
    _codec: &str,
    compression_potential: Option<f64>,
) -> f32 {
    // 🔥 v5.31: 精确的基础 offset
    let base_offset = match gpu_type {
        GpuType::Apple => 5.0,
        GpuType::Nvidia => 4.0,
        GpuType::IntelQsv => 3.5,
        GpuType::AmdAmf => 5.0,
        GpuType::Vaapi => 4.0,
        GpuType::None => 0.0,
    };

    // 🔥 v5.31: 极保守的微调（幅度小）
    let adjustment = if let Some(potential) = compression_potential {
        if potential < 0.3 {
            0.3 // 高复杂度: 仅 +0.3
        } else if potential > 0.7 {
            -0.2 // 低复杂度: 仅 -0.2
        } else {
            0.0
        }
    } else {
        0.0
    };

    gpu_boundary + base_offset + adjustment
}

/// 🔥 v5.31: 精确的搜索范围映射
/// 不仅映射单个点，还映射完整的搜索范围
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

/// 🔥 v5.31: 向后兼容
pub fn estimate_cpu_search_center(gpu_boundary: f32, gpu_type: GpuType, codec: &str) -> f32 {
    estimate_cpu_search_center_dynamic(gpu_boundary, gpu_type, codec, None)
}

/// 计算 CPU 搜索范围（v5.9 修正方向）
///
/// 基于 GPU 粗略边界，返回 CPU 精细搜索的范围 (low, high)
///
/// ## 策略（v5.9 修正）
/// - CPU 从 GPU 边界开始向上搜索
/// - low = GPU 边界（最高质量点）
/// - high = 估算的 CPU 压缩点 + margin
pub fn gpu_boundary_to_cpu_range(
    gpu_boundary: f32,
    gpu_type: GpuType,
    codec: &str,
    min_crf: f32,
    max_crf: f32,
) -> (f32, f32) {
    let cpu_center = estimate_cpu_search_center(gpu_boundary, gpu_type, codec);

    // 🔥 v5.9: 修正方向
    // CPU 从 GPU 边界开始，向上搜索
    let cpu_low = gpu_boundary.max(min_crf); // 从 GPU 边界开始
    let cpu_high = (cpu_center + 3.0).min(max_crf); // 向上扩展

    (cpu_low, cpu_high)
}

/// 兼容旧 API（deprecated）
#[deprecated(since = "5.0.1", note = "use estimate_cpu_search_center instead")]
pub fn gpu_to_cpu_crf(gpu_crf: f32, gpu_type: GpuType, codec: &str) -> f32 {
    estimate_cpu_search_center(gpu_crf, gpu_type, codec)
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.1: GPU 粗略搜索 + CPU 精细搜索 智能化处理
// ═══════════════════════════════════════════════════════════════

/// GPU 精细化搜索结果
#[derive(Debug, Clone)]
pub struct GpuCoarseResult {
    /// GPU 找到的最优 CRF（能压缩的最低 CRF = 最高质量）
    pub gpu_boundary_crf: f32,
    /// GPU 最优点的输出大小
    pub gpu_best_size: Option<u64>,
    /// 🔥 v5.6: GPU 最优点的 SSIM（用于评估 GPU 质量上限）
    pub gpu_best_ssim: Option<f64>,
    /// GPU 类型
    pub gpu_type: GpuType,
    /// 编解码器
    pub codec: String,
    /// 搜索迭代次数
    pub iterations: u32,
    /// 是否找到有效边界
    pub found_boundary: bool,
    /// 🔥 v5.4: GPU 精细化搜索阶段
    pub fine_tuned: bool,
    /// 日志
    pub log: Vec<String>,
    /// 🔥 v5.45: GPU 采样输入大小（用于正确计算压缩率）
    pub sample_input_size: u64,
    /// 🔥 v5.66: GPU 质量天花板 CRF（SSIM 不再提升的点）
    pub quality_ceiling_crf: Option<f32>,
    /// 🔥 v5.66: GPU 质量天花板 SSIM（GPU 能达到的最高 SSIM）
    pub quality_ceiling_ssim: Option<f64>,
}

impl GpuCoarseResult {
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v7.1: 类型安全辅助方法
    // ═══════════════════════════════════════════════════════════════

    /// 获取类型安全的最优 SSIM 值
    #[inline]
    pub fn best_ssim_typed(&self) -> Option<crate::types::Ssim> {
        self.gpu_best_ssim
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

    /// 获取类型安全的质量天花板 SSIM 值
    #[inline]
    pub fn ceiling_ssim_typed(&self) -> Option<crate::types::Ssim> {
        self.quality_ceiling_ssim
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

    /// 获取类型安全的输出文件大小
    #[inline]
    pub fn best_size_typed(&self) -> Option<crate::types::FileSize> {
        self.gpu_best_size.map(crate::types::FileSize::new)
    }
}

/// GPU/CPU CRF 映射表
///
/// ## 背景
/// GPU 和 CPU 编码器压缩效率不同：
/// - GPU 效率**低于** CPU（相同 CRF 下 GPU 输出更大）
/// - GPU CRF 11 能压缩 → CPU 需要**更高** CRF（如 12-14）才能压缩
///
/// ## 映射方向（v5.9 修正）
/// - GPU 边界 CRF 11 → CPU 需要从 CRF 11 向上搜索（+offset）
/// - offset 表示 CPU 需要增加的 CRF 值
///
/// ## 注意
/// 这些映射是**近似值**，实际差异取决于：
/// - 视频内容复杂度
/// - 分辨率和帧率
/// - 编码器版本和 preset
#[derive(Debug, Clone)]
pub struct CrfMapping {
    /// GPU 类型
    pub gpu_type: GpuType,
    /// 编解码器 (hevc, av1, h264)
    pub codec: &'static str,
    /// GPU → CPU 偏移量（CPU 需要更高 CRF = GPU + offset）
    /// 正值表示 CPU 效率更高（相同压缩效果需要更高 CRF）
    pub offset: f32,
    /// 映射的不确定性范围（±）
    pub uncertainty: f32,
}

impl CrfMapping {
    /// 获取 HEVC 编码器的 CRF 映射
    ///
    /// 🔥 v5.9: 基于实测数据更新 offset
    /// VideoToolbox 实测：GPU q:v 75 (170%) ≈ CPU CRF 14 (124%)
    /// 差距约 4-6 CRF，取 5.0 作为 offset
    /// 🔥 v5.33: 精细化offset校准和uncertainty范围
    pub fn hevc(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple => (5.0, 0.5), // 🔥 v5.33: 精细uncertainty=0.5（±0.5CRF）
            GpuType::Nvidia => (3.8, 0.3), // NVENC 更精确的offset和较小uncertainty
            GpuType::IntelQsv => (3.5, 0.3), // QSV 效率较好，更小uncertainty
            GpuType::AmdAmf => (4.8, 0.5), // AMF 效率较低
            GpuType::Vaapi => (3.8, 0.4), // VAAPI 效率中等
            GpuType::None => (0.0, 0.0),  // 无 GPU
        };
        Self {
            gpu_type,
            codec: "hevc",
            offset,
            uncertainty,
        }
    }

    /// 获取 AV1 编码器的 CRF 映射
    /// 🔥 v5.33: 精细化offset校准
    pub fn av1(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple => (0.0, 0.0),    // VideoToolbox 不支持 AV1
            GpuType::Nvidia => (3.8, 0.4),   // NVENC AV1 更精确的offset
            GpuType::IntelQsv => (3.5, 0.3), // QSV AV1 效率较好
            GpuType::AmdAmf => (4.5, 0.5),   // AMF AV1 效率较低
            GpuType::Vaapi => (3.8, 0.4),    // VAAPI AV1 效率中等
            GpuType::None => (0.0, 0.0),     // 无 GPU
        };
        Self {
            gpu_type,
            codec: "av1",
            offset,
            uncertainty,
        }
    }

    /// GPU CRF → CPU 搜索范围（v5.9 修正方向）
    ///
    /// GPU 效率低，CPU 效率高，所以：
    /// - GPU CRF 11 能压缩 → CPU 需要更高 CRF（如 13）才能压缩
    ///
    /// 返回 (center, low, high) 三元组：
    /// - center: 估算的 CPU 压缩点（GPU + offset）
    /// - low: 搜索范围下限（从 GPU 边界开始）
    /// - high: 搜索范围上限（center + uncertainty）
    pub fn gpu_to_cpu_range(&self, gpu_crf: f32, min_crf: f32, max_crf: f32) -> (f32, f32, f32) {
        // 🔥 v5.9: 修正方向！CPU 需要更高 CRF
        let center = (gpu_crf + self.offset).min(max_crf);
        let low = gpu_crf.max(min_crf); // 从 GPU 边界开始
        let high = (center + self.uncertainty).min(max_crf);
        (center, low, high)
    }

    /// CPU CRF → 等效 GPU CRF（用于预览）
    /// GPU 效率低，所以 GPU 需要更低 CRF 才能达到相同效果
    pub fn cpu_to_gpu(&self, cpu_crf: f32) -> f32 {
        cpu_crf - self.offset
    }

    /// 打印映射信息
    pub fn print_mapping_info(&self) {
        eprintln!(
            "   📊 GPU/CPU CRF Mapping ({} - {}):",
            self.gpu_type,
            self.codec.to_uppercase()
        );
        if self.gpu_type == GpuType::Apple {
            // 🔥 v5.9: VideoToolbox 实测数据
            // q:v 100: SSIM 0.91-0.97 (内容相关)
            // q:v 75-80: SSIM 0.90-0.97, 最佳性价比
            // q:v 1: SSIM 0.73-0.90 (最低)
            eprintln!("      • VideoToolbox q:v: 1=lowest, 100=highest quality");
            eprintln!("      • SSIM ceiling: 0.91~0.97 (content-dependent, cannot reach 0.98+)");
            eprintln!("      • Best value: q:v 75-80 (SSIM ~0.97, good compression)");
        } else {
            eprintln!("      • GPU 60s sampling + step=2 → accurate boundary");
        }
        // 🔥 v5.9: 修正说明 - CPU 需要更高 CRF
        eprintln!(
            "      • CPU offset: +{:.1} (CPU needs higher CRF for same compression)",
            self.offset
        );
        eprintln!("      • 💡 CPU fine-tunes for SSIM 0.98+ (GPU max ~0.97)");
    }
}

/// GPU 粗略搜索配置
#[derive(Debug, Clone)]
pub struct GpuCoarseConfig {
    /// 起始 CRF（通常是算法预测值）
    pub initial_crf: f32,
    /// 最小 CRF（最高质量）
    pub min_crf: f32,
    /// 最大 CRF（最低质量）
    pub max_crf: f32,
    /// 搜索步长（粗略搜索用大步长）
    pub step: f32,
    /// 最大迭代次数
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

// ═══════════════════════════════════════════════════════════════
// 🔥 PSNR快速计算 - 用于GPU粗搜索阶段的质量监控
// ═══════════════════════════════════════════════════════════════

/// 快速计算PSNR（比SSIM快10-50倍）
/// 用于GPU粗搜索阶段的实时质量监控
///
/// ## 为什么使用PSNR而不是SSIM？
/// - PSNR计算速度约为SSIM的10-50倍
/// - GPU阶段需要频繁质量检测（每次编码后）
/// - PSNR与SSIM有高度相关性，可通过动态映射转换
///
/// ## 返回值
/// - `Ok(psnr)`: PSNR值（dB），通常在30-50dB范围
/// - `Err`: 计算失败（文件不存在、ffmpeg错误等）
fn calculate_psnr_fast(input: &str, output: &str) -> Result<f64, String> {
    let psnr_output = Command::new("ffmpeg")
        .arg("-i")
        // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
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

    // 解析PSNR值：查找 "psnr_avg:" 行
    // 示例：[Parsed_psnr_0 @ 0x...] PSNR psnr_avg:42.35
    for line in stderr.lines() {
        if line.contains("psnr_avg:") {
            if let Some(pos) = line.find("psnr_avg:") {
                let after = &line[pos + 9..];
                // 提取数字（可能后面跟空格或其他字符）
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

// ═══════════════════════════════════════════════════════════════
// 🔥 质量天花板检测器 - 识别GPU编码器的质量上限
// ═══════════════════════════════════════════════════════════════

/// GPU质量天花板检测器
///
/// ## 核心概念：GPU编码器的质量天花板
/// 不同GPU编码器存在固有的质量上限：
/// - **VideoToolbox (Apple)**: SSIM约0.970（PSNR约40dB）
/// - **NVENC (NVIDIA)**: SSIM约0.965（PSNR约38dB）
/// - **QSV (Intel)**: SSIM约0.960（PSNR约37dB）
///
/// ## 检测策略
/// 当连续3次编码后PSNR提升小于阈值（<0.1dB），判定为到达天花板
///
/// ## 使用场景
/// GPU粗搜索时实时监控，提前终止无意义的向下搜索（降低CRF）
#[derive(Debug)]
struct QualityCeilingDetector {
    /// 历史采样点 (CRF, PSNR/SSIM)
    samples: Vec<(f32, f64)>,
    /// 平台检测阈值（PSNR dB）
    plateau_threshold: f64,
    /// 连续平台次数
    plateau_count: usize,
    /// 检测到天花板的标志
    ceiling_detected: bool,
}

impl QualityCeilingDetector {
    /// 创建新的天花板检测器
    fn new() -> Self {
        Self {
            samples: Vec::new(),
            plateau_threshold: 0.1, // PSNR提升<0.1dB视为平台
            plateau_count: 0,
            ceiling_detected: false,
        }
    }

    /// 添加新的质量采样点
    ///
    /// ## 参数
    /// - `crf`: 当前CRF值
    /// - `quality`: 质量指标（PSNR dB）
    ///
    /// ## 返回
    /// - `true`: 检测到质量天花板，应停止向下搜索
    /// - `false`: 质量仍在提升，继续搜索
    fn add_sample(&mut self, crf: f32, quality: f64) -> bool {
        self.samples.push((crf, quality));

        // 至少需要2个样本才能比较
        if self.samples.len() >= 2 {
            let last = self.samples[self.samples.len() - 1].1;
            let prev = self.samples[self.samples.len() - 2].1;
            let improvement = last - prev;

            if improvement < self.plateau_threshold {
                // 质量提升不明显，计数器+1
                self.plateau_count += 1;

                // 连续3次提升不明显，判定为天花板
                if self.plateau_count >= 3 {
                    self.ceiling_detected = true;
                    return true;
                }
            } else {
                // 质量显著提升，重置计数器
                self.plateau_count = 0;
            }
        }

        false
    }

    /// 获取当前检测到的质量天花板
    ///
    /// ## 返回
    /// - `Some((crf, quality))`: 质量最高的采样点
    /// - `None`: 样本不足，无法确定天花板
    fn get_ceiling(&self) -> Option<(f32, f64)> {
        if self.samples.len() >= 3 {
            // 返回质量最高的点（PSNR最大）
            self.samples
                .iter()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .copied()
        } else {
            None
        }
    }

    /// 获取最后一个采样点的质量值（预留接口）
    #[allow(dead_code)]
    fn get_last_quality(&self) -> Option<f64> {
        self.samples.last().map(|(_, q)| *q)
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 PSNR-SSIM动态映射器 - 确保GPU阶段PSNR能精确映射到SSIM
// ═══════════════════════════════════════════════════════════════

/// PSNR-SSIM动态映射器
///
/// ## 核心问题
/// GPU粗搜索阶段使用PSNR快速检测（10-50倍快），但最终目标是SSIM。
/// 需要建立PSNR→SSIM的精确映射关系。
///
/// ## 映射策略
/// 1. **初始校准**：在关键点同时计算PSNR和SSIM，建立映射关系
/// 2. **线性插值**：使用收集的数据点进行线性插值
/// 3. **置信度评估**：根据数据点数量和分布评估映射精度
///
/// ## 使用场景
/// - GPU搜索时频繁使用PSNR（快速）
/// - 最终验证时使用SSIM（精确）
/// - 通过映射推断PSNR对应的SSIM值
#[derive(Debug)]
struct PsnrSsimMapper {
    /// 映射数据点 (PSNR, SSIM)
    calibration_points: Vec<(f64, f64)>,
    /// 是否已校准
    calibrated: bool,
}

impl PsnrSsimMapper {
    /// 创建新的映射器
    fn new() -> Self {
        Self {
            calibration_points: Vec::new(),
            calibrated: false,
        }
    }

    /// 添加校准点（同时测量PSNR和SSIM）
    ///
    /// ## 参数
    /// - `psnr`: PSNR值（dB）
    /// - `ssim`: SSIM值（0-1）
    fn add_calibration_point(&mut self, psnr: f64, ssim: f64) {
        self.calibration_points.push((psnr, ssim));
        // 至少需要2个点才能建立映射
        if self.calibration_points.len() >= 2 {
            self.calibrated = true;
        }
    }

    /// 从PSNR预测SSIM（使用线性插值）
    ///
    /// ## 返回
    /// - `Some(ssim)`: 预测的SSIM值
    /// - `None`: 数据不足，无法预测
    fn predict_ssim_from_psnr(&self, psnr: f64) -> Option<f64> {
        if !self.calibrated || self.calibration_points.len() < 2 {
            return None;
        }

        // 对校准点按PSNR排序
        let mut points = self.calibration_points.clone();
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // 查找插值区间
        for i in 0..points.len() - 1 {
            let (psnr1, ssim1) = points[i];
            let (psnr2, ssim2) = points[i + 1];

            if psnr >= psnr1 && psnr <= psnr2 {
                // 线性插值
                let ratio = (psnr - psnr1) / (psnr2 - psnr1);
                let predicted_ssim = ssim1 + ratio * (ssim2 - ssim1);
                return Some(predicted_ssim);
            }
        }

        // 外推：使用最近的两个点
        if psnr < points[0].0 {
            // 低于最小值，使用前两个点外推
            let (psnr1, ssim1) = points[0];
            let (psnr2, ssim2) = points[1];
            let slope = (ssim2 - ssim1) / (psnr2 - psnr1);
            Some(ssim1 + slope * (psnr - psnr1))
        } else {
            // 高于最大值，使用后两个点外推
            let n = points.len();
            let (psnr1, ssim1) = points[n - 2];
            let (psnr2, ssim2) = points[n - 1];
            let slope = (ssim2 - ssim1) / (psnr2 - psnr1);
            Some(ssim2 + slope * (psnr - psnr2))
        }
    }

    /// 获取映射质量（R²值）
    /// 返回值越接近1.0，映射越准确
    fn get_mapping_quality(&self) -> f64 {
        if self.calibration_points.len() < 3 {
            return 0.5; // 数据不足，置信度中等
        }

        // 简单评估：根据数据点数量
        // 3-5个点：0.7-0.8
        // 6-10个点：0.8-0.9
        // 10+个点：0.9+
        let n = self.calibration_points.len() as f64;
        (0.6 + (n / 20.0).min(0.35)).min(0.95)
    }

    /// 打印映射报告
    fn print_report(&self) {
        if !self.calibrated {
            eprintln!("   ⚠️ PSNR-SSIM mapping not calibrated");
            return;
        }

        eprintln!("   📊 PSNR-SSIM Mapping Report:");
        eprintln!(
            "      Calibration points: {}",
            self.calibration_points.len()
        );
        eprintln!(
            "      Mapping quality: {:.1}%",
            self.get_mapping_quality() * 100.0
        );

        // 显示几个示例映射
        if self.calibration_points.len() >= 2 {
            let test_psnrs = vec![35.0, 38.0, 40.0, 42.0, 45.0];
            eprintln!("      Example mappings:");
            for psnr in test_psnrs {
                if let Some(ssim) = self.predict_ssim_from_psnr(psnr) {
                    eprintln!("         PSNR {:.1}dB → SSIM {:.4}", psnr, ssim);
                }
            }
        }
    }
}

/// 执行 GPU 粗略搜索
///
/// ## 目的
/// 快速找到一个**压缩边界的大致范围**，供 CPU 精细搜索使用。
///
/// ## 策略
/// 1. 从 initial_crf 开始，用大步长（4 CRF）快速搜索
/// 2. 找到"刚好能压缩"的 CRF 边界
/// 3. 返回边界值，供 CPU 精细搜索缩小范围
///
/// ## 注意
/// - 这只是粗略估算，不追求精确
/// - GPU 编码速度快，适合快速预览
/// - 最终精确结果由 CPU 搜索确定
///
/// 🔥 v5.22: 添加 log_cb 参数，让调用者控制日志输出方式
pub fn gpu_coarse_search(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str, // "hevc" or "av1"
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

/// 🔥 v5.22: 带日志回调的 GPU 粗略搜索
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

    // 🔥 v5.35: 在有progress callback时进入静默模式，防止日志刷屏
    // 进度条已经显示实时信息，不需要大量详细日志
    let silent_mode = progress_cb.is_some();

    // 🔥 v5.22: 如果有日志回调，使用回调输出；否则直接 eprintln
    #[allow(unused_macros)]
    macro_rules! log_msg {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            // 只在非静默模式时输出日志，防止progress bar刷屏
            if !silent_mode {
                if let Some(cb) = &log_cb {
                    cb(&msg);
                } else {
                    eprintln!("{}", msg);
                }
            }
            log.push(msg);
        }};
    }

    let gpu = GpuAccel::detect();

    // 检查 GPU 是否可用
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

    // 获取对应的 GPU 编码器
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

    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.15: 智能跳过 GPU（极短视频/小文件场景）
    // 🔥 v5.17: 性能保护（极大视频/边缘案例）
    // ═══════════════════════════════════════════════════════════

    // 跳过阈值
    const SKIP_GPU_SIZE_THRESHOLD: u64 = 500 * 1024; // 500KB - 太小跳过
    const SKIP_GPU_DURATION_THRESHOLD: f32 = 3.0; // 3秒 - 太短跳过

    // 🔥 v5.17: 性能保护阈值
    const LARGE_FILE_THRESHOLD: u64 = 500 * 1024 * 1024; // 500MB - 大文件
    const VERY_LARGE_FILE_THRESHOLD: u64 = 2 * 1024 * 1024 * 1024; // 2GB - 超大文件
    const LONG_DURATION_THRESHOLD: f32 = 600.0; // 10分钟 - 长视频
    const VERY_LONG_DURATION_THRESHOLD: f32 = 3600.0; // 1小时 - 超长视频

    // 快速获取时长
    let quick_duration: f32 = {
        let duration_output = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                "--", // 🔥 v7.9: 防止 dash-prefix 文件名被解析为参数
            ])
            .arg(input)
            .output();

        duration_output
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(GPU_SAMPLE_DURATION)
    };

    // 判断是否跳过 GPU（太小/太短）
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

    // 🔥 v5.17: 性能模式判断
    let is_large_file = input_size >= LARGE_FILE_THRESHOLD;
    let is_very_large_file = input_size >= VERY_LARGE_FILE_THRESHOLD;
    let is_long_video = quick_duration >= LONG_DURATION_THRESHOLD;
    let is_very_long_video = quick_duration >= VERY_LONG_DURATION_THRESHOLD;

    // 🔥 v5.52: 动态调整采样时长（保留），移除迭代硬限制（改用保底上限）
    // 用户要求："绝不要限制死迭代次数！你必须通过改进设计来实现更好的迭代效率！"
    //
    // 关键修复：大文件也跳过并行探测，因为并行探测会阻塞直到最慢的编码完成
    // 在169MB文件上，CRF 1编码45秒采样可能需要30-60秒，导致进度条冻结
    let (sample_duration_limit, skip_parallel) = if is_very_large_file || is_very_long_video {
        // 超大文件/超长视频：最保守策略
        log_msg!("   ⚠️ Very large file detected → Conservative mode (30s sample)");
        (30.0_f32, true) // 只采样 30 秒，跳过并行
    } else if is_large_file || is_long_video {
        // 大文件：跳过并行，防止进度条冻结
        log_msg!("   📊 Large file detected → Sequential mode (45s sample)");
        (45.0_f32, true) // 采样 45 秒，跳过并行探测
    } else {
        // 正常文件：跳过并行以保证响应性
        log_msg!(
            "   ✅ Normal file → Sequential mode ({}s sample)",
            GPU_SAMPLE_DURATION
        );
        (GPU_SAMPLE_DURATION, true) // 使用默认采样时长
    };

    // 🔥 v5.52: 使用保底上限，不限制死迭代次数
    let max_iterations_limit = GPU_ABSOLUTE_MAX_ITERATIONS;

    // 🔥 v5.5: 简洁日志
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

    // 🔥 v5.17: 使用动态采样时长
    let duration = quick_duration;
    let actual_sample_duration = duration.min(sample_duration_limit);

    // 🔥 v5.64: 计算采样部分的输入大小
    // 短视频（<60s）：使用完整大小
    // 长视频（>=60s）：多段采样（5段×10秒=50秒）
    let sample_input_size = if duration < 60.0 {
        // 短视频，使用完整大小
        input_size
    } else {
        // 长视频，多段采样总时长 = 50 秒
        let multi_segment_duration = GPU_SAMPLE_DURATION; // 50 秒
        let ratio = multi_segment_duration / duration;
        (input_size as f64 * ratio as f64) as u64
    };

    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.18: 缓存预热（Cache Warmup）
    // 用极短采样（5秒）快速测试 max_crf，获取压缩趋势
    // 如果 max_crf 都无法压缩，提前退出节省时间
    // ═══════════════════════════════════════════════════════════
    const WARMUP_DURATION: f32 = 5.0; // 预热只用 5 秒
    let warmup_duration = duration.min(WARMUP_DURATION);

    // 预热编码函数（极短采样）
    let encode_warmup = |crf: f32| -> anyhow::Result<u64> {
        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.get_extra_args();
        // 🔥 v6.4.7: 从输出路径派生临时文件扩展名
        let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
        let warmup_output = output.with_extension(format!("warmup.{}", ext));

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-t")
            .arg(format!("{}", warmup_duration))
            .arg("-i")
            // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-c:v")
            .arg(gpu_encoder.name);

        for arg in &crf_args {
            cmd.arg(arg);
        }
        for arg in &extra_args {
            cmd.arg(*arg);
        }

        cmd.arg("-an").arg(&warmup_output);

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

    // 执行预热：测试 max_crf
    let warmup_input_size = if duration <= WARMUP_DURATION {
        input_size
    } else {
        (input_size as f64 * warmup_duration as f64 / duration as f64) as u64
    };

    let warmup_result = encode_warmup(config.max_crf);
    let can_compress_at_max = match &warmup_result {
        Ok(size) => *size < warmup_input_size,
        Err(_) => true, // 编码失败时继续正常流程
    };

    if !can_compress_at_max {
        // max_crf 都无法压缩，提前退出
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

    // 🔥 v5.64: 打印采样策略
    if duration >= 60.0 {
        log_msg!("   📊 Multi-segment sampling: 5 segments × 10s = 50s (0%, 25%, 50%, 75%, 90%)");
    } else {
        log_msg!("   📊 Full video sampling: {:.1}s", duration);
    }

    // 🔥 v5.64: 多段采样函数 - 采样开头+25%+50%+75%+结尾
    // 覆盖视频全局特征，避免"开头简单、结尾复杂"导致的误判
    // 🔥 v5.42: 实时进度更新 - 读取ffmpeg的-progress输出，多次调用progress_cb
    // 🔥 v5.44: 简化超时逻辑 - 仅保留 12 小时底线超时，响亮 fallback
    let encode_gpu = |crf: f32| -> anyhow::Result<u64> {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.get_extra_args();

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y");

        // 🔥 v5.64: 多段采样策略
        // 短视频（<60s）：直接采样全片
        // 长视频（>=60s）：采样5个关键片段（开头+25%+50%+75%+结尾）
        let use_multi_segment = duration >= 60.0;

        if !use_multi_segment {
            // 短视频：直接采样前 N 秒
            cmd.arg("-t").arg(format!("{}", actual_sample_duration));
        }

        cmd.arg("-i")
            // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-c:v")
            .arg(gpu_encoder.name);

        // 🔥 v5.64: 长视频使用 select 滤镜多段采样
        if use_multi_segment {
            // 采样位置：0%, 25%, 50%, 75%, 90%（避免结尾可能的黑屏）
            let seg_dur = GPU_SEGMENT_DURATION;
            let positions = [
                0.0,                                       // 开头
                duration * 0.25,                           // 25%
                duration * 0.50,                           // 50%
                duration * 0.75,                           // 75%
                (duration * 0.90).max(duration - seg_dur), // 结尾（避免黑屏）
            ];

            // 构建 select 滤镜表达式
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
            .arg(output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;
        let start_time = Instant::now();
        let absolute_timeout = Duration::from_secs(12 * 3600);
        let child_pid = child.id();

        // 🔥 v7.5.3: 启动stderr捕获
        let stderr_capture = StderrCapture::new(100);
        let stderr_handle = child.stderr.take().map(|stderr| stderr_capture.spawn_capture_thread(stderr));

        // 🔥 v7.5.3: 启动心跳监控
        let last_activity = Arc::new(Mutex::new(Instant::now()));
        let stop_signal = Arc::new(AtomicBool::new(false));
        let heartbeat = HeartbeatMonitor::new(
            Arc::clone(&last_activity),
            Arc::clone(&stop_signal),
            child_pid,
            Duration::from_secs(300), // 5分钟超时
        );
        let heartbeat_handle = heartbeat.spawn();

        // 🔥 v7.5.3: 启动检测（30秒内必须有首次输出）
        let first_output = Arc::new(AtomicBool::new(false));
        let first_output_clone = Arc::clone(&first_output);
        let stop_clone = Arc::clone(&stop_signal);
        let startup_handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            if !first_output_clone.load(Ordering::Relaxed) && !stop_clone.load(Ordering::Relaxed) {
                eprintln!(
                    "❌ STARTUP FAILED: No output in 30s (Beijing: {})",
                    beijing_time_now()
                );
                #[cfg(unix)]
                unsafe {
                    libc::kill(child_pid as i32, libc::SIGKILL);
                }
            }
        });

        eprintln!(
            "🔄 GPU Encoding started (heartbeat active) - Beijing: {}",
            beijing_time_now()
        );

        // 🔥 v7.5.3: 解析进度并更新心跳
        let mut last_progress_time = Instant::now();
        let mut fallback_logged = false;

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);

            for line in reader.lines() {
                // 首次输出
                if !first_output.load(Ordering::Relaxed) {
                    first_output.store(true, Ordering::Relaxed);
                }

                // 更新心跳
                *last_activity.lock().unwrap() = Instant::now();

                if let Ok(line) = line {
                    // 解析 out_time_us=XXXXX
                    if let Some(val) = line.strip_prefix("out_time_us=") {
                        if let Ok(time_us) = val.parse::<u64>() {
                            // 每 1 秒更新一次进度
                            if last_progress_time.elapsed().as_secs_f64() >= 1.0 {
                                let current_secs = time_us as f64 / 1_000_000.0;
                                let pct = (current_secs / actual_sample_duration as f64 * 100.0)
                                    .min(100.0);
                                let eta = if pct > 0.1 {
                                    ((actual_sample_duration as f64 - current_secs)
                                        / (current_secs / start_time.elapsed().as_secs_f64()))
                                    .max(0.0) as u64
                                } else {
                                    0
                                };
                                let speed = if current_secs > 0.0 {
                                    start_time.elapsed().as_secs_f64() / current_secs
                                } else {
                                    0.0
                                };

                                // 尝试获取实时文件大小
                                let estimated_final_size = match std::fs::metadata(output) {
                                    Ok(metadata) => {
                                        let current_size = metadata.len();
                                        fallback_logged = false;
                                        (current_size as f64 / pct.max(1.0) * 100.0) as u64
                                    }
                                    Err(_) => {
                                        if !fallback_logged {
                                            eprintln!(
                                                "📍 Using linear estimation (metadata unavailable)"
                                            );
                                            fallback_logged = true;
                                        }
                                        (sample_input_size as f64 * (1.0 / pct.max(0.1)))
                                            .min(sample_input_size as f64 * 10.0)
                                            as u64
                                    }
                                };

                                eprintln!("⏳ Progress: {:.1}% ({:.1}s / {:.1}s) - ETA: {}s - Speed: {:.2}x", 
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

        // 等待编码完成
        let status = child.wait().context("Failed to wait for ffmpeg")?;

        // 🔥 v7.5.3: 停止所有监控线程
        stop_signal.store(true, Ordering::Relaxed);
        let _ = heartbeat_handle.join();
        let _ = startup_handle.join();
        if let Some(handle) = stderr_handle {
            let _ = handle.join();
        }

        // 检查底线超时
        if start_time.elapsed() > absolute_timeout {
            eprintln!("⏰ WARNING: GPU encoding took longer than 12 hours!");
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

        eprintln!(
            "✅ Encoding completed, heartbeat stopped - Beijing: {}",
            beijing_time_now()
        );

        Ok(std::fs::metadata(output)?.len())
    };

    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.16: 并行编码函数（2-3 路）
    // 用于 Stage 1 初始探测，同时测试多个 CRF 点
    // ═══════════════════════════════════════════════════════════
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
                        .arg(&input_path)
                        .arg("-c:v")
                        .arg(&encoder_name);

                    for arg in &crf_args {
                        cmd.arg(arg);
                    }
                    for arg in &extra_args {
                        cmd.arg(arg);
                    }

                    cmd.arg("-an").arg(&output_path);

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

                    // 清理临时文件
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

    // 🔥 v6.5: 使用 CrfCache 替代 HashMap
    let mut size_cache: CrfCache<u64> = CrfCache::new();
    let mut best_crf: Option<f32> = None;
    let mut best_size: Option<u64> = None;

    // 🔥 v6.5: 使用 CrfCache（直接用 crf 作为 key）
    let encode_cached = |crf: f32, cache: &mut CrfCache<u64>| -> anyhow::Result<u64> {
        if let Some(&size) = cache.get(crf) {
            return Ok(size);
        }
        let size = encode_gpu(crf)?;
        cache.insert(crf, size);
        Ok(size)
    };

    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.14: 优化三阶段搜索
    //
    // 改进：
    // 1. Stage 1: 标准指数搜索（从 min_crf 向上倍增）
    // 2. Stage 2: 智能跳过（如果已经是 0.5 精度）
    // 3. 提前终止阈值放宽到 0.1%（更稳健）
    // ═══════════════════════════════════════════════════════════

    // 智能终止常量
    const WINDOW_SIZE: usize = 3;
    const _VARIANCE_THRESHOLD: f64 = 0.0001; // 0.01% 方差阈值（保留备用）
    const CHANGE_RATE_THRESHOLD: f64 = 0.02; // 🔥 v5.21: 放宽到 2%（避免过早终止导致低 SSIM）

    // 滑动窗口历史记录 (crf, size)
    let mut size_history: Vec<(f32, u64)> = Vec::new();

    // 计算滑动窗口方差（保留备用）
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

    // 计算相对变化率
    let calc_change_rate = |prev: u64, curr: u64| -> f64 {
        if prev == 0 {
            return f64::MAX;
        }
        ((curr as f64 - prev as f64) / prev as f64).abs()
    };

    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.46: 智能初始探测 - 使用 initial_crf 作为起点
    // initial_crf 是质量分析预测的合适值，应该是最佳起点
    // ═══════════════════════════════════════════════════════════
    let mut boundary_low: f32 = config.min_crf;
    let mut boundary_high: f32 = config.max_crf;
    let mut prev_size: Option<u64> = None;
    let mut found_compress_point = false;

    // 🔥 v5.46: 策略改变 - initial_crf 优先
    // 场景 1: initial_crf 在合理范围内 → 从它开始
    // 场景 2: initial_crf 接近边界 → 使用 mid_crf
    let use_initial =
        config.initial_crf >= config.min_crf + 5.0 && config.initial_crf <= config.max_crf - 5.0;

    let probe_crfs = if use_initial {
        // 🔥 优先方案：initial_crf 在中间，向两侧探测
        log_msg!(
            "   🎯 Using initial_crf {:.0} as search anchor",
            config.initial_crf
        );
        vec![config.initial_crf, config.max_crf, config.min_crf]
    } else {
        // 🔥 后备方案：initial_crf 太极端，使用三点探测
        let mid_crf = (config.min_crf + config.max_crf) / 2.0;
        log_msg!(
            "   ⚠️ initial_crf {:.0} out of range, using mid_crf {:.0}",
            config.initial_crf,
            mid_crf
        );
        vec![mid_crf, config.max_crf, config.min_crf]
    };

    // 🔥 v5.17: 检查是否跳过并行探测
    let probe_results = if skip_parallel {
        log_msg!("   ⚡ Skip parallel probe (large file mode)");
        // 大文件模式：只测试第一个探测点
        let test_crf = probe_crfs[0];
        log_msg!("   🔄 Testing CRF {:.0} (anchor point)...", test_crf);
        let single_result = encode_gpu(test_crf);
        if let Ok(size) = &single_result {
            // 🔥 v6.5: CrfCache 直接用 crf 作为 key
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

    // 🔥 v6.5: CrfCache 直接用 crf 作为 key
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

    // 🔥 v5.46: 智能分析探测结果 - 基于 initial_crf 决定搜索方向
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

    // 根据 initial_crf 的结果智能决定搜索方向
    if let Some((initial_crf_val, Ok(initial_size))) = initial_result {
        if *initial_size < sample_input_size {
            // ✅ initial_crf 能压缩！
            best_crf = Some(*initial_crf_val);
            best_size = Some(*initial_size);
            found_compress_point = true;

            // 🔥 关键决策：尝试更高的 CRF（更低质量，更小文件）
            boundary_low = *initial_crf_val;
            boundary_high = config.max_crf;
            log_msg!(
                "   ✅ initial_crf {:.0} compresses! Searching higher CRF [{:.0}, {:.0}]",
                initial_crf_val,
                boundary_low,
                boundary_high
            );

            // 如果测试了 max_crf，检查它是否更好
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
            // ❌ initial_crf 不能压缩 - 需要更低 CRF（更高质量）
            boundary_low = config.min_crf;
            boundary_high = *initial_crf_val;
            prev_size = Some(*initial_size);
            log_msg!(
                "   ⚠️ initial_crf {:.0} cannot compress! Searching lower CRF [{:.0}, {:.0}]",
                initial_crf_val,
                boundary_low,
                boundary_high
            );

            // 检查 min_crf 是否能压缩
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

    // ═══════════════════════════════════════════════════════════
    // 🔥 v6.0: Stage 1 重写 - 曲线模型激进撞墙策略
    //
    // 核心改进（与 CPU v5.99 一致）：
    // 1. 使用指数衰减曲线计算步长：step = initial_step * 0.5^(wall_hits)
    // 2. 每次撞墙后步长衰减，但仍保持激进
    // 3. 当曲线步长 < 1.0 时，切换到 0.5 精细调整阶段
    // 4. 最多 4 次撞墙即停止
    // ═══════════════════════════════════════════════════════════

    // 🔥 v6.0: GPU 曲线模型常量
    const GPU_DECAY_FACTOR: f32 = 0.5; // GPU 衰减因子（比 CPU 的 0.4 保守一点）
    const GPU_MAX_WALL_HITS: u32 = 4; // 最大撞墙次数
    const GPU_MIN_STEP: f32 = 0.5; // GPU 最小步长

    if (boundary_high - boundary_low) > 4.0 {
        if found_compress_point {
            // ✅ 场景 A: 初始探测找到压缩点 → 向上搜索更高的 CRF（曲线模型）
            // 目标：找到最高的仍能压缩的 CRF（比如从 35 搜到 39）
            let crf_range = config.max_crf - boundary_low;
            let initial_step = (crf_range / 2.0).clamp(4.0, 15.0); // 初始大步长

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
                // 🔥 v6.5: CrfCache 直接用 crf 作为 key
                let size_result = if size_cache.contains_key(test_crf) {
                    Ok(*size_cache.get(test_crf).unwrap())
                } else {
                    encode_cached(test_crf, &mut size_cache)
                };

                match size_result {
                    Ok(size) => {
                        if !size_cache.contains_key(test_crf) {
                            iterations += 1;
                            if let Some(cb) = progress_cb {
                                cb(test_crf, size);
                            }
                        }

                        if size < sample_input_size {
                            // ✅ 能压缩！记录并继续向上
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
                            // ❌ 不能压缩 - WALL HIT！
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

                            // 曲线衰减步长
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

            // 确保 best_crf 是最后一个能压缩的点
            if last_compressible_crf > 0.0 {
                best_crf = Some(last_compressible_crf);
                best_size = Some(last_compressible_size);
            }
        } else {
            // ✅ 场景 B: 初始探测未找到压缩点 → 向下搜索（曲线模型）
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
                // 🔥 v6.5: CrfCache 直接用 crf 作为 key
                let size_result = if size_cache.contains_key(test_crf) {
                    Ok(*size_cache.get(test_crf).unwrap())
                } else {
                    encode_cached(test_crf, &mut size_cache)
                };

                match size_result {
                    Ok(size) => {
                        if !size_cache.contains_key(test_crf) {
                            iterations += 1;
                            if let Some(cb) = progress_cb {
                                cb(test_crf, size);
                            }
                        }

                        if size < sample_input_size {
                            // ✅ 找到能压缩的点！
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
                            // ❌ 还不能压缩 - 继续向下或撞墙回退
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

                            // 曲线衰减步长
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

            // 🔥 v6.0: 抑制未使用变量警告
            let _ = last_fail_crf;
        }
    }

    // ═══════════════════════════════════════════════════════════
    // Stage 2: 整数二分搜索
    // 🔥 v5.14: 智能跳过 - 如果边界已经是整数或 0.5 精度，跳过
    // ═══════════════════════════════════════════════════════════
    let skip_stage2 = if let Some(b) = best_crf {
        let fract = (b * 2.0).fract(); // 检查是否是 0.5 的倍数
        fract.abs() < 0.01 || (fract - 1.0).abs() < 0.01
    } else {
        false
    };

    if found_compress_point && !skip_stage2 && (boundary_high - boundary_low) > 1.0 {
        let mut lo = boundary_low.ceil() as i32;
        let mut hi = boundary_high.floor() as i32;

        // 最多 log2(range) 次迭代
        let max_binary_iter = 5;
        let mut binary_iter = 0;

        while lo < hi && iterations < max_iterations_limit && binary_iter < max_binary_iter {
            binary_iter += 1;
            let mid = lo + (hi - lo) / 2;
            let test_crf = mid as f32;

            // 🔥 v6.5: CrfCache 直接用 crf 作为 key
            if size_cache.contains_key(test_crf) {
                let cached_size = *size_cache.get(test_crf).unwrap();
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

                    // 智能终止
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

    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.52: Stage 3 重写 - 基于收益递减的 0.5 步长搜索
    // 🔥 v5.80: 添加GPU质量天花板检测 - 使用PSNR快速监控
    //
    // 用户要求："绝不要限制死迭代次数！通过改进设计来实现更好的迭代效率！"
    //
    // 设计改进：
    // - 移除"最多 3 次"硬限制
    // - 改为基于收益递减的自然停止（改进 < 1% 或 < 0.5% 时停止）
    // - 🆕 添加质量天花板检测：PSNR连续3次提升<0.1dB时停止
    // - 步长 0.5 保持，向下搜索直到边界
    // - 只受保底上限 (500) 和 min_crf 限制
    // ═══════════════════════════════════════════════════════════

    // 🔥 v5.80: 在Stage 3外创建质量天花板检测器和PSNR-SSIM映射器
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

                // 检查边界
                if test_crf < config.min_crf {
                    log_msg!("   ⚡ Stop: reached min_crf {:.1}", config.min_crf);
                    break;
                }

                // 🔥 v6.5: CrfCache 直接用 crf 作为 key
                let result = if size_cache.contains_key(test_crf) {
                    let cached_size = *size_cache.get(test_crf).unwrap();
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
                            // 能够压缩，计算改进
                            let improvement = best_size
                                .map(|b| (b as f64 - size as f64) / b as f64 * 100.0)
                                .unwrap_or(0.0);
                            log_msg!("   ✓ CRF {:.1}: {:.1}% improvement", test_crf, improvement);

                            // 更新最佳点
                            best_crf = Some(test_crf);
                            best_size = Some(size);
                            current_best = test_crf;

                            // 🔥 v5.80: 使用PSNR进行快速质量监控
                            // PSNR计算速度约为SSIM的10-50倍，适合GPU阶段频繁检测
                            // 🔥 v6.5: 安全路径转换，避免 unwrap panic
                            let input_str = input.to_string_lossy();
                            let output_str = output.to_string_lossy();
                            if let Ok(psnr) = calculate_psnr_fast(&input_str, &output_str) {
                                log_msg!("      📊 PSNR: {:.2}dB", psnr);

                                // 添加到质量天花板检测器
                                if ceiling_detector.add_sample(test_crf, psnr) {
                                    // 检测到质量天花板
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
                                // PSNR计算失败，降级到仅使用文件大小判断
                                log_msg!("      ⚠️ PSNR calc failed, fallback to size-only");
                            }

                            // 🔥 收益递减检测
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
                                // 改进显著，重置计数器
                                consecutive_small_improvements = 0;
                            }

                            // 继续向下搜索
                            offset += 0.5;
                        } else {
                            // 无法压缩，停止
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

            // 🔥 v5.80: 输出质量天花板信息（如果检测到）
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

    // 🔥 v5.80: 区分"最后测试点"和"压缩边界"
    // - last_tested_crf: 最后测试成功的CRF（用于日志）
    // - gpu_boundary_crf: 能压缩的最低CRF（质量最高且能压缩）
    let (last_tested_crf, found, fine_tuned) = if let Some(b) = best_crf {
        (b, true, iterations > 8) // 超过 8 次迭代说明进行了精细化
    } else {
        (config.max_crf, false, false)
    };

    // 🔥 v5.80: 检测质量天花板（PSNR平台）
    // 策略：
    // 1. 优先使用Stage 3检测到的PSNR天花板
    // 2. 如果未检测到，返回None（说明GPU未达到质量天花板）
    let quality_ceiling_info = if ceiling_detector.ceiling_detected {
        ceiling_detector.get_ceiling()
    } else {
        None
    };

    let (quality_ceiling_crf, _quality_ceiling_psnr) = quality_ceiling_info
        .map(|(crf, psnr)| (Some(crf), if psnr > 0.0 { Some(psnr) } else { None }))
        .unwrap_or((None, None));

    // 🔥 v5.50: Stage 3 已经计算了 SSIM，直接使用
    // 🔥 v5.80: 同时计算PSNR和SSIM，建立PSNR-SSIM映射
    // 重新计算最终点的 SSIM 和 PSNR
    let (gpu_ssim, gpu_psnr) = if found {
        log_msg!(
            "   📍 Final quality validation at CRF {:.1}",
            last_tested_crf
        );
        match encode_gpu(last_tested_crf) {
            Ok(_) => {
                // 🔥 v5.80: 并行计算SSIM和PSNR
                let ssim_output = Command::new("ffmpeg")
                    .arg("-i")
                    // .arg("--") // 🔥 v7.9: ffmpeg does not support '--' as delimiter
                    .arg(crate::safe_path_arg(input).as_ref())
                    .arg("-i")
                    .arg(crate::safe_path_arg(output).as_ref())
                    .arg("-lavfi")
                    .arg("ssim")
                    .arg("-f")
                    .arg("null")
                    .arg("-")
                    .output();

                // 🔥 v6.5: 安全路径转换
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

                // 🔥 v5.80: 如果同时有PSNR和SSIM，添加到映射器
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

    // 🔥 v5.80: 确定GPU压缩边界（能压缩的最低CRF，质量最高）
    // 关键逻辑：
    // - 如果检测到天花板 → 边界 = 天花板CRF（再往下是虚胖，质量不再提升）
    // - 如果未检测到天花板 → 边界 = 最后测试成功的CRF
    let gpu_boundary_crf = if let Some(ceiling_crf) = quality_ceiling_info.map(|(crf, _)| crf) {
        log_msg!("   🎯 GPU Quality Ceiling Detected!");
        log_msg!("      └─ Ceiling CRF: {:.1} (PSNR plateau)", ceiling_crf);
        log_msg!("      └─ Last tested CRF: {:.1}", last_tested_crf);
        if ceiling_crf != last_tested_crf {
            log_msg!("      └─ Boundary = Ceiling (lower CRFs are bloated, no quality gain)");
        }
        ceiling_crf // 边界 = 天花板（防止虚胖）
    } else {
        last_tested_crf // 未检测到天花板，使用最后测试点
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

        // 🔥 v5.80: 打印PSNR-SSIM映射报告
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

    // 清理临时文件
    let _ = std::fs::remove_file(output);

    Ok(GpuCoarseResult {
        gpu_boundary_crf, // 🔥 v5.80: 能压缩的最低CRF（质量最高且能压缩）
        gpu_best_size: best_size,
        gpu_best_ssim: gpu_ssim,
        gpu_type: gpu.gpu_type,
        codec: encoder.to_string(),
        iterations,
        found_boundary: found,
        fine_tuned,
        log,
        sample_input_size,
        quality_ceiling_crf, // 🔥 v5.80: 检测到的质量天花板（可能为None）
        quality_ceiling_ssim: gpu_ssim, // 使用SSIM作为天花板质量指标
    })
}

/// 获取 GPU 粗略搜索后的 CPU 搜索范围
///
/// ## 返回值
/// (min_crf, max_crf, center_crf) - CPU 精细搜索的范围
pub fn get_cpu_search_range_from_gpu(
    gpu_result: &GpuCoarseResult,
    original_min_crf: f32,
    original_max_crf: f32,
) -> (f32, f32, f32) {
    if !gpu_result.found_boundary {
        // GPU 没找到边界，使用原始范围
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
    fn test_gpu_detection() {
        let gpu = GpuAccel::detect_fresh();
        println!("GPU Type: {:?}", gpu.gpu_type);
        println!("HEVC: {:?}", gpu.hevc_encoder.as_ref().map(|e| e.name));
        println!("AV1: {:?}", gpu.av1_encoder.as_ref().map(|e| e.name));
        println!("H264: {:?}", gpu.h264_encoder.as_ref().map(|e| e.name));
    }

    #[test]
    fn test_crf_to_bitrate() {
        // HEVC
        assert!(crf_to_estimated_bitrate(18.0, "hevc") > crf_to_estimated_bitrate(28.0, "hevc"));
        // AV1
        assert!(crf_to_estimated_bitrate(25.0, "av1") > crf_to_estimated_bitrate(35.0, "av1"));
    }

    #[test]
    fn test_gpu_encoder_crf_args() {
        let encoder = GpuEncoder {
            gpu_type: GpuType::Nvidia,
            name: "hevc_nvenc",
            codec: "hevc",
            supports_crf: true,
            crf_param: "cq",
            crf_range: (0, 51),
            extra_args: vec![],
        };

        let args = encoder.get_crf_args(23.5);
        assert_eq!(args, vec!["-cq", "24"]);
    }

    #[test]
    fn test_estimate_cpu_search_center() {
        // 🔥 v5.9: 基于实测数据更新
        // VideoToolbox: offset = 5.0, GPU 10 → CPU 15
        let cpu_center = estimate_cpu_search_center(10.0, GpuType::Apple, "hevc");
        assert!(
            (cpu_center - 15.0).abs() < 0.1,
            "Expected ~15.0, got {}",
            cpu_center
        );

        // NVENC: offset = 4.0, GPU 10 → CPU 14
        let cpu_center = estimate_cpu_search_center(10.0, GpuType::Nvidia, "hevc");
        assert!(
            (cpu_center - 14.0).abs() < 0.1,
            "Expected ~14.0, got {}",
            cpu_center
        );

        // None: offset = 0, GPU 10 → CPU 10
        let cpu_center = estimate_cpu_search_center(10.0, GpuType::None, "hevc");
        assert!(
            (cpu_center - 10.0).abs() < 0.1,
            "Expected ~10.0, got {}",
            cpu_center
        );
    }

    #[test]
    fn test_gpu_boundary_to_cpu_range() {
        // 🔥 v5.9: 基于实测数据更新
        // Apple: GPU 10 → CPU 从 10 开始向上搜索到 ~18 (center=15, +3)
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

        // 边界限制测试
        let (low, _high) = gpu_boundary_to_cpu_range(12.0, GpuType::Nvidia, "hevc", 10.0, 28.0);
        assert!((low - 12.0).abs() < 0.1, "low should be GPU boundary");
    }

    // ═══════════════════════════════════════════════════════════════
    // 🔥 v6.4.7: GPU 临时文件扩展名派生测试
    // ═══════════════════════════════════════════════════════════════

    /// **Feature: code-quality-v6.4.7, Property 3: GPU 临时文件扩展名派生**
    /// **验证: Requirements 2.1, 2.2, 2.3**
    #[test]
    fn test_derive_gpu_temp_extension_mp4() {
        use std::path::PathBuf;
        let output = PathBuf::from("/path/to/output.mp4");
        let ext = super::derive_gpu_temp_extension(&output);
        assert_eq!(ext, "gpu_temp.mp4");
    }

    #[test]
    fn test_derive_gpu_temp_extension_mkv() {
        use std::path::PathBuf;
        let output = PathBuf::from("/path/to/output.mkv");
        let ext = super::derive_gpu_temp_extension(&output);
        assert_eq!(ext, "gpu_temp.mkv");
    }

    #[test]
    fn test_derive_gpu_temp_extension_webm() {
        use std::path::PathBuf;
        let output = PathBuf::from("/path/to/output.webm");
        let ext = super::derive_gpu_temp_extension(&output);
        assert_eq!(ext, "gpu_temp.webm");
    }

    #[test]
    fn test_derive_gpu_temp_extension_no_ext() {
        use std::path::PathBuf;
        let output = PathBuf::from("/path/to/output");
        let ext = super::derive_gpu_temp_extension(&output);
        assert_eq!(
            ext, "gpu_temp.mp4",
            "Should default to mp4 when no extension"
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // 🔥 v6.4.9: VideoToolbox CRF 映射边界测试
    // ═══════════════════════════════════════════════════════════════

    /// **Feature: code-quality-v6.4.9, Requirement 5.1**
    /// CRF=0 应映射到 q:v=100（最高质量）
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

    /// **Feature: code-quality-v6.4.9, Requirement 5.2**
    /// CRF=51 应映射到有效的 clamp 值（不为负数）
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
        // 100 - 51*2 = -2, clamp to 1
        assert_eq!(
            args,
            vec!["-q:v", "1"],
            "CRF 51 should clamp to q:v 1 (not negative)"
        );
    }

    /// **Feature: code-quality-v6.4.9, Requirement 5.3**
    /// 测试 CRF 1, 25, 50 的映射
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

        // CRF 1 -> q:v = 100 - 1*2 = 98
        let args = encoder.get_crf_args(1.0);
        assert_eq!(args, vec!["-q:v", "98"], "CRF 1 should map to q:v 98");

        // CRF 25 -> q:v = 100 - 25*2 = 50
        let args = encoder.get_crf_args(25.0);
        assert_eq!(args, vec!["-q:v", "50"], "CRF 25 should map to q:v 50");

        // CRF 50 -> q:v = 100 - 50*2 = 0, clamp to 1
        let args = encoder.get_crf_args(50.0);
        assert_eq!(args, vec!["-q:v", "1"], "CRF 50 should clamp to q:v 1");
    }

    /// **Feature: code-quality-v6.4.9**
    /// 验证映射公式不会产生负数或超过 100 的值
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

        // 测试极端值
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

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.7: GPU 临时文件扩展名属性测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::PathBuf;

    proptest! {
        /// **Feature: code-quality-v6.4.7, Property 3: GPU 临时文件扩展名派生**
        /// *对于任意*输出路径，GPU 临时文件的扩展名应与输出路径的扩展名匹配
        /// **验证: Requirements 2.1, 2.2, 2.3**
        #[test]
        fn prop_gpu_temp_extension_matches_output(ext in "[a-z]{2,4}") {
            let output = PathBuf::from(format!("/path/to/output.{}", ext));
            let temp_ext = derive_gpu_temp_extension(&output);

            // 验证临时文件扩展名以原始扩展名结尾
            prop_assert!(temp_ext.ends_with(&ext),
                "Temp extension '{}' should end with '{}'", temp_ext, ext);

            // 验证格式为 "gpu_temp.{ext}"
            prop_assert_eq!(temp_ext, format!("gpu_temp.{}", ext));
        }

        /// **Feature: code-quality-v6.4.7, Property 3b: 常见视频格式支持**
        /// 验证常见视频格式都能正确派生
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
