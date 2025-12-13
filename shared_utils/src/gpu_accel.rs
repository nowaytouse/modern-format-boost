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
}
