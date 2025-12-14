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

/// GPU Stage 1 粗略搜索最大迭代次数
pub const GPU_STAGE1_MAX_ITERATIONS: u32 = 8;

/// GPU Stage 2 精细搜索最大迭代次数
pub const GPU_STAGE2_MAX_ITERATIONS: u32 = 15;

/// GPU Stage 3 超精细搜索最大迭代次数
pub const GPU_STAGE3_MAX_ITERATIONS: u32 = 20;

/// GPU 配置默认最大迭代次数
pub const GPU_MAX_ITERATIONS: u32 = 10;

/// GPU 默认最小 CRF
/// 🔥 v5.7: VideoToolbox 需要更低 CRF (更高 q:v) 才能达到高 SSIM
/// CRF 1 → q:v 98 → SSIM ~0.99
/// CRF 10 → q:v 80 → SSIM ~0.85 (不够高!)
pub const GPU_DEFAULT_MIN_CRF: f32 = 1.0;

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
            if self.hevc_encoder.is_some() {
                eprintln!("      • HEVC: {}", self.hevc_encoder.as_ref().unwrap().name);
            }
            if self.av1_encoder.is_some() {
                eprintln!("      • AV1: {}", self.av1_encoder.as_ref().unwrap().name);
            }
            if self.h264_encoder.is_some() {
                eprintln!("      • H.264: {}", self.h264_encoder.as_ref().unwrap().name);
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
            0.3   // 高复杂度: 仅 +0.3
        } else if potential > 0.7 {
            -0.2  // 低复杂度: 仅 -0.2
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
    let cpu_low = estimate_cpu_search_center_dynamic(gpu_low, gpu_type, codec, compression_potential);
    let cpu_high = estimate_cpu_search_center_dynamic(gpu_high, gpu_type, codec, compression_potential);

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
    max_crf: f32
) -> (f32, f32) {
    let cpu_center = estimate_cpu_search_center(gpu_boundary, gpu_type, codec);
    
    // 🔥 v5.9: 修正方向
    // CPU 从 GPU 边界开始，向上搜索
    let cpu_low = gpu_boundary.max(min_crf);  // 从 GPU 边界开始
    let cpu_high = (cpu_center + 3.0).min(max_crf);  // 向上扩展
    
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
            GpuType::Apple => (5.0, 0.5),      // 🔥 v5.33: 精细uncertainty=0.5（±0.5CRF）
            GpuType::Nvidia => (3.8, 0.3),     // NVENC 更精确的offset和较小uncertainty
            GpuType::IntelQsv => (3.5, 0.3),   // QSV 效率较好，更小uncertainty
            GpuType::AmdAmf => (4.8, 0.5),     // AMF 效率较低
            GpuType::Vaapi => (3.8, 0.4),      // VAAPI 效率中等
            GpuType::None => (0.0, 0.0),       // 无 GPU
        };
        Self { gpu_type, codec: "hevc", offset, uncertainty }
    }
    
    /// 获取 AV1 编码器的 CRF 映射
    /// 🔥 v5.33: 精细化offset校准
    pub fn av1(gpu_type: GpuType) -> Self {
        let (offset, uncertainty) = match gpu_type {
            GpuType::Apple => (0.0, 0.0),      // VideoToolbox 不支持 AV1
            GpuType::Nvidia => (3.8, 0.4),     // NVENC AV1 更精确的offset
            GpuType::IntelQsv => (3.5, 0.3),   // QSV AV1 效率较好
            GpuType::AmdAmf => (4.5, 0.5),     // AMF AV1 效率较低
            GpuType::Vaapi => (3.8, 0.4),      // VAAPI AV1 效率中等
            GpuType::None => (0.0, 0.0),       // 无 GPU
        };
        Self { gpu_type, codec: "av1", offset, uncertainty }
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
        let low = gpu_crf.max(min_crf);  // 从 GPU 边界开始
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
        eprintln!("   📊 GPU/CPU CRF Mapping ({} - {}):", self.gpu_type, self.codec.to_uppercase());
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
        eprintln!("      • CPU offset: +{:.1} (CPU needs higher CRF for same compression)", self.offset);
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
    encoder: &str,  // "hevc" or "av1"
    input_size: u64,
    config: &GpuCoarseConfig,
    progress_cb: Option<&dyn Fn(f32, u64)>,
) -> anyhow::Result<GpuCoarseResult> {
    gpu_coarse_search_with_log(input, output, encoder, input_size, config, progress_cb, None)
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
    use std::process::Command;
    use anyhow::{Context, bail};
    
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
                gpu_best_ssim: None,
                gpu_type: gpu.gpu_type,
                codec: encoder.to_string(),
                iterations: 0,
                found_boundary: false,
                fine_tuned: false,
                log,
            });
        }
    };
    
    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.15: 智能跳过 GPU（极短视频/小文件场景）
    // 🔥 v5.17: 性能保护（极大视频/边缘案例）
    // ═══════════════════════════════════════════════════════════
    
    // 跳过阈值
    const SKIP_GPU_SIZE_THRESHOLD: u64 = 500 * 1024;  // 500KB - 太小跳过
    const SKIP_GPU_DURATION_THRESHOLD: f32 = 3.0;     // 3秒 - 太短跳过
    
    // 🔥 v5.17: 性能保护阈值
    const LARGE_FILE_THRESHOLD: u64 = 500 * 1024 * 1024;  // 500MB - 大文件
    const VERY_LARGE_FILE_THRESHOLD: u64 = 2 * 1024 * 1024 * 1024;  // 2GB - 超大文件
    const LONG_DURATION_THRESHOLD: f32 = 600.0;  // 10分钟 - 长视频
    const VERY_LONG_DURATION_THRESHOLD: f32 = 3600.0;  // 1小时 - 超长视频
    
    // 快速获取时长
    let quick_duration: f32 = {
        let duration_output = Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
            .arg(input)
            .output();
        
        duration_output
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(GPU_SAMPLE_DURATION)
    };
    
    // 判断是否跳过 GPU（太小/太短）
    let skip_gpu = input_size < SKIP_GPU_SIZE_THRESHOLD || quick_duration < SKIP_GPU_DURATION_THRESHOLD;
    
    if skip_gpu {
        let reason = if input_size < SKIP_GPU_SIZE_THRESHOLD {
            format!("file too small ({:.1}KB < 500KB)", input_size as f64 / 1024.0)
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
        });
    }
    
    // 🔥 v5.17: 性能模式判断
    let is_large_file = input_size >= LARGE_FILE_THRESHOLD;
    let is_very_large_file = input_size >= VERY_LARGE_FILE_THRESHOLD;
    let is_long_video = quick_duration >= LONG_DURATION_THRESHOLD;
    let is_very_long_video = quick_duration >= VERY_LONG_DURATION_THRESHOLD;
    
    // 🔥 v5.35: 动态调整采样时长和迭代限制
    // 关键修复：大文件也跳过并行探测，因为并行探测会阻塞直到最慢的编码完成
    // 在169MB文件上，CRF 1编码45秒采样可能需要30-60秒，导致进度条冻结
    let (sample_duration_limit, max_iterations_limit, skip_parallel) = if is_very_large_file || is_very_long_video {
        // 超大文件/超长视频：最保守策略
        log_msg!("   ⚠️ Very large file detected → Conservative mode");
        (30.0_f32, 6_u32, true)  // 只采样 30 秒，最多 6 次迭代，跳过并行
    } else if is_large_file || is_long_video {
        // 🔥 v5.35: 大文件也跳过并行，防止进度条冻结
        log_msg!("   📊 Large file detected → Sequential probing mode");
        (45.0_f32, 8_u32, true)  // 采样 45 秒，最多 8 次迭代，跳过并行探测
    } else {
        // 正常文件：可以使用并行探测，但也建议跳过以保证响应性
        log_msg!("   ✅ Normal file → Sequential probing mode");
        (GPU_SAMPLE_DURATION, GPU_STAGE1_MAX_ITERATIONS, true)  // 🔥 v5.35: 全部跳过并行，保证实时进度显示
    };
    
    // 🔥 v5.5: 简洁日志
    log_msg!("GPU搜索 ({}, {:.2}MB, {:.1}s)", gpu.gpu_type, input_size as f64 / 1024.0 / 1024.0, quick_duration);
    log.push(format!("GPU: {} | Input: {:.2}MB | Duration: {:.1}s", gpu.gpu_type, input_size as f64 / 1024.0 / 1024.0, quick_duration));
    
    let mut iterations = 0u32;
    
    // 🔥 v5.17: 使用动态采样时长
    let duration = quick_duration;
    let actual_sample_duration = duration.min(sample_duration_limit);
    
    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.18: 缓存预热（Cache Warmup）
    // 用极短采样（5秒）快速测试 max_crf，获取压缩趋势
    // 如果 max_crf 都无法压缩，提前退出节省时间
    // ═══════════════════════════════════════════════════════════
    const WARMUP_DURATION: f32 = 5.0;  // 预热只用 5 秒
    let warmup_duration = duration.min(WARMUP_DURATION);
    
    // 预热编码函数（极短采样）
    let encode_warmup = |crf: f32| -> anyhow::Result<u64> {
        let crf_args = gpu_encoder.get_crf_args(crf);
        let extra_args = gpu_encoder.get_extra_args();
        let warmup_output = output.with_extension("warmup.mp4");
        
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-t").arg(format!("{}", warmup_duration))
            .arg("-i").arg(input)
            .arg("-c:v").arg(gpu_encoder.name);
        
        for arg in &crf_args {
            cmd.arg(arg);
        }
        for arg in &extra_args {
            cmd.arg(*arg);
        }
        
        cmd.arg("-an")
            .arg(&warmup_output);
        
        let result = cmd.output().context("Failed to run warmup encode")?;
        let size = if result.status.success() {
            std::fs::metadata(&warmup_output).map(|m| m.len()).unwrap_or(0)
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
        Err(_) => true,  // 编码失败时继续正常流程
    };
    
    if !can_compress_at_max {
        // max_crf 都无法压缩，提前退出
        log_msg!("   ⚡ Warmup: max_crf={:.0} cannot compress → skip GPU search", config.max_crf);
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
        });
    }
    log_msg!("   🔥 Warmup: max_crf={:.0} can compress → continue search", config.max_crf);
    
    // 🔥 v5.5: 简洁 - 不打印采样信息，直接开始搜索
    
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
    
    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.16: 并行编码函数（2-3 路）
    // 用于 Stage 1 初始探测，同时测试多个 CRF 点
    // ═══════════════════════════════════════════════════════════
    let encode_parallel = |crfs: &[f32]| -> Vec<(f32, anyhow::Result<u64>)> {
        use std::thread;
        
        let handles: Vec<_> = crfs.iter().enumerate().map(|(i, &crf)| {
            let crf_args = gpu_encoder.get_crf_args(crf);
            let extra_args: Vec<String> = gpu_encoder.get_extra_args().iter().map(|s| s.to_string()).collect();
            let input_path = input.to_path_buf();
            let output_path = output.with_extension(format!("tmp{}.mp4", i));
            let encoder_name = gpu_encoder.name.to_string();
            let sample_dur = actual_sample_duration;
            
            thread::spawn(move || {
                let mut cmd = Command::new("ffmpeg");
                cmd.arg("-y")
                    .arg("-t").arg(format!("{}", sample_dur))
                    .arg("-i").arg(&input_path)
                    .arg("-c:v").arg(&encoder_name);
                
                for arg in &crf_args {
                    cmd.arg(arg);
                }
                for arg in &extra_args {
                    cmd.arg(arg);
                }
                
                cmd.arg("-an")
                    .arg(&output_path);
                
                let result = cmd.output();
                
                let size = match result {
                    Ok(out) if out.status.success() => {
                        std::fs::metadata(&output_path).map(|m| m.len()).map_err(|e| anyhow::anyhow!("{}", e))
                    }
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        Err(anyhow::anyhow!("GPU encoding failed: {}", stderr.lines().last().unwrap_or("unknown")))
                    }
                    Err(e) => Err(anyhow::anyhow!("{}", e)),
                };
                
                // 清理临时文件
                let _ = std::fs::remove_file(&output_path);
                
                (crf, size)
            })
        }).collect();
        
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (0.0, Err(anyhow::anyhow!("thread panic"))))).collect()
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
    
    // 🔥 v5.5: 不打印采样大小
    
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
    // 🔥 v5.14: 优化三阶段搜索
    // 
    // 改进：
    // 1. Stage 1: 标准指数搜索（从 min_crf 向上倍增）
    // 2. Stage 2: 智能跳过（如果已经是 0.5 精度）
    // 3. 提前终止阈值放宽到 0.1%（更稳健）
    // ═══════════════════════════════════════════════════════════
    
    // 智能终止常量
    const WINDOW_SIZE: usize = 3;
    const VARIANCE_THRESHOLD: f64 = 0.0001;    // 0.01% 方差阈值
    const CHANGE_RATE_THRESHOLD: f64 = 0.02;   // 🔥 v5.21: 放宽到 2%（避免过早终止导致低 SSIM）
    
    // 滑动窗口历史记录 (crf, size)
    let mut size_history: Vec<(f32, u64)> = Vec::new();
    
    // 计算滑动窗口方差
    let calc_window_variance = |history: &[(f32, u64)], input_size: u64| -> f64 {
        if history.len() < WINDOW_SIZE { return f64::MAX; }
        let recent: Vec<f64> = history.iter()
            .rev()
            .take(WINDOW_SIZE)
            .map(|(_, s)| *s as f64 / input_size as f64)
            .collect();
        let mean = recent.iter().sum::<f64>() / recent.len() as f64;
        recent.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / recent.len() as f64
    };
    
    // 计算相对变化率
    let calc_change_rate = |prev: u64, curr: u64| -> f64 {
        if prev == 0 { return f64::MAX; }
        ((curr as f64 - prev as f64) / prev as f64).abs()
    };
    
    // ═══════════════════════════════════════════════════════════
    // 🔥 v5.16: 并行初始探测（可选）
    // 同时测试 3 个关键点：min_crf, mid_crf, max_crf
    // 快速确定搜索区间，减少后续迭代
    // ═══════════════════════════════════════════════════════════
    let mut boundary_low: f32 = config.min_crf;
    let mut boundary_high: f32 = config.max_crf;
    let mut prev_size: Option<u64> = None;
    let mut found_compress_point = false;

    // 🔥 v5.17: 并行探测 3 个关键点（大文件时跳过）
    // 🔥 v5.35: 改变探测顺序 - 从mid_crf开始，避免很慢的min_crf编码
    let mid_crf = (config.min_crf + config.max_crf) / 2.0;
    let probe_crfs = [mid_crf, config.max_crf, config.min_crf];  // 改变顺序：mid → max → min

    // 🔥 v5.17: 检查是否跳过并行探测
    let probe_results = if skip_parallel {
        log_msg!("   ⚡ Skip parallel probe (large file mode)");
        // 大文件模式：从mid_crf开始，避免很慢的min_crf
        log_msg!("   🔄 Testing CRF {:.0} (mid-point)...", mid_crf);
        let single_result = encode_gpu(mid_crf);
        if let Ok(size) = &single_result {
            let key = (mid_crf * 10.0).round() as i32;
            size_cache.insert(key, *size);
            iterations += 1;
            size_history.push((mid_crf, *size));
            if let Some(cb) = progress_cb { cb(mid_crf, *size); }
        }
        vec![(mid_crf, single_result)]
    } else {
        log_msg!("   🚀 Parallel probe: CRF {:.0}, {:.0}, {:.0}", probe_crfs[0], probe_crfs[1], probe_crfs[2]);
        encode_parallel(&probe_crfs)
    };
    
    // 处理并行结果（非跳过模式时）
    if !skip_parallel {
        for (crf, result) in &probe_results {
            if let Ok(size) = result {
                let key = (*crf * 10.0).round() as i32;
                size_cache.insert(key, *size);
                iterations += 1;
                size_history.push((*crf, *size));
                if let Some(cb) = progress_cb { cb(*crf, *size); }
            }
        }
    }
    
    // 分析并行结果，确定搜索区间
    let min_result = probe_results.iter().find(|(c, _)| (*c - config.min_crf).abs() < 0.1);
    let mid_result = probe_results.iter().find(|(c, _)| (*c - mid_crf).abs() < 0.1);
    let max_result = probe_results.iter().find(|(c, _)| (*c - config.max_crf).abs() < 0.1);
    
    // 根据并行结果快速定位边界
    if let Some((_, Ok(min_size))) = min_result {
        if *min_size < sample_input_size {
            // min_crf 就能压缩！最佳情况
            best_crf = Some(config.min_crf);
            best_size = Some(*min_size);
            boundary_high = config.min_crf;
            found_compress_point = true;
            log_msg!("   ⚡ Parallel: min_crf compresses! Best case.");
        } else if let Some((_, Ok(mid_size))) = mid_result {
            if *mid_size < sample_input_size {
                // mid_crf 能压缩，边界在 [min, mid]
                boundary_low = config.min_crf;
                boundary_high = mid_crf;
                best_crf = Some(mid_crf);
                best_size = Some(*mid_size);
                found_compress_point = true;
                prev_size = Some(*min_size);
                log_msg!("   ⚡ Parallel: boundary in [{:.0}, {:.0}]", boundary_low, boundary_high);
            } else if let Some((_, Ok(max_size))) = max_result {
                if *max_size < sample_input_size {
                    // max_crf 能压缩，边界在 [mid, max]
                    boundary_low = mid_crf;
                    boundary_high = config.max_crf;
                    best_crf = Some(config.max_crf);
                    best_size = Some(*max_size);
                    found_compress_point = true;
                    prev_size = Some(*mid_size);
                    log_msg!("   ⚡ Parallel: boundary in [{:.0}, {:.0}]", boundary_low, boundary_high);
                } else {
                    // 即使 max_crf 也无法压缩
                    log_msg!("   ⚠️ Parallel: cannot compress even at max CRF");
                    prev_size = Some(*max_size);
                }
            }
        }
    }
    
    // ═══════════════════════════════════════════════════════════
    // Stage 1: 指数搜索（如果并行探测未完全确定边界）
    // 🔥 v5.17: 使用动态迭代限制
    // ═══════════════════════════════════════════════════════════
    if !found_compress_point && (boundary_high - boundary_low) > 4.0 {
        // 并行探测未找到压缩点，继续指数搜索
        let mut step: f32 = 1.0;

        while iterations < max_iterations_limit && !found_compress_point {
            let test_crf = (boundary_low + step).min(config.max_crf);
            
            let key = (test_crf * 10.0).round() as i32;
            if size_cache.contains_key(&key) {
                // 已有缓存，检查结果
                let cached_size = *size_cache.get(&key).unwrap();
                if cached_size < sample_input_size {
                    boundary_high = test_crf;
                    best_crf = Some(test_crf);
                    best_size = Some(cached_size);
                    found_compress_point = true;
                } else {
                    boundary_low = test_crf;
                    prev_size = Some(cached_size);
                }
                step *= 2.0;
                if test_crf >= config.max_crf { break; }
                continue;
            }
            
            match encode_cached(test_crf, &mut size_cache) {
                Ok(size) => {
                    iterations += 1;
                    size_history.push((test_crf, size));
                    if let Some(cb) = progress_cb { cb(test_crf, size); }
                    
                    // 智能终止检测
                    let variance = calc_window_variance(&size_history, sample_input_size);
                    let change_rate = prev_size.map(|p| calc_change_rate(p, size)).unwrap_or(f64::MAX);
                    
                    if size < sample_input_size {
                        // 找到能压缩的点！
                        boundary_high = test_crf;
                        best_crf = Some(test_crf);
                        best_size = Some(size);
                        found_compress_point = true;
                        
                        // 智能终止
                        if variance < VARIANCE_THRESHOLD && size_history.len() >= WINDOW_SIZE {
                            log_msg!("   ⚡ Stage1 early stop: variance {:.6}", variance);
                        }
                        if change_rate < CHANGE_RATE_THRESHOLD && prev_size.is_some() {
                            log_msg!("   ⚡ Stage1 early stop: Δ{:.3}%", change_rate * 100.0);
                        }
                        break;  // 找到压缩点就停
                    } else {
                        // 还不能压缩，继续向上
                        boundary_low = test_crf;
                        prev_size = Some(size);
                        step *= 2.0;  // 指数增长
                    }
                }
                Err(_) => break,
            }
            
            if test_crf >= config.max_crf { break; }
        }
    }
    
    // ═══════════════════════════════════════════════════════════
    // Stage 2: 整数二分搜索
    // 🔥 v5.14: 智能跳过 - 如果边界已经是整数或 0.5 精度，跳过
    // ═══════════════════════════════════════════════════════════
    let skip_stage2 = if let Some(b) = best_crf {
        let fract = (b * 2.0).fract();  // 检查是否是 0.5 的倍数
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
        
        while lo < hi && iterations < GPU_STAGE2_MAX_ITERATIONS && binary_iter < max_binary_iter {
            binary_iter += 1;
            let mid = lo + (hi - lo) / 2;
            let test_crf = mid as f32;
            
            let key = (test_crf * 10.0).round() as i32;
            if size_cache.contains_key(&key) {
                let cached_size = *size_cache.get(&key).unwrap();
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
                    if let Some(cb) = progress_cb { cb(test_crf, size); }

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
    // Stage 3: 自适应精细化 O(1) - 0.5 精度探测
    // GPU 只到 0.5 精度，0.1 交给 CPU
    // ═══════════════════════════════════════════════════════════
    if let Some(fine) = best_crf {
        // 只测试 -0.5 和 -1.0 两个点（自适应：如果 -0.5 不行就停）
        for &offset in &[0.5_f32, 1.0] {
            let test_crf = fine - offset;
            if test_crf < config.min_crf || iterations >= GPU_STAGE3_MAX_ITERATIONS {
                break;
            }
            
            let key = (test_crf * 10.0).round() as i32;
            if size_cache.contains_key(&key) {
                let cached_size = *size_cache.get(&key).unwrap();
                if cached_size < sample_input_size {
                    best_crf = Some(test_crf);
                    best_size = Some(cached_size);
                } else {
                    break;  // 自适应：不能压缩就停
                }
                continue;
            }

            match encode_cached(test_crf, &mut size_cache) {
                Ok(size) => {
                    iterations += 1;
                    if let Some(cb) = progress_cb { cb(test_crf, size); }

                    if size < sample_input_size {
                        best_crf = Some(test_crf);
                        best_size = Some(size);
                        
                        // 智能终止
                        if let Some(prev) = prev_size {
                            let rate = calc_change_rate(prev, size);
                            if rate < CHANGE_RATE_THRESHOLD {
                                log_msg!("   ⚡ Stage3 early stop: Δ{:.3}%", rate * 100.0);
                                break;
                            }
                        }
                        prev_size = Some(size);
                    } else {
                        break;  // 自适应：不能压缩就停
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
    
    // 🔥 v5.6: 计算 GPU 最优点的 SSIM（评估 GPU 质量上限）
    let gpu_ssim = if found {
        // 重新编码最优点以计算 SSIM
        log_msg!("   📍 GPU Stage 4: SSIM validation at best CRF {:.1}", final_boundary);
        match encode_gpu(final_boundary) {
            Ok(_) => {
                // 计算 SSIM
                let ssim_output = Command::new("ffmpeg")
                    .arg("-i").arg(input)
                    .arg("-i").arg(output)
                    .arg("-lavfi").arg("ssim")
                    .arg("-f").arg("null")
                    .arg("-")
                    .output();
                
                match ssim_output {
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        // 解析 SSIM: "SSIM Y:0.998990 ... All:0.968472"
                        if let Some(line) = stderr.lines().find(|l| l.contains("SSIM") && l.contains("All:")) {
                            if let Some(all_pos) = line.find("All:") {
                                let after_all = &line[all_pos + 4..];
                                if let Some(space_pos) = after_all.find(' ') {
                                    if let Ok(ssim) = after_all[..space_pos].parse::<f64>() {
                                        log_msg!("      📊 GPU SSIM: {:.6} (ceiling ~0.97)", ssim);
                                        Some(ssim)
                                    } else { None }
                                } else if let Ok(ssim) = after_all.trim().parse::<f64>() {
                                    log_msg!("      📊 GPU SSIM: {:.6} (ceiling ~0.97)", ssim);
                                    Some(ssim)
                                } else { None }
                            } else { None }
                        } else { None }
                    }
                    Err(_) => None,
                }
            }
            Err(_) => None,
        }
    } else {
        None
    };
    
    log_msg!("   ═══════════════════════════════════════════════════");
    if found {
        log_msg!("   📊 GPU Best CRF: {:.1}", final_boundary);
        if let Some(size) = best_size {
            let ratio = size as f64 / sample_input_size as f64 * 100.0;
            log_msg!("   📊 GPU Best Size: {:.1}% of input", ratio);
        }
        if let Some(ssim) = gpu_ssim {
            let quality_hint = if ssim >= 0.97 { "🟢 Near ceiling" } 
                              else if ssim >= 0.95 { "🟡 Good" } 
                              else { "🟠 Below expected" };
            log_msg!("   📊 GPU Best SSIM: {:.6} {}", ssim, quality_hint);
        }
        let mapping = match encoder {
            "hevc" => CrfMapping::hevc(gpu.gpu_type),
            "av1" => CrfMapping::av1(gpu.gpu_type),
            _ => CrfMapping::hevc(gpu.gpu_type),
        };
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
        gpu_best_ssim: gpu_ssim,
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
        // 🔥 v5.9: 基于实测数据更新
        // VideoToolbox: offset = 5.0, GPU 10 → CPU 15
        let cpu_center = estimate_cpu_search_center(10.0, GpuType::Apple, "hevc");
        assert!((cpu_center - 15.0).abs() < 0.1, "Expected ~15.0, got {}", cpu_center);
        
        // NVENC: offset = 4.0, GPU 10 → CPU 14
        let cpu_center = estimate_cpu_search_center(10.0, GpuType::Nvidia, "hevc");
        assert!((cpu_center - 14.0).abs() < 0.1, "Expected ~14.0, got {}", cpu_center);
        
        // None: offset = 0, GPU 10 → CPU 10
        let cpu_center = estimate_cpu_search_center(10.0, GpuType::None, "hevc");
        assert!((cpu_center - 10.0).abs() < 0.1, "Expected ~10.0, got {}", cpu_center);
    }
    
    #[test]
    fn test_gpu_boundary_to_cpu_range() {
        // 🔥 v5.9: 基于实测数据更新
        // Apple: GPU 10 → CPU 从 10 开始向上搜索到 ~18 (center=15, +3)
        let (low, high) = gpu_boundary_to_cpu_range(10.0, GpuType::Apple, "hevc", 8.0, 28.0);
        assert!((low - 10.0).abs() < 0.1, "low={} should be ~10.0 (GPU boundary)", low);
        assert!(high >= 15.0 && high <= 22.0, "high={} should be in [15, 22]", high);
        
        // 边界限制测试
        let (low, _high) = gpu_boundary_to_cpu_range(12.0, GpuType::Nvidia, "hevc", 10.0, 28.0);
        assert!((low - 12.0).abs() < 0.1, "low should be GPU boundary");
    }
}
