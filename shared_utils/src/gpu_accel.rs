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

use std::process::Command;
use std::sync::OnceLock;

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.3: 全局常量 - 避免硬编码
// ═══════════════════════════════════════════════════════════════

/// GPU 采样时长（秒）- 用于长视频的快速边界估算
pub const GPU_SAMPLE_DURATION: f32 = 60.0;

/// GPU 粗略搜索步长
pub const GPU_COARSE_STEP: f32 = 2.0;

/// GPU 粗略搜索最大迭代次数
pub const GPU_MAX_ITERATIONS: u32 = 10;

/// GPU 默认最小 CRF
pub const GPU_DEFAULT_MIN_CRF: f32 = 10.0;

/// GPU 默认最大 CRF
pub const GPU_DEFAULT_MAX_CRF: f32 = 40.0;

/// GPU 加速检测结果（全局缓存）
static GPU_ACCEL: OnceLock<GpuAccel> = OnceLock::new();

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
    /// 🔥 v4.14: VideoToolbox 质量映射修正
    /// - libx265 CRF: 0=无损, 51=最差 (常用范围 18-28)
    /// - VideoToolbox -q:v: 1=最高质量, 100=最低质量 (0 不可用)
    /// - 映射公式: q:v = max(1, crf * 1.5) 更激进映射
    ///   - CRF 10 → q:v 15 (高质量)
    ///   - CRF 18 → q:v 27 (常用质量)
    ///   - CRF 28 → q:v 42 (可接受质量)
    pub fn get_crf_args(&self, crf: f32) -> Vec<String> {
        if self.supports_crf {
            // 🔥 v4.14: VideoToolbox 更激进的质量映射
            let quality_value = if self.gpu_type == GpuType::Apple {
                // VideoToolbox: 使用更激进的映射以获得更高 SSIM
                // q:v 1 是最高质量 (0 会导致错误)
                // 映射: CRF * 1.5，最小值为 1
                (crf * 1.5).clamp(1.0, 100.0)
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
            vec![
                "-b:v".to_string(),
                format!("{}k", bitrate),
            ]
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
            eprintln!("🔍 Detecting GPU acceleration...");
            let result = Self::detect_internal();
            if result.enabled {
                eprintln!("   ✅ GPU: {} detected", result.gpu_type);
                if result.hevc_encoder.is_some() {
                    eprintln!("      • HEVC: {}", result.hevc_encoder.as_ref().unwrap().name);
                }
                if result.av1_encoder.is_some() {
                    eprintln!("      • AV1: {}", result.av1_encoder.as_ref().unwrap().name);
                }
                if result.h264_encoder.is_some() {
                    eprintln!("      • H.264: {}", result.h264_encoder.as_ref().unwrap().name);
                }
            } else {
                eprintln!("   ⚠️ No GPU acceleration available, using CPU encoding");
            }
            result
        })
    }

    /// 强制重新检测（不使用缓存）
    pub fn detect_fresh() -> GpuAccel {
        Self::detect_internal()
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
                    crf_param: "q:v",  // VideoToolbox 使用 -q:v
                    crf_range: (0, 100),  // 0=最高质量, 100=最低
                    extra_args: vec![
                        "-profile:v", "main",
                        "-tag:v", "hvc1",  // Apple 兼容标签
                    ],
                })
            } else {
                None
            },
            av1_encoder: None,  // VideoToolbox 不支持 AV1
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
                    crf_param: "cq",  // NVENC 使用 -cq (Constant Quality)
                    crf_range: (0, 51),
                    extra_args: vec![
                        "-preset", "p4",  // 平衡质量和速度
                        "-tune", "hq",
                        "-rc", "vbr",
                        "-profile:v", "main",
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
                    extra_args: vec![
                        "-preset", "p4",
                        "-tune", "hq",
                        "-rc", "vbr",
                    ],
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
                        "-preset", "p4",
                        "-tune", "hq",
                        "-rc", "vbr",
                        "-profile:v", "high",
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
                    extra_args: vec![
                        "-preset", "medium",
                        "-profile:v", "main",
                    ],
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
                    extra_args: vec![
                        "-preset", "medium",
                        "-profile:v", "high",
                    ],
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
                    crf_param: "qp_i",  // AMF 使用 QP
                    crf_range: (0, 51),
                    extra_args: vec![
                        "-quality", "quality",
                        "-profile:v", "main",
                    ],
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
                    extra_args: vec![
                        "-quality", "quality",
                        "-profile:v", "high",
                    ],
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
                    extra_args: vec![
                        "-vaapi_device", "/dev/dri/renderD128",
                        "-profile:v", "main",
                    ],
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
                    extra_args: vec![
                        "-vaapi_device", "/dev/dri/renderD128",
                        "-profile:v", "high",
                    ],
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
                .filter(|line| line.starts_with(" V"))  // 视频编码器
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
        .arg("-f").arg("lavfi")
        .arg("-i").arg("nullsrc=s=64x64:d=0.1")
        .arg("-c:v").arg(encoder)
        .arg("-frames:v").arg("1")
        .arg("-f").arg("null")
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
        "hevc" => 5000,  // 5 Mbps 基准
        "av1" => 4000,   // 4 Mbps 基准
        "h264" => 8000,  // 8 Mbps 基准
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
// 🔥 v5.0: GPU → CPU 压缩边界估算
// ═══════════════════════════════════════════════════════════════

/// GPU 压缩边界到 CPU 压缩边界的估算
/// 
/// ## 背景
/// GPU 硬件编码器（NVENC, VideoToolbox, QSV 等）压缩效率低于 CPU 软件编码器：
/// - 相同 CRF 下，GPU 输出文件更大（压缩效率低）
/// - 质量排序：x264/x265 > QSV > NVENC > VCE (AMD)
/// - 差异程度取决于内容复杂度、preset 等因素
/// 
/// ## 映射目的
/// GPU 粗略搜索找到的"压缩边界"（刚好能压缩的 CRF）需要转换为 CPU 的等效边界：
/// - GPU 在 CRF=20 刚好能压缩 → CPU 在更低 CRF（如 16-18）就能达到相同大小
/// - 因为 CPU 效率更高，相同 CRF 下文件更小
/// 
/// ## 策略
/// 返回一个**估算的 CPU 搜索中心点**，实际边界由 CPU 精细搜索确定。
/// 这只是缩小搜索范围的提示，不是精确映射。
/// 
/// ## 注意
/// - 这不是精确的 CRF 转换，只是搜索范围的估算
/// - 实际差异取决于内容、preset、编码器版本等
/// - CPU 精细搜索会找到真正的边界
pub fn estimate_cpu_search_center(gpu_boundary: f32, gpu_type: GpuType, _codec: &str) -> f32 {
    // GPU 效率低 → 相同文件大小需要更高 CRF
    // 反过来：GPU 边界 CRF → CPU 可以用更低 CRF 达到相同大小
    // 
    // 估算：CPU 边界 ≈ GPU 边界 - offset
    // offset 取决于 GPU 类型（效率差异）
    let offset = match gpu_type {
        GpuType::Apple => {
            // VideoToolbox 效率相对较好（Apple 优化）
            2.0
        }
        GpuType::Nvidia => {
            // NVENC 效率中等
            3.0
        }
        GpuType::IntelQsv => {
            // QSV 效率较好
            2.5
        }
        GpuType::AmdAmf => {
            // AMF 效率较低
            3.5
        }
        GpuType::Vaapi => {
            // VAAPI 效率中等
            3.0
        }
        GpuType::None => {
            // 无 GPU，不需要偏移
            0.0
        }
    };
    
    // CPU 边界估算 = GPU 边界 - offset（更低 CRF = 更高质量）
    // 但不能低于合理范围
    (gpu_boundary - offset).max(1.0)
}

/// 计算 CPU 搜索范围
/// 
/// 基于 GPU 粗略边界，返回 CPU 精细搜索的范围 (low, high)
/// 
/// ## 策略
/// - 以估算的 CPU 边界为中心
/// - 扩展 ±4 CRF 作为安全边界（覆盖不确定性）
/// - 确保不超出 min_crf/max_crf 限制
pub fn gpu_boundary_to_cpu_range(
    gpu_boundary: f32, 
    gpu_type: GpuType, 
    codec: &str, 
    min_crf: f32, 
    max_crf: f32
) -> (f32, f32) {
    let cpu_center = estimate_cpu_search_center(gpu_boundary, gpu_type, codec);
    
    // 扩展范围：±4 CRF 作为安全边界
    // 因为 GPU/CPU 差异不确定，需要足够的搜索空间
    let margin = 4.0;
    let cpu_low = (cpu_center - margin).max(min_crf);
    let cpu_high = (cpu_center + margin).min(max_crf);
    
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
}

/// GPU/CPU CRF 映射表
/// 
/// ## 背景
/// GPU 和 CPU 编码器对 CRF 的解释不同：
/// - GPU CRF 10 可能产生的文件大小 ≈ CPU CRF 15 的文件大小
/// - 这是因为 GPU 编码器压缩效率较低
/// 
/// ## 映射方向
/// - `gpu_to_cpu`: GPU CRF → 等效 CPU CRF（用于搜索范围估算）
/// - `cpu_to_gpu`: CPU CRF → 等效 GPU CRF（用于预览）
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
    /// GPU CRF → CPU CRF 偏移量（CPU = GPU - offset）
    /// 正值表示 CPU 效率更高（相同质量需要更低 CRF）
    pub offset: f32,
    /// 映射的不确定性范围（±）
    pub uncertainty: f32,
}

impl CrfMapping {
    /// 获取 HEVC 编码器的 CRF 映射
    pub fn hevc(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple => (2.0, 1.5),      // VideoToolbox 效率较好
            GpuType::Nvidia => (3.0, 2.0),     // NVENC 效率中等
            GpuType::IntelQsv => (2.5, 1.5),   // QSV 效率较好
            GpuType::AmdAmf => (3.5, 2.5),     // AMF 效率较低
            GpuType::Vaapi => (3.0, 2.0),      // VAAPI 效率中等
            GpuType::None => (0.0, 0.0),       // 无 GPU
        };
        Self { gpu_type, codec: "hevc", offset, uncertainty }
    }
    
    /// 获取 AV1 编码器的 CRF 映射
    pub fn av1(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple => (0.0, 0.0),      // VideoToolbox 不支持 AV1
            GpuType::Nvidia => (4.0, 2.5),     // NVENC AV1 效率较低
            GpuType::IntelQsv => (3.5, 2.0),   // QSV AV1 效率中等
            GpuType::AmdAmf => (4.5, 3.0),     // AMF AV1 效率较低
            GpuType::Vaapi => (4.0, 2.5),      // VAAPI AV1 效率中等
            GpuType::None => (0.0, 0.0),       // 无 GPU
        };
        Self { gpu_type, codec: "av1", offset, uncertainty }
    }
    
    /// GPU CRF → 等效 CPU CRF
    /// 
    /// 返回 (center, low, high) 三元组：
    /// - center: 估算的 CPU CRF 中心点
    /// - low: 搜索范围下限（更高质量）
    /// - high: 搜索范围上限（更低质量）
    pub fn gpu_to_cpu_range(&self, gpu_crf: f32, min_crf: f32, max_crf: f32) -> (f32, f32, f32) {
        let center = (gpu_crf - self.offset).max(min_crf);
        let low = (center - self.uncertainty).max(min_crf);
        let high = (center + self.uncertainty).min(max_crf);
        (center, low, high)
    }
    
    /// CPU CRF → 等效 GPU CRF（用于预览）
    pub fn cpu_to_gpu(&self, cpu_crf: f32) -> f32 {
        cpu_crf + self.offset
    }
    
    /// 打印映射信息
    pub fn print_mapping_info(&self) {
        eprintln!("   📊 GPU/CPU CRF Mapping ({} - {}):", self.gpu_type, self.codec.to_uppercase());
        eprintln!("      • GPU 60s sampling + step=2 → accurate boundary");
        eprintln!("      • CPU offset: {:.1} (GPU CRF - {:.1} = CPU CRF)", self.offset, self.offset);
        eprintln!("      • Uncertainty: ±{:.1} CRF", self.uncertainty);
        eprintln!("      • 💡 CPU fine-tunes within GPU-guided range");
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
pub fn gpu_coarse_search(
    input: &std::path::Path,
    output: &std::path::Path,
    encoder: &str,  // "hevc" or "av1"
    input_size: u64,
    config: &GpuCoarseConfig,
) -> anyhow::Result<GpuCoarseResult> {
    use std::process::Command;
    use anyhow::{Context, bail};
    
    let mut log = Vec::new();
    
    macro_rules! log_msg {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            eprintln!("{}", msg);
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
            gpu_type: GpuType::None,
            codec: encoder.to_string(),
            iterations: 0,
            found_boundary: false,
            fine_tuned: false,
            log,
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
            log_msg!("   ║  ⚠️  FALLBACK: No GPU encoder for {}!              ║", encoder.to_uppercase());
            log_msg!("   ║  Skipping GPU coarse search, using CPU-only mode          ║");
            log_msg!("   ║  This may take longer but results will be accurate        ║");
            log_msg!("   ╚═══════════════════════════════════════════════════════════╝");
            return Ok(GpuCoarseResult {
                gpu_boundary_crf: config.initial_crf,
                gpu_best_size: None,
                gpu_type: gpu.gpu_type,
                codec: encoder.to_string(),
                iterations: 0,
                found_boundary: false,
                fine_tuned: false,
                log,
            });
        }
    };
    
    log_msg!("🚀 GPU Fine Search v5.4 ({} - {})", gpu.gpu_type, encoder.to_uppercase());
    log_msg!("   📁 Input: {} bytes ({:.2} MB)", input_size, input_size as f64 / 1024.0 / 1024.0);
    log_msg!("   🎯 Goal: Find compression boundary (step={:.0})", config.step);
    log_msg!("   ═══════════════════════════════════════════════════");
    
    // 打印 CRF 映射信息
    let mapping = match encoder {
        "hevc" => CrfMapping::hevc(gpu.gpu_type),
        "av1" => CrfMapping::av1(gpu.gpu_type),
        _ => CrfMapping::hevc(gpu.gpu_type),
    };
    mapping.print_mapping_info();
    log_msg!("   ═══════════════════════════════════════════════════");
    
    let mut iterations = 0u32;
    
    // 🔥 v5.3: GPU 采样使用全局常量，更精确的边界估算
    // 对于短视频（<60秒），编码整个视频
    // 对于长视频（>60秒），只编码前 60 秒来估算压缩边界
    
    // 🔥 v5.3: 获取视频时长，智能处理短视频
    let duration: f32 = {
        let duration_output = Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
            .arg(input)
            .output();
        
        duration_output
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(GPU_SAMPLE_DURATION)
    };
    
    // 实际采样时长（短视频使用完整时长）
    let actual_sample_duration = duration.min(GPU_SAMPLE_DURATION);
    
    if duration < GPU_SAMPLE_DURATION {
        log_msg!("   ⚠️ Short video ({:.1}s < {:.0}s), using full duration for GPU sampling", duration, GPU_SAMPLE_DURATION);
    } else {
        log_msg!("   💡 GPU samples first {:.0}s of {:.1}s (accurate estimation)", actual_sample_duration, duration);
    }
    
    // 快速编码函数（GPU）- 只编码前 N 秒
    let encode_gpu = |crf: f32| -> anyhow::Result<u64> {
        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.get_extra_args();
        
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-t").arg(format!("{}", actual_sample_duration))  // 🔥 使用实际采样时长
            .arg("-i").arg(input)
            .arg("-c:v").arg(gpu_encoder.name);
        
        for arg in &crf_args {
            cmd.arg(arg);
        }
        for arg in &extra_args {
            cmd.arg(*arg);
        }
        
        cmd.arg("-an")  // 忽略音频，加速
            .arg(output);
        
        let result = cmd.output().context("Failed to run ffmpeg")?;
        
        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            bail!("GPU encoding failed: {}", stderr.lines().last().unwrap_or("unknown error"));
        }
        
        Ok(std::fs::metadata(output)?.len())
    };
    
    // 🔥 v5.3: 计算采样部分的输入大小（按比例估算）
    let sample_input_size = if duration <= GPU_SAMPLE_DURATION {
        // 短视频，使用完整大小
        input_size
    } else {
        // 长视频，按比例计算采样部分的预期大小
        let ratio = actual_sample_duration / duration;
        (input_size as f64 * ratio as f64) as u64
    };
    
    log_msg!("   📊 Sample input size: {} bytes (for comparison)", sample_input_size);
    
    // 缓存已测试的 CRF 结果
    let mut size_cache: std::collections::HashMap<i32, u64> = std::collections::HashMap::new();
    let mut best_crf: Option<f32> = None;
    let mut best_size: Option<u64> = None;
    
    // 带缓存的编码函数
    let encode_cached = |crf: f32, cache: &mut std::collections::HashMap<i32, u64>| -> anyhow::Result<u64> {
        let key = (crf * 10.0).round() as i32;
        if let Some(&size) = cache.get(&key) {
            return Ok(size);
        }
        let size = encode_gpu(crf)?;
        cache.insert(key, size);
        Ok(size)
    };
    
    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.4: GPU 三阶段精细化搜索
    // ═══════════════════════════════════════════════════════════
    
    // Stage 1: 粗略搜索 (step=4) 找大致边界
    log_msg!("   📍 GPU Stage 1: Coarse search (step=4)");
    let mut coarse_boundary: Option<f32> = None;
    let mut test_crf = config.max_crf;
    
    while test_crf >= config.min_crf && iterations < 8 {
        log_msg!("   🔄 GPU CRF {:.0}...", test_crf);
        match encode_cached(test_crf, &mut size_cache) {
            Ok(size) => {
                iterations += 1;
                let ratio = size as f64 / sample_input_size as f64 * 100.0;
                if size < sample_input_size {
                    coarse_boundary = Some(test_crf);
                    best_crf = Some(test_crf);
                    best_size = Some(size);
                    log_msg!("      ✅ {:.1}% - Compresses", ratio);
                    test_crf -= 4.0;
                } else {
                    log_msg!("      ❌ {:.1}% - Too large", ratio);
                    break;
                }
            }
            Err(e) => {
                log_msg!("      ⚠️ Error: {}", e);
                break;
            }
        }
    }
    
    // Stage 2: 精细搜索 (step=1) 在边界附近
    if let Some(coarse) = coarse_boundary {
        log_msg!("   📍 GPU Stage 2: Fine search around CRF {:.0} (step=1)", coarse);
        
        // 向下探索（更高质量）
        for offset in [1.0_f32, 2.0, 3.0] {
            let test = coarse - offset;
            if test < config.min_crf || iterations >= 15 { break; }
            
            let key = (test * 10.0).round() as i32;
            if size_cache.contains_key(&key) { continue; }
            
            log_msg!("   🔄 GPU CRF {:.0}...", test);
            match encode_cached(test, &mut size_cache) {
                Ok(size) => {
                    iterations += 1;
                    let ratio = size as f64 / sample_input_size as f64 * 100.0;
                    if size < sample_input_size {
                        best_crf = Some(test);
                        best_size = Some(size);
                        log_msg!("      ✅ {:.1}% - New best!", ratio);
                    } else {
                        log_msg!("      ❌ {:.1}% - Too large, stop", ratio);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    
    // Stage 3: 超精细搜索 (step=0.5) 找 GPU 最优点
    if let Some(fine) = best_crf {
        log_msg!("   📍 GPU Stage 3: Ultra-fine search around CRF {:.1} (step=0.5)", fine);
        
        for offset in [0.5_f32, 1.0, 1.5, 2.0] {
            let test = fine - offset;
            if test < config.min_crf || iterations >= 20 { break; }
            
            let key = (test * 10.0).round() as i32;
            if size_cache.contains_key(&key) { continue; }
            
            log_msg!("   🔄 GPU CRF {:.1}...", test);
            match encode_cached(test, &mut size_cache) {
                Ok(size) => {
                    iterations += 1;
                    let ratio = size as f64 / sample_input_size as f64 * 100.0;
                    if size < sample_input_size {
                        best_crf = Some(test);
                        best_size = Some(size);
                        log_msg!("      ✅ {:.1}% - New best!", ratio);
                    } else {
                        log_msg!("      ❌ {:.1}% - Too large, stop", ratio);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    
    // 确定最终结果
    let (final_boundary, found, fine_tuned) = if let Some(b) = best_crf {
        (b, true, iterations > 8)  // 超过 8 次迭代说明进行了精细化
    } else {
        (config.max_crf, false, false)
    };
    
    log_msg!("   ═══════════════════════════════════════════════════");
    if found {
        log_msg!("   📊 GPU Best CRF: {:.1}", final_boundary);
        if let Some(size) = best_size {
            let ratio = size as f64 / sample_input_size as f64 * 100.0;
            log_msg!("   📊 GPU Best Size: {:.1}% of input", ratio);
        }
        let (cpu_center, cpu_low, cpu_high) = mapping.gpu_to_cpu_range(final_boundary, config.min_crf, config.max_crf);
        log_msg!("   📊 CPU Search Range: [{:.1}, {:.1}] (center: {:.1})", cpu_low, cpu_high, cpu_center);
    } else {
        log_msg!("   ⚠️ No compression boundary found (file may be already compressed)");
    }
    log_msg!("   📈 GPU Iterations: {} (fine-tuned: {})", iterations, if fine_tuned { "yes" } else { "no" });
    
    // 清理临时文件
    let _ = std::fs::remove_file(output);
    
    Ok(GpuCoarseResult {
        gpu_boundary_crf: final_boundary,
        gpu_best_size: best_size,
        gpu_type: gpu.gpu_type,
        codec: encoder.to_string(),
        iterations,
        found_boundary: found,
        fine_tuned,
        log,
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
    
    mapping.gpu_to_cpu_range(gpu_result.gpu_boundary_crf, original_min_crf, original_max_crf)
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
        // VideoToolbox: offset = 2.0
        let cpu_center = estimate_cpu_search_center(20.0, GpuType::Apple, "hevc");
        assert!((cpu_center - 18.0).abs() < 0.1, "Expected ~18.0, got {}", cpu_center);
        
        // NVENC: offset = 3.0
        let cpu_center = estimate_cpu_search_center(20.0, GpuType::Nvidia, "hevc");
        assert!((cpu_center - 17.0).abs() < 0.1, "Expected ~17.0, got {}", cpu_center);
        
        // None: offset = 0
        let cpu_center = estimate_cpu_search_center(20.0, GpuType::None, "hevc");
        assert!((cpu_center - 20.0).abs() < 0.1, "Expected ~20.0, got {}", cpu_center);
        
        // 边界情况：不能低于 1.0
        let cpu_center = estimate_cpu_search_center(2.0, GpuType::Nvidia, "hevc");
        assert!(cpu_center >= 1.0, "Should not go below 1.0");
    }
    
    #[test]
    fn test_gpu_boundary_to_cpu_range() {
        // Apple: center = 20 - 2 = 18, range = [14, 22]
        let (low, high) = gpu_boundary_to_cpu_range(20.0, GpuType::Apple, "hevc", 10.0, 28.0);
        assert!(low >= 10.0 && low <= 18.0, "low={} should be in [10, 18]", low);
        assert!(high >= 18.0 && high <= 28.0, "high={} should be in [18, 28]", high);
        
        // 边界限制测试
        let (low, _high) = gpu_boundary_to_cpu_range(12.0, GpuType::Nvidia, "hevc", 10.0, 28.0);
        assert!(low >= 10.0, "low should respect min_crf");
    }
}
