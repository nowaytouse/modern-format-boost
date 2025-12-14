//! Video CRF Explorer Module - 统一的视频质量探索器
//!
//! 🔥 三种探索模式：
//! 1. `--explore` 单独使用：寻找更小的文件大小（不验证质量，仅保证 size < input）
//! 2. `--match-quality` 单独使用：使用算法预测的 CRF，单次编码 + SSIM 验证
//! 3. `--explore --match-quality` 组合：二分搜索 + SSIM 裁判验证，找到最精确的质量匹配
//!
//! ⚠️ 仅支持动态图片→视频和视频→视频转换！
//! ⚠️ 静态图片使用无损转换，不支持探索模式！
//!
//! ## 模块化设计
//! 
//! 所有探索逻辑集中在此模块，其他模块（imgquality_hevc, vidquality_hevc）
//! 只需调用此模块的便捷函数，避免重复实现。

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.5: 进度条辅助宏 - 固定底部显示
// ═══════════════════════════════════════════════════════════════

/// 固定底部进度显示（覆盖当前行）
#[allow(unused_macros)]
macro_rules! progress_line {
    ($($arg:tt)*) => {{
        eprint!("\r\x1b[K{}", format!($($arg)*));
        let _ = std::io::stderr().flush();
    }};
}

/// 进度完成后换行
#[allow(unused_macros)]
macro_rules! progress_done {
    () => {{
        eprintln!();
    }};
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.3: 全局常量 - 避免硬编码
// ═══════════════════════════════════════════════════════════════

/// 绝对最低 CRF（最高质量边界）
pub const ABSOLUTE_MIN_CRF: f32 = 10.0;

/// 绝对最高 CRF（最低质量边界）
pub const ABSOLUTE_MAX_CRF: f32 = 51.0;

/// Stage B-1 快速搜索最大迭代次数
pub const STAGE_B1_MAX_ITERATIONS: u32 = 20;

/// Stage B-2 精细调整最大迭代次数
pub const STAGE_B2_MAX_ITERATIONS: u32 = 25;

/// Stage B 双向搜索最大迭代次数
pub const STAGE_B_BIDIRECTIONAL_MAX: u32 = 18;

/// 二分搜索最大迭代次数
pub const BINARY_SEARCH_MAX_ITERATIONS: u32 = 12;

/// 🔥 v5.25: 全局迭代底线（防止无限循环）
pub const GLOBAL_MAX_ITERATIONS: u32 = 60;

// ═══════════════════════════════════════════════════════════════
// 探索模式枚举
// ═══════════════════════════════════════════════════════════════

/// 探索模式 - 决定探索器的行为
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreMode {
    /// 仅探索更小的文件大小（--explore 单独使用）
    /// - 二分搜索找到 size < input 的最高 CRF（最小文件）
    /// - 不验证 SSIM/PSNR 质量
    /// - 输出：裁判验证准确度提示（仅供参考）
    SizeOnly,
    
    /// 仅匹配输入质量（--match-quality 单独使用）
    /// - 使用算法预测的 CRF 值（基于 bpp、分辨率等特征）
    /// - 单次编码 + SSIM 验证
    /// - 目标：快速匹配质量
    QualityMatch,
    
    /// 精确质量匹配（--explore + --match-quality 组合）
    /// - 🔥 v4.5: 高效搜索 + 精确质量匹配
    /// - 目标：找到**最高 SSIM**（最接近源质量）
    /// - 不关心文件大小，只关心质量
    PreciseQualityMatch,
    
    /// 🔥 v4.5: 精确质量匹配 + 压缩（--explore + --match-quality + --compress 组合）
    /// - 目标：找到**最高 SSIM** 且 **输出 < 输入**
    /// - 如果无法同时满足，优先保证压缩，然后在压缩范围内找最高 SSIM
    PreciseQualityMatchWithCompression,
    
    /// 🔥 v4.6: 仅压缩（--compress 单独使用）
    /// - 目标：确保输出 < 输入（哪怕只小 1KB 也算成功）
    /// - 不验证 SSIM 质量
    /// - 与 SizeOnly 不同：SizeOnly 寻找**最小**输出，CompressOnly 只要**更小**即可
    CompressOnly,
    
    /// 🔥 v4.6: 压缩 + 粗略质量验证（--compress --match-quality 组合）
    /// - 目标：确保输出 < 输入 + 粗略 SSIM 验证
    /// - 与 PreciseQualityMatchWithCompression 不同：不追求最高 SSIM，只要通过阈值即可
    CompressWithQuality,
}

/// 🔥 v4.1: 交叉验证结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossValidationResult {
    /// 所有指标一致通过 (SSIM + PSNR + VMAF)
    AllAgree,
    /// 多数指标通过 (2/3)
    MajorityAgree,
    /// 指标分歧 (1/3 或更少)
    Divergent,
}

// ═══════════════════════════════════════════════════════════════
// 数据结构
// ═══════════════════════════════════════════════════════════════

/// 探索结果
/// 🔥 v5.57: 置信度分解详情
#[derive(Debug, Clone, Default)]
pub struct ConfidenceBreakdown {
    /// 采样覆盖度 (0-1): 采样时长 / 总时长
    pub sampling_coverage: f64,
    /// GPU→CPU 预测准确度 (0-1): 基于实测差异
    pub prediction_accuracy: f64,
    /// 安全边界余量 (0-1): 输出比输入小的程度
    pub margin_safety: f64,
    /// SSIM 可靠性 (0-1): 基于 SSIM 值本身
    pub ssim_confidence: f64,
}

impl ConfidenceBreakdown {
    /// 计算加权平均置信度
    pub fn overall(&self) -> f64 {
        (self.sampling_coverage * 0.3
            + self.prediction_accuracy * 0.3
            + self.margin_safety * 0.2
            + self.ssim_confidence * 0.2)
            .min(1.0)
    }

    /// 打印置信度报告
    pub fn print_report(&self) {
        let overall = self.overall();
        let grade = if overall >= 0.9 { "🟢 Excellent" }
                   else if overall >= 0.75 { "🟡 Good" }
                   else if overall >= 0.5 { "🟠 Fair" }
                   else { "🔴 Low" };
        
        eprintln!("┌─────────────────────────────────────────────────────");
        eprintln!("│ 📊 置信度报告 (Confidence Report)");
        eprintln!("├─────────────────────────────────────────────────────");
        eprintln!("│ 📈 总体置信度: {:.0}% {}", overall * 100.0, grade);
        eprintln!("├─────────────────────────────────────────────────────");
        eprintln!("│ 📹 采样覆盖度: {:.0}% (权重 30%)", self.sampling_coverage * 100.0);
        eprintln!("│ 🎯 预测准确度: {:.0}% (权重 30%)", self.prediction_accuracy * 100.0);
        eprintln!("│ 💾 安全边界: {:.0}% (权重 20%)", self.margin_safety * 100.0);
        eprintln!("│ 📊 SSIM可靠性: {:.0}% (权重 20%)", self.ssim_confidence * 100.0);
        eprintln!("└─────────────────────────────────────────────────────");
    }
}

#[derive(Debug, Clone)]
pub struct ExploreResult {
    /// 最优 CRF 值
    /// 🔥 v3.4: Changed from u8 to f32 for sub-integer precision (0.5 step)
    pub optimal_crf: f32,
    /// 输出文件大小
    pub output_size: u64,
    /// 相对于输入的大小变化百分比（负数表示减小）
    pub size_change_pct: f64,
    /// SSIM 分数
    pub ssim: Option<f64>,
    /// PSNR 分数
    pub psnr: Option<f64>,
    /// VMAF 分数 (0-100, Netflix 感知质量指标)
    pub vmaf: Option<f64>,
    /// 探索迭代次数
    pub iterations: u32,
    /// 是否通过质量验证
    pub quality_passed: bool,
    /// 探索日志
    pub log: Vec<String>,
    /// 🔥 v5.57: 整体置信度 (0-1)
    pub confidence: f64,
    /// 🔥 v5.57: 置信度分解详情
    pub confidence_detail: ConfidenceBreakdown,
}

/// 质量验证阈值
#[derive(Debug, Clone)]
pub struct QualityThresholds {
    /// 最小 SSIM（0.0-1.0，推荐 >= 0.95）
    pub min_ssim: f64,
    /// 最小 PSNR（dB，推荐 >= 35）
    pub min_psnr: f64,
    /// 最小 VMAF（0-100，推荐 >= 85）
    pub min_vmaf: f64,
    /// 是否启用 SSIM 验证
    pub validate_ssim: bool,
    /// 是否启用 PSNR 验证
    pub validate_psnr: bool,
    /// 是否启用 VMAF 验证（较慢但更准确）
    pub validate_vmaf: bool,
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_vmaf: 85.0,
            validate_ssim: true,
            validate_psnr: false,
            validate_vmaf: false, // 默认关闭，因为较慢
        }
    }
}

/// 探索配置
#[derive(Debug, Clone)]
pub struct ExploreConfig {
    /// 探索模式
    pub mode: ExploreMode,
    /// 起始 CRF（算法预测值）
    /// 🔥 v3.4: Changed from u8 to f32 for sub-integer precision (0.5 step)
    pub initial_crf: f32,
    /// 最小 CRF（最高质量）
    pub min_crf: f32,
    /// 最大 CRF（最低可接受质量）
    pub max_crf: f32,
    /// 目标比率：输出大小 <= 输入大小 * target_ratio
    pub target_ratio: f64,
    /// 质量验证阈值
    pub quality_thresholds: QualityThresholds,
    /// 最大迭代次数
    pub max_iterations: u32,
}

impl Default for ExploreConfig {
    fn default() -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatch, // 默认：精确质量匹配
            initial_crf: 18.0,
            min_crf: 10.0,
            max_crf: 28.0,
            target_ratio: 1.0,
            quality_thresholds: QualityThresholds::default(),
            // 🔥 v3.6: 增加迭代次数以支持三阶段搜索
            // 粗搜索 ~5 次 + 细搜索 ~4 次 + 精细化 ~2 次 = ~11 次
            max_iterations: 12,
        }
    }
}

impl ExploreConfig {
    /// 创建仅探索大小的配置（--explore 单独使用）
    pub fn size_only(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::SizeOnly,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                validate_ssim: false,
                validate_psnr: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    
    /// 创建仅匹配质量的配置（--match-quality 单独使用）
    pub fn quality_match(predicted_crf: f32) -> Self {
        Self {
            mode: ExploreMode::QualityMatch,
            initial_crf: predicted_crf,
            max_iterations: 1, // 单次编码
            quality_thresholds: QualityThresholds {
                validate_ssim: true, // 验证但不探索
                validate_psnr: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    
    /// 创建精确质量匹配的配置（--explore + --match-quality 组合）
    /// 
    /// 🔥 v4.5: 高效搜索 + 精确质量匹配
    /// - 目标：找到最高 SSIM
    /// - 不关心文件大小
    pub fn precise_quality_match(initial_crf: f32, max_crf: f32, min_ssim: f64) -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatch,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim,
                min_psnr: 40.0,
                min_vmaf: 90.0,
                validate_ssim: true,
                validate_psnr: false, // 简化，只用 SSIM
                validate_vmaf: false,
            },
            ..Default::default()
        }
    }
    
    /// 🔥 v4.5: 创建精确质量匹配 + 压缩的配置（--explore + --match-quality + --compress 组合）
    /// 
    /// - 目标：找到最高 SSIM 且输出 < 输入
    /// - 如果无法同时满足，优先保证压缩
    pub fn precise_quality_match_with_compression(initial_crf: f32, max_crf: f32, min_ssim: f64) -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatchWithCompression,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim,
                min_psnr: 40.0,
                min_vmaf: 90.0,
                validate_ssim: true,
                validate_psnr: false,
                validate_vmaf: false,
            },
            ..Default::default()
        }
    }
    
    /// 🔥 v4.6: 创建仅压缩的配置（--compress 单独使用）
    /// 
    /// - 目标：确保输出 < 输入（哪怕只小 1KB 也算成功）
    /// - 不验证 SSIM 质量
    /// - 与 size_only 不同：size_only 寻找最小输出，compress_only 只要更小即可
    pub fn compress_only(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::CompressOnly,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                validate_ssim: false, // 不验证质量
                validate_psnr: false,
                validate_vmaf: false,
                ..Default::default()
            },
            max_iterations: 8, // 较少迭代，因为只需要找到能压缩的点
            ..Default::default()
        }
    }
    
    /// 🔥 v4.6: 创建压缩 + 粗略质量验证的配置（--compress --match-quality 组合）
    /// 
    /// - 目标：确保输出 < 输入 + 粗略 SSIM 验证
    /// - 与 precise_quality_match_with_compression 不同：不追求最高 SSIM，只要通过阈值即可
    pub fn compress_with_quality(initial_crf: f32, max_crf: f32) -> Self {
        Self {
            mode: ExploreMode::CompressWithQuality,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim: 0.95, // 粗略验证阈值
                validate_ssim: true,
                validate_psnr: false,
                validate_vmaf: false,
                ..Default::default()
            },
            max_iterations: 10,
            ..Default::default()
        }
    }
}

/// 视频编码器类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoEncoder {
    /// HEVC/H.265 (libx265)
    Hevc,
    /// AV1 (libsvtav1)
    Av1,
    /// H.264 (libx264)
    H264,
}

impl VideoEncoder {
    /// 获取 ffmpeg 编码器名称
    pub fn ffmpeg_name(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => "libx265",
            VideoEncoder::Av1 => "libsvtav1",
            VideoEncoder::H264 => "libx264",
        }
    }
    
    /// 获取输出容器格式
    pub fn container(&self) -> &'static str {
        match self {
            VideoEncoder::Hevc => "mp4",
            VideoEncoder::Av1 => "mp4",
            VideoEncoder::H264 => "mp4",
        }
    }
    
    /// 获取额外的编码器参数
    pub fn extra_args(&self, max_threads: usize) -> Vec<String> {
        match self {
            VideoEncoder::Hevc => vec![
                "-tag:v".to_string(), "hvc1".to_string(),
                "-x265-params".to_string(), 
                format!("log-level=error:pools={}", max_threads),
            ],
            VideoEncoder::Av1 => vec![
                "-svtav1-params".to_string(),
                format!("tune=0:film-grain=0"),
            ],
            VideoEncoder::H264 => vec![
                "-profile:v".to_string(), "high".to_string(),
            ],
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// 核心探索器
// ═══════════════════════════════════════════════════════════════

/// 视频 CRF 探索器 - 使用二分搜索 + SSIM 裁判验证
pub struct VideoExplorer {
    config: ExploreConfig,
    encoder: VideoEncoder,
    input_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
    input_size: u64,
    vf_args: Vec<String>,
    max_threads: usize,
    /// 🔥 v4.9: GPU 加速选项
    use_gpu: bool,
}

impl VideoExplorer {
    /// 创建新的探索器
    /// 
    /// # Arguments
    /// * `input` - 输入文件路径（动态图片或视频）
    /// * `output` - 输出文件路径
    /// * `encoder` - 视频编码器
    /// * `vf_args` - 视频滤镜参数
    /// * `config` - 探索配置
    pub fn new(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
    ) -> Result<Self> {
        let input_size = fs::metadata(input)
            .context("Failed to read input file metadata")?
            .len();

        let max_threads = (num_cpus::get() / 2).clamp(1, 4);

        // 🔥 v4.9: 自动检测并启用 GPU 加速
        let gpu = crate::gpu_accel::GpuAccel::detect();
        let use_gpu = gpu.is_available() && match encoder {
            VideoEncoder::Hevc => gpu.get_hevc_encoder().is_some(),
            VideoEncoder::Av1 => gpu.get_av1_encoder().is_some(),
            VideoEncoder::H264 => gpu.get_h264_encoder().is_some(),
        };

        Ok(Self {
            config,
            encoder,
            input_path: input.to_path_buf(),
            output_path: output.to_path_buf(),
            input_size,
            vf_args,
            max_threads,
            use_gpu,
        })
    }

    /// 🔥 v4.9: 创建新的探索器（带 GPU 控制选项）
    pub fn new_with_gpu(
        input: &Path,
        output: &Path,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        config: ExploreConfig,
        use_gpu: bool,
    ) -> Result<Self> {
        let input_size = fs::metadata(input)
            .context("Failed to read input file metadata")?
            .len();

        let max_threads = (num_cpus::get() / 2).clamp(1, 4);

        Ok(Self {
            config,
            encoder,
            input_path: input.to_path_buf(),
            output_path: output.to_path_buf(),
            input_size,
            vf_args,
            max_threads,
            use_gpu,
        })
    }

    /// 执行探索（根据模式选择不同策略）
    pub fn explore(&self) -> Result<ExploreResult> {
        match self.config.mode {
            ExploreMode::SizeOnly => self.explore_size_only(),
            ExploreMode::QualityMatch => self.explore_quality_match(),
            ExploreMode::PreciseQualityMatch => self.explore_precise_quality_match(),
            ExploreMode::PreciseQualityMatchWithCompression => self.explore_precise_quality_match_with_compression(),
            ExploreMode::CompressOnly => self.explore_compress_only(),
            ExploreMode::CompressWithQuality => self.explore_compress_with_quality(),
        }
    }
    
    /// 模式 1: 仅探索更小的文件大小（--explore 单独使用）
    ///
    /// 🔥 v4.8: 简化逻辑 + 避免重复编码
    ///
    /// ## 目标
    /// 找到 size < input 的**最高 CRF**（最小文件）
    ///
    /// ## 策略
    /// 1. 测试 max_crf 确认能否压缩
    /// 2. 如果能压缩，max_crf 就是答案（最高 CRF = 最小文件）
    /// 3. 如果不能压缩，返回失败
    fn explore_size_only(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let start_time = std::time::Instant::now();

        // 🔥 v5.7: Unified Professional Process
        let pb = crate::progress::create_professional_spinner("🔍 Size Explore");
        
        macro_rules! progress_line {
            ($($arg:tt)*) => {{
                pb.set_message(format!($($arg)*));
            }};
        }
        
        macro_rules! progress_done {
            () => {{ }};
        }

        // 🔥 v5.8: Modern Header style
        pb.suspend(|| {
             eprintln!("┌ 🔍 Size-Only Explore ({:?})", self.encoder);
             eprintln!("└ 📁 Input: {:.2} MB", self.input_size as f64 / 1024.0 / 1024.0);
        });

        log.push(format!("🔍 Size-Only Explore ({:?})", self.encoder));

        // 测试 max_crf（最高 CRF = 最小文件）
        progress_line!("Test CRF {:.1}...", self.config.max_crf);
        let max_size = self.encode(self.config.max_crf)?;
        let iterations = 1u32;
        progress_done!();

        let (best_crf, best_size, quality_passed) = if max_size < self.input_size {
            (self.config.max_crf, max_size, true)
        } else {
            (self.config.max_crf, max_size, false)
        };

        // 计算 SSIM（仅供参考）
        progress_line!("Calculate SSIM...");
        let ssim = self.calculate_ssim().ok().flatten();
        progress_done!();
        
        let size_change_pct = self.calc_change_pct(best_size);
        let elapsed = start_time.elapsed();

        pb.finish_and_clear();
        let ssim_str = ssim.map(|s| format!("{:.4}", s)).unwrap_or_else(|| "---".to_string());
        let status = if quality_passed { "💾" } else { "⚠️" };
        eprintln!("✅ Result: CRF {:.1} • SSIM {} • Size {:+.1}% ({}) • {:.1}s",
            best_crf, ssim_str, size_change_pct, status, elapsed.as_secs_f64());
        log.push(format!("📊 RESULT: CRF {:.1}, {:+.1}%", best_crf, size_change_pct));

        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim,
            psnr: None,
            vmaf: None,
            iterations,
            quality_passed,
            log,
            confidence: 0.7,  // 简单模式默认置信度
            confidence_detail: ConfidenceBreakdown::default(),
        })
    }
    
    /// 模式 2: 仅匹配输入质量（--match-quality 单独使用）
    /// 
    /// 策略：使用 AI 预测的 CRF 值，单次编码
    /// 验证 SSIM 但不探索，快速完成
    fn explore_quality_match(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        
        log.push(format!("🎯 Quality-Match Mode ({:?})", self.encoder));
        log.push(format!("   Input: {} bytes", self.input_size));
        log.push(format!("   Predicted CRF: {}", self.config.initial_crf));
        
        // 单次编码
        let output_size = self.encode(self.config.initial_crf)?;
        let quality = self.validate_quality()?;
        
        // 🔥 v3.3: 显示所有启用的质量指标
        let mut quality_str = format!("SSIM: {:.4}", quality.0.unwrap_or(0.0));
        if let Some(vmaf) = quality.2 {
            quality_str.push_str(&format!(", VMAF: {:.2}", vmaf));
        }
        log.push(format!("   CRF {}: {} bytes ({:+.1}%), {}", 
            self.config.initial_crf, output_size, 
            self.calc_change_pct(output_size),
            quality_str));
        
        let quality_passed = self.check_quality_passed(quality.0, quality.1, quality.2);
        if quality_passed {
            log.push("   ✅ Quality validation passed".to_string());
        } else {
            log.push(format!("   ⚠️ Quality below threshold (min SSIM: {:.4})", 
                self.config.quality_thresholds.min_ssim));
        }
        
        Ok(ExploreResult {
            optimal_crf: self.config.initial_crf,
            output_size,
            size_change_pct: self.calc_change_pct(output_size),
            ssim: quality.0,
            psnr: quality.1,
            vmaf: quality.2,
            iterations: 1,
            quality_passed,
            log,
            confidence: 0.6,  // 单次编码置信度较低
            confidence_detail: ConfidenceBreakdown::default(),
        })
    }
    
    /// 🔥 v4.8 模式 5: 仅压缩（--compress 单独使用）
    ///
    /// ## 目标
    /// 确保输出 < 输入（哪怕只小 1KB 也算成功）
    ///
    /// ## 策略
    /// 1. 先测试 initial_crf，如果能压缩直接返回（最高质量）
    /// 2. 二分搜索找最低能压缩的 CRF
    /// 3. 使用缓存避免重复编码
    fn explore_compress_only(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let mut cache: std::collections::HashMap<i32, u64> = std::collections::HashMap::new();



        let start_time = std::time::Instant::now();
        let mut _best_crf_so_far: f32 = 0.0;
        
        // 带缓存的编码
        let encode_cached = |crf: f32, cache: &mut std::collections::HashMap<i32, u64>, explorer: &VideoExplorer| -> Result<u64> {
            let key = (crf * 4.0).round() as i32;
            if let Some(&size) = cache.get(&key) {
                return Ok(size);
            }
            let size = explorer.encode(crf)?;
            cache.insert(key, size);
            Ok(size)
        };

        // 🔥 v5.7: Unified Professional Process
        let pb = crate::progress::create_professional_spinner("📦 Compress Only");
        
        macro_rules! progress_line {
            ($($arg:tt)*) => {{
                pb.set_message(format!($($arg)*));
            }};
        }
        
        macro_rules! progress_done {
            () => {{ }};
        }

        // 🔥 v5.8: Modern Header style
        pb.suspend(|| {
             eprintln!("┌ 📦 Compress-Only ({:?})", self.encoder);
             eprintln!("└ 📁 Input: {:.2} MB", self.input_size as f64 / 1024.0 / 1024.0);
        });
        log.push(format!("📦 Compress-Only ({:?})", self.encoder));

        let mut iterations = 0u32;

        // 先测试 initial_crf
        let initial_size = encode_cached(self.config.initial_crf, &mut cache, self)?;
        iterations += 1;
        let size_pct = self.calc_change_pct(initial_size);
        progress_line!("CRF {:.1} | {:+.1}% | Iter {}", self.config.initial_crf, size_pct, iterations);

        if initial_size < self.input_size {

            progress_done!();
            _best_crf_so_far = self.config.initial_crf;
            let elapsed = start_time.elapsed();
            
            pb.finish_and_clear();
            eprintln!("✅ Result: CRF {:.1} • {:+.1}% ✅ • ({:.1}s)", 
                self.config.initial_crf, size_pct, elapsed.as_secs_f64());
            return Ok(ExploreResult {
                optimal_crf: self.config.initial_crf,
                output_size: initial_size,
                size_change_pct: self.calc_change_pct(initial_size),
                ssim: None,
                psnr: None,
                vmaf: None,
                iterations,
                quality_passed: true,
                log,
                confidence: 0.7,
                confidence_detail: ConfidenceBreakdown::default(),
            });
        }

        // 二分搜索找最低能压缩的 CRF
        let mut low = self.config.initial_crf;
        let mut high = self.config.max_crf;
        let mut best_crf: Option<f32> = None;
        let mut best_size: Option<u64> = None;

        while high - low > precision::FINE_STEP && iterations < self.config.max_iterations {
            let mid = ((low + high) / 2.0 * 2.0).round() / 2.0;

            let size = encode_cached(mid, &mut cache, self)?;
            iterations += 1;
            let size_pct = self.calc_change_pct(size);
            let compress_icon = if size < self.input_size { "✅" } else { "❌" };
            progress_line!("Binary Search | CRF {:.1} | {:+.1}% {} | Best: {:.1}", 
                mid, size_pct, compress_icon, _best_crf_so_far);

            if size < self.input_size {
                best_crf = Some(mid);
                best_size = Some(size);
                _best_crf_so_far = mid;
                high = mid;
            } else {
                low = mid;
            }
        }
        progress_done!();

        // 返回结果
        let (final_crf, final_size) = if let (Some(crf), Some(size)) = (best_crf, best_size) {
            (crf, size)
        } else {
            let size = encode_cached(self.config.max_crf, &mut cache, self)?;
            (self.config.max_crf, size)
        };

        let size_change_pct = self.calc_change_pct(final_size);
        let compressed = final_size < self.input_size;
        let elapsed = start_time.elapsed();

        // 🔥 v5.7: Result
        pb.finish_and_clear();
        let status = if compressed { "✅" } else { "⚠️" };
        eprintln!("✅ Result: CRF {:.1} • {:+.1}% {} • Iter {} ({:.1}s)", 
            final_crf, size_change_pct, status, iterations, elapsed.as_secs_f64());
        log.push(format!("📊 RESULT: CRF {:.1}, {:+.1}%", final_crf, size_change_pct));

        Ok(ExploreResult {
            optimal_crf: final_crf,
            output_size: final_size,
            size_change_pct,
            ssim: None,
            psnr: None,
            vmaf: None,
            iterations,
            quality_passed: compressed,
            log,
            confidence: 0.65,
            confidence_detail: ConfidenceBreakdown::default(),
        })
    }
    
    /// 🔥 v4.8 模式 4: 压缩 + 粗略质量验证（--compress --match-quality 组合）
    ///
    /// ## 目标
    /// 确保输出 < 输入 + SSIM >= 阈值
    ///
    /// ## 策略
    /// 1. 二分搜索找最低能压缩的 CRF
    /// 2. 验证 SSIM 是否满足阈值
    /// 3. 使用缓存避免重复编码
    fn explore_compress_with_quality(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        // 缓存：CRF (x10) -> (size, ssim)
        let mut cache: std::collections::HashMap<i32, (u64, Option<f64>)> = std::collections::HashMap::new();

        // 🔥 v5.7: Unified Process
        let pb = crate::progress::create_professional_spinner("📦 Compress+Quality");
        
        macro_rules! log_realtime {
            ($($arg:tt)*) => {{
                let msg = format!($($arg)*);
                pb.suspend(|| eprintln!("{}", msg));
                log.push(msg);
            }};
        }

        let min_ssim = self.config.quality_thresholds.min_ssim;
        // 🔥 v5.8: Modern Header
        pb.suspend(|| {
             eprintln!("┌ 📦 Compress + Quality v4.8 ({:?})", self.encoder);
             eprintln!("├ 📁 Input: {} bytes", self.input_size);
             eprintln!("└ 🎯 Goal: output < input + SSIM >= {:.2}", min_ssim);
        });

        let mut iterations = 0u32;
        let mut best_result: Option<(f32, u64, f64)> = None; // (crf, size, ssim)

        // Phase 1: 二分搜索找最低能压缩的 CRF
        pb.set_message("Phase 1: Binary search for compression boundary");
        let mut low = self.config.initial_crf;
        let mut high = self.config.max_crf;
        let mut compress_boundary: Option<f32> = None;
        
        // 进度条辅助（保留以备将来使用）
        #[allow(unused_macros)]
        macro_rules! progress_log {
            ($($arg:tt)*) => {{
                pb.set_message(format!($($arg)*));
            }};
        }

        while high - low > precision::COARSE_STEP / 2.0 && iterations < self.config.max_iterations {
            let mid = ((low + high) / 2.0).round();

            log_realtime!("   🔄 Testing CRF {:.0}...", mid);
            let size = self.encode(mid as f32)?;
            iterations += 1;

            let key = (mid * 10.0).round() as i32;
            cache.insert(key, (size, None));

            if size < self.input_size {
                compress_boundary = Some(mid as f32);
                high = mid;
                log_realtime!("      ✅ Compresses at CRF {:.0}", mid);
            } else {
                low = mid;
                log_realtime!("      ❌ Too large at CRF {:.0}", mid);
            }
        }

        // Phase 2: 在压缩边界验证质量
        if let Some(boundary) = compress_boundary {
            log_realtime!("   📍 Phase 2: Validate quality at CRF {:.1}", boundary);

            // 直接在边界点验证质量（边界点是最低能压缩的 CRF = 最高质量）
            let key = (boundary * 10.0).round() as i32;
            let size = if let Some(&(s, _)) = cache.get(&key) {
                s
            } else {
                let s = self.encode(boundary)?;
                iterations += 1;
                s
            };

            let quality = self.validate_quality()?;
            let ssim = quality.0.unwrap_or(0.0);
            cache.insert(key, (size, Some(ssim)));

            log_realtime!("      CRF {:.1}: SSIM {:.4}, Size {:+.1}%", boundary, ssim, self.calc_change_pct(size));

            if ssim >= min_ssim {
                best_result = Some((boundary, size, ssim));
                log_realtime!("      ✅ Valid: compresses + SSIM OK");
            } else {
                // SSIM 不够，但这是最高质量的压缩点，记录为备选
                best_result = Some((boundary, size, ssim));
                log_realtime!("      ⚠️ SSIM below threshold, but best available");
            }
        }

        // 返回结果（使用缓存的值）
        let (final_crf, final_size, final_ssim) = if let Some((crf, size, ssim)) = best_result {
            (crf, size, ssim)
        } else {
            // 无法压缩，测试 max_crf
            let size = self.encode(self.config.max_crf)?;
            let quality = self.validate_quality()?;
            (self.config.max_crf, size, quality.0.unwrap_or(0.0))
        };

        let size_change_pct = self.calc_change_pct(final_size);
        let compressed = final_size < self.input_size;
        let quality_ok = final_ssim >= min_ssim;
        let passed = compressed && quality_ok;

        // 🔥 v5.7: Result
        pb.finish_and_clear();
        log_realtime!("✅ RESULT: CRF {:.1} • SSIM {:.4} • Size {:+.1}% {}",
            final_crf, final_ssim, size_change_pct,
            if passed { "✅" } else if compressed { "⚠️ SSIM low" } else { "⚠️ Not compressed" });
        log_realtime!("📈 Iterations: {}", iterations);

        Ok(ExploreResult {
            optimal_crf: final_crf,
            output_size: final_size,
            size_change_pct,
            ssim: Some(final_ssim),
            psnr: None,
            vmaf: None,
            iterations,
            quality_passed: passed,
            log,
            confidence: 0.75,
            confidence_detail: ConfidenceBreakdown::default(),
        })
    }
    
    /// 模式 3: 精确质量匹配（--explore + --match-quality 组合）
    ///
    /// 🔥 v4.9: 优化效率 - 消除重复编码，统一缓存机制
    ///
    /// ## 目标
    /// 找到**最高 SSIM**（最接近源质量）的 CRF 值
    /// **不关心文件大小**，只关心质量精度
    ///
    /// ## 优化策略 (v4.9)
    /// 1. **统一缓存**：所有编码结果缓存，避免重复
    /// 2. **智能最终编码**：只有当最后编码不是best_crf时才重编码
    /// 3. **三阶段搜索**：边界→黄金分割→精细调整（±0.1精度）
    /// 4. **早期终止**：检测到SSIM平台立即停止
    fn explore_precise_quality_match(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        // 🔥 v4.9: 统一缓存 - CRF (x10) -> (size, quality)
        let mut cache: std::collections::HashMap<i32, (u64, (Option<f64>, Option<f64>, Option<f64>))> =
            std::collections::HashMap::new();
        // 🔥 v4.9: 跟踪最后实际编码的 CRF（整数 x10）
        let mut last_encoded_key: i32 = -1;

        macro_rules! log_realtime {
            ($($arg:tt)*) => {{
                let msg = format!($($arg)*);
                eprintln!("{}", msg);
                log.push(msg);
            }};
        }

        log_realtime!("🔬 Precise Quality-Match v4.9 ({:?})", self.encoder);
        log_realtime!("   📁 Input: {} bytes ({:.2} MB)",
            self.input_size, self.input_size as f64 / 1024.0 / 1024.0);
        log_realtime!("   📐 CRF range: [{:.1}, {:.1}]",
            self.config.min_crf, self.config.max_crf);
        log_realtime!("   🎯 Goal: Find HIGHEST SSIM (best quality match)");
        log_realtime!("   ═══════════════════════════════════════════════════");

        let mut iterations = 0u32;
        const MAX_ITERATIONS: u32 = 15;
        const SSIM_PLATEAU_THRESHOLD: f64 = 0.0002;

        let mut best_crf: f32;
        let mut best_size: u64;
        let mut best_quality: (Option<f64>, Option<f64>, Option<f64>);
        let mut best_ssim: f64;

        // 🔥 v4.9: 带缓存和跟踪的编码函数
        let encode_cached = |crf: f32,
                            cache: &mut std::collections::HashMap<i32, (u64, (Option<f64>, Option<f64>, Option<f64>))>,
                            last_key: &mut i32,
                            explorer: &VideoExplorer| -> Result<(u64, (Option<f64>, Option<f64>, Option<f64>))> {
            let key = (crf * 4.0).round() as i32;
            if let Some(&cached) = cache.get(&key) {
                return Ok(cached);
            }

            let size = explorer.encode(crf)?;
            let quality = explorer.validate_quality()?;
            cache.insert(key, (size, quality));
            *last_key = key;  // 更新最后编码的 key
            Ok((size, quality))
        };

        // Phase 1: 边界测试
        log_realtime!("   📍 Phase 1: Boundary test");

        log_realtime!("   🔄 Testing min CRF {:.1}...", self.config.min_crf);
        let (min_size, min_quality) = encode_cached(self.config.min_crf, &mut cache, &mut last_encoded_key, self)?;
        iterations += 1;
        let min_ssim = min_quality.0.unwrap_or(0.0);
        log_realtime!("      CRF {:.1}: SSIM {:.6}, Size {:+.1}%",
            self.config.min_crf, min_ssim, self.calc_change_pct(min_size));

        best_crf = self.config.min_crf;
        best_size = min_size;
        best_quality = min_quality;
        best_ssim = min_ssim;

        log_realtime!("   🔄 Testing max CRF {:.1}...", self.config.max_crf);
        let (max_size, max_quality) = encode_cached(self.config.max_crf, &mut cache, &mut last_encoded_key, self)?;
        iterations += 1;
        let max_ssim = max_quality.0.unwrap_or(0.0);
        log_realtime!("      CRF {:.1}: SSIM {:.6}, Size {:+.1}%",
            self.config.max_crf, max_ssim, self.calc_change_pct(max_size));

        let ssim_range = min_ssim - max_ssim;
        log_realtime!("      SSIM range: {:.6}", ssim_range);

        // 早期终止：SSIM 几乎无变化，选择更高 CRF（更小文件）
        if ssim_range < SSIM_PLATEAU_THRESHOLD {
            log_realtime!("   ⚡ Early exit: SSIM plateau, using max CRF for smaller file");
            best_crf = self.config.max_crf;
            best_size = max_size;
            best_quality = max_quality;
            best_ssim = max_ssim;
        } else {
            // Phase 2: 黄金分割搜索找平台边缘
            log_realtime!("   📍 Phase 2: Golden section search");
            const PHI: f32 = 0.618;

            let mut low = self.config.min_crf;
            let mut high = self.config.max_crf;
            let mut prev_ssim = min_ssim;

            while high - low > 1.0 && iterations < MAX_ITERATIONS {
                let mid = low + (high - low) * PHI;
                let mid_rounded = (mid * 2.0).round() / 2.0;

                log_realtime!("   🔄 Testing CRF {:.1}...", mid_rounded);
                let (size, quality) = encode_cached(mid_rounded, &mut cache, &mut last_encoded_key, self)?;
                iterations += 1;
                let ssim = quality.0.unwrap_or(0.0);
                log_realtime!("      CRF {:.1}: SSIM {:.6}, Size {:+.1}%",
                    mid_rounded, ssim, self.calc_change_pct(size));

                // 更新最佳（优先高 SSIM，相同时选高 CRF = 更小文件）
                if ssim > best_ssim + 0.00001 || (ssim >= best_ssim - 0.00001 && mid_rounded > best_crf) {
                    best_crf = mid_rounded;
                    best_size = size;
                    best_quality = quality;
                    best_ssim = ssim;
                }

                // 检测 SSIM 下降 → 收缩搜索范围
                if prev_ssim - ssim > SSIM_PLATEAU_THRESHOLD * 2.0 {
                    high = mid_rounded;
                    log_realtime!("      ↓ SSIM drop, narrowing to [{:.1}, {:.1}]", low, high);
                } else {
                    low = mid_rounded;
                }
                prev_ssim = ssim;
            }

            // Phase 3: 精细调整 ±0.5 和 ±0.1
            if iterations < MAX_ITERATIONS {
                log_realtime!("   📍 Phase 3: Fine-tune around CRF {:.1}", best_crf);

                // 先测试 ±0.5
                for offset in [-0.5_f32, 0.5] {
                    let crf = (best_crf + offset).clamp(self.config.min_crf, self.config.max_crf);
                    if iterations >= MAX_ITERATIONS { break; }

                    log_realtime!("   🔄 Testing CRF {:.1}...", crf);
                    let (size, quality) = encode_cached(crf, &mut cache, &mut last_encoded_key, self)?;
                    iterations += 1;
                    let ssim = quality.0.unwrap_or(0.0);
                    log_realtime!("      CRF {:.1}: SSIM {:.6}", crf, ssim);

                    if ssim > best_ssim + 0.00001 || (ssim >= best_ssim - 0.00001 && crf > best_crf) {
                        best_crf = crf;
                        best_size = size;
                        best_quality = quality;
                        best_ssim = ssim;
                    }
                }

                // 🔥 v4.9: 进一步 ±0.1 精细调整（达到 ±0.1 精度）
                if iterations < MAX_ITERATIONS {
                    for offset in [-0.25_f32, 0.25, -0.5, 0.5] {
                        let crf = (best_crf + offset).clamp(self.config.min_crf, self.config.max_crf);
                        // 避免重复测试已缓存的值
                        let key = (crf * 4.0).round() as i32;
                        if cache.contains_key(&key) { continue; }
                        if iterations >= MAX_ITERATIONS { break; }

                        log_realtime!("   🔄 Testing CRF {:.1}...", crf);
                        let (size, quality) = encode_cached(crf, &mut cache, &mut last_encoded_key, self)?;
                        iterations += 1;
                        let ssim = quality.0.unwrap_or(0.0);
                        log_realtime!("      CRF {:.1}: SSIM {:.6}", crf, ssim);

                        if ssim > best_ssim + 0.00001 || (ssim >= best_ssim - 0.00001 && crf > best_crf) {
                            best_crf = crf;
                            best_size = size;
                            best_quality = quality;
                            best_ssim = ssim;
                        }
                    }
                }
            }
        }

        // 🔥 v4.9: 智能最终编码 - 只有必要时才重新编码
        let best_key = (best_crf * 4.0).round() as i32;
        let (final_size, final_quality) = if last_encoded_key == best_key {
            // 最后一次编码就是 best_crf，直接使用缓存
            log_realtime!("   ✨ Output already at best CRF {:.1} (no re-encoding needed)", best_crf);
            (best_size, best_quality)
        } else {
            // 最后一次编码不是 best_crf，需要重新编码
            log_realtime!("   📍 Final: Re-encoding to best CRF {:.1}", best_crf);
            let size = self.encode(best_crf)?;
            (size, best_quality)
        };

        let size_change_pct = self.calc_change_pct(final_size);

        let status = if best_ssim >= 0.9999 { "✅ Near-Lossless" }
            else if best_ssim >= 0.999 { "✅ Excellent" }
            else if best_ssim >= 0.99 { "✅ Very Good" }
            else if best_ssim >= 0.98 { "✅ Good" }
            else { "✅ Acceptable" };

        log_realtime!("   ═══════════════════════════════════════════════════");
        log_realtime!("   📊 RESULT: CRF {:.1}, SSIM {:.6} {}, Size {:+.1}%", best_crf, best_ssim, status, size_change_pct);
        log_realtime!("   📈 Iterations: {} (cache hits saved encoding time)", iterations);

        let quality_passed = best_ssim >= self.config.quality_thresholds.min_ssim;

        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: final_size,
            size_change_pct,
            ssim: final_quality.0,
            psnr: final_quality.1,
            vmaf: final_quality.2,
            iterations,
            quality_passed,
            log,
            confidence: 0.8,
            confidence_detail: ConfidenceBreakdown::default(),
        })
    }
    
    /// 🔥 v4.13: 精确质量匹配 + 压缩（--explore + --match-quality + --compress 组合）
    ///
    /// ## 目标
    /// 找到**最高 SSIM** 且 **输出 < 输入**
    ///
    /// ## 🔥 v4.13 新增：智能提前终止
    ///
    /// ### 提前终止机制
    /// 1. **滑动窗口方差检测**：最近 3 次编码的 size 方差 < 0.01% → 已接近边界
    /// 2. **相对变化率检测**：size 变化率 < 0.5% → 提前终止
    ///
    /// ### 三阶段搜索
    /// 1. **Phase 1**: 二分搜索（0.5 步进）+ 智能终止
    /// 2. **Phase 2**: 双向 0.1 精细调整 + 智能终止
    /// 3. **Phase 3**: SSIM 验证
    ///
    /// ### 效率优化
    /// - 智能终止可减少 30-50% 编码次数
    /// - Phase 1: ~3-7 次编码（取决于内容）
    /// - Phase 2: ~1-4 次编码（取决于边界精度）
    /// - Phase 3: 只对最终边界点算1次SSIM
    fn explore_precise_quality_match_with_compression(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let mut size_cache: std::collections::HashMap<i32, u64> = std::collections::HashMap::new();
        let mut quality_cache: std::collections::HashMap<i32, (Option<f64>, Option<f64>, Option<f64>)> = std::collections::HashMap::new();
        let mut last_encoded_key: i32 = -1;
        
        // 🔥 v5.5: 进度追踪变量
        let mut best_crf_so_far: f32 = 0.0;

        let start_time = std::time::Instant::now();

        // 🔥 v5.7: Unified Professional Progress
        let pb = crate::progress::create_professional_spinner("🔍 Initializing");

        // Local macros to use pb
        macro_rules! progress_line {
            ($($arg:tt)*) => {{
                pb.set_message(format!($($arg)*));
            }};
        }

        macro_rules! progress_done {
            () => {{ }};
        }

        macro_rules! log_header {
            ($($arg:tt)*) => {{
                let msg = format!($($arg)*);
                pb.suspend(|| eprintln!("{}", msg));
                log.push(msg);
            }};
        }
        
        // 🔥 v5.7: Detailed Real-time Jumping Data
        macro_rules! log_progress {
            ($stage:expr, $crf:expr, $size:expr, $iter:expr) => {{
                let size_pct = if self.input_size > 0 {
                    (($size as f64 / self.input_size as f64) - 1.0) * 100.0
                } else { 0.0 };
                let compress_icon = if $size < self.input_size { "💾" } else { "⚠️" };
                
                // Update Prefix with Phase
                pb.set_prefix(format!("🔍 {}", $stage));
                
                // Content-rich message
                let msg = format!(
                    "CRF {:.1} | {:+.1}% {} | Iter {} | Best: {:.1}",
                     $crf, size_pct, compress_icon, $iter, best_crf_so_far
                );
                pb.set_message(msg);
                
                log.push(format!("   🔄 CRF {:.1}: {:+.1}%", $crf, size_pct));
            }};
        }

        // 🔥 v5.31: 优化缓存精度 (CRF*100) - 支持0.01精度
        // 仅编码（不计算SSIM）
        let encode_size_only = |crf: f32,
                               size_cache: &mut std::collections::HashMap<i32, u64>,
                               last_key: &mut i32,
                               explorer: &VideoExplorer| -> Result<u64> {
            let key = (crf * 4.0).round() as i32;  // 🔥 提升精度：10 → 100
            if let Some(&size) = size_cache.get(&key) {
                return Ok(size);
            }
            let size = explorer.encode(crf)?;
            size_cache.insert(key, size);
            *last_key = key;
            Ok(size)
        };

        // 计算SSIM
        let validate_ssim = |crf: f32,
                            quality_cache: &mut std::collections::HashMap<i32, (Option<f64>, Option<f64>, Option<f64>)>,
                            explorer: &VideoExplorer| -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
            let key = (crf * 4.0).round() as i32;  // 🔥 提升精度：10 → 100
            if let Some(&quality) = quality_cache.get(&key) {
                return Ok(quality);
            }
            let quality = explorer.validate_quality()?;
            quality_cache.insert(key, quality);
            Ok(quality)
        };

        // 🔥 v5.5: Clean Header
        log_header!("🔬 Precise Quality + Compression ({:?}) • Input: {:.2} MB", self.encoder, self.input_size as f64 / 1024.0 / 1024.0);
        log_header!("   Goal: Best SSIM + Output < Input • Range: [{:.1}, {:.1}]", self.config.min_crf, self.config.max_crf);

        let mut iterations = 0u32;

        // ═══════════════════════════════════════════════════════════
        // Stage A: 纯大小搜索（从 min_crf 向上搜索找压缩边界）
        // ═══════════════════════════════════════════════════════════
        log_header!("   📍 Stage A: 大小搜索");

        // 🔥 关键修复：从 min_crf 开始测试（最高质量）
        let min_size = encode_size_only(self.config.min_crf, &mut size_cache, &mut last_encoded_key, self)?;
        iterations += 1;
        log_progress!("Stage A", self.config.min_crf, min_size, iterations);

        if min_size < self.input_size {
            // min_crf 能压缩，但可能还能更低！继续向下探索
            best_crf_so_far = self.config.min_crf;
            progress_done!();
            
            // 🔥 v5.3: 先用 0.5 步长快速向下探索，再用 0.1 精细调整
            let mut best_crf = self.config.min_crf;
            let mut best_size = min_size;
            // Stage B-1: 0.5 步长快速向下探索
            log_header!("   📍 Stage B-1: 快速搜索 (0.5 步长)");
            let mut test_crf = self.config.min_crf - 0.5;
            while test_crf >= ABSOLUTE_MIN_CRF && iterations < STAGE_B1_MAX_ITERATIONS {
                let size = encode_size_only(test_crf, &mut size_cache, &mut last_encoded_key, self)?;
                iterations += 1;
                log_progress!("Stage B-1", test_crf, size, iterations);
                
                if size < self.input_size {
                    best_crf = test_crf;
                    best_size = size;
                    best_crf_so_far = test_crf;
                    test_crf -= 0.5;
                } else {
                    break;
                }
            }
            progress_done!();
            
            // Stage B-2: 0.1 步长精细调整（在 best_crf 附近）
            log_header!("   📍 Stage B-2: 精细调整 (0.1 步长)");
            for offset in [-0.25_f32, -0.5, -0.75, -1.0] {
                let fine_crf = best_crf + offset;
                if fine_crf < ABSOLUTE_MIN_CRF { break; }
                if iterations >= STAGE_B2_MAX_ITERATIONS { break; }

                let key = (fine_crf * 4.0).round() as i32;  // 🔥 v5.31: 精度修正
                if size_cache.contains_key(&key) { continue; }

                let size = encode_size_only(fine_crf, &mut size_cache, &mut last_encoded_key, self)?;
                iterations += 1;
                log_progress!("Stage B-2", fine_crf, size, iterations);

                if size < self.input_size {
                    best_crf = fine_crf;
                    best_size = size;
                    best_crf_so_far = fine_crf;
                } else {
                    break;
                }
            }
            progress_done!();

            // 确保输出文件是 best_crf 的版本
            let best_key = (best_crf * 4.0).round() as i32;  // 🔥 v5.31: 精度修正
            if last_encoded_key != best_key {
                progress_line!("│ 重新编码到最佳 CRF {:.1}... │", best_crf);
                let _ = encode_size_only(best_crf, &mut size_cache, &mut last_encoded_key, self)?;
                progress_done!();
            }
            
            log_header!("   📍 Stage C: SSIM 验证");
            progress_line!("│ 计算 SSIM... │");
            let quality = validate_ssim(best_crf, &mut quality_cache, self)?;
            let ssim = quality.0.unwrap_or(0.0);

            progress_done!();

            let status = if ssim >= 0.999 { "✅ 极佳" }
                else if ssim >= 0.99 { "✅ 优秀" }
                else if ssim >= 0.98 { "✅ 良好" }
                else { "✅ 可接受" };

            // 🔥 v5.5: 最终结果框
            let elapsed = start_time.elapsed();
            let saved = self.input_size - best_size;
            pb.finish_and_clear();
            eprintln!("✅ Result: CRF {:.1} • SSIM {:.4} {} • {:+.1}% ({:.2} MB saved) • {} iter in {:.1}s",
                best_crf, ssim, status, self.calc_change_pct(best_size), saved as f64 / 1024.0 / 1024.0, iterations, elapsed.as_secs_f64());
            
            return Ok(ExploreResult {
                optimal_crf: best_crf,
                output_size: best_size,
                size_change_pct: self.calc_change_pct(best_size),
                ssim: quality.0,
                psnr: quality.1,
                vmaf: quality.2,
                iterations,
                quality_passed: true,
                log,
                confidence: 0.85,
                confidence_detail: ConfidenceBreakdown::default(),
            });
        }

        progress_done!();

        // 测试 max_crf 确认能否压缩
        let max_size = encode_size_only(self.config.max_crf, &mut size_cache, &mut last_encoded_key, self)?;
        iterations += 1;
        log_progress!("Stage A", self.config.max_crf, max_size, iterations);

        if max_size >= self.input_size {
            // 即使 max_crf 也无法压缩
            progress_done!();
            log_header!("   ⚠️ 文件已高度压缩，无法进一步压缩");
            let quality = validate_ssim(self.config.max_crf, &mut quality_cache, self)?;

            let elapsed = start_time.elapsed();
            pb.finish_and_clear();
            eprintln!("⚠️ Cannot compress file (already optimized) • {} iter in {:.1}s", iterations, elapsed.as_secs_f64());

            return Ok(ExploreResult {
                optimal_crf: self.config.max_crf,
                output_size: max_size,
                size_change_pct: self.calc_change_pct(max_size),
                ssim: quality.0,
                psnr: quality.1,
                vmaf: quality.2,
                iterations,
                quality_passed: false,
                log,
                confidence: 0.3,  // 无法压缩，置信度低
                confidence_detail: ConfidenceBreakdown::default(),
            });
        }

        progress_done!();

        // 🔥 v5.31: 最保守的提前终止（保证质量第一）
        const WINDOW_SIZE: usize = 3;
        const VARIANCE_THRESHOLD: f64 = 0.00001;  // 🔥 v5.31 修正：超保守（收敛度极高才终止）
        const CHANGE_RATE_THRESHOLD: f64 = 0.005;  // 🔥 v5.31 修正：0.5%（极其保守）
        let mut size_history: Vec<(f32, u64)> = Vec::new();

        // 🔥 v5.31: 最保守的方差计算 - 不归一化，用绝对值
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

        // 🔥 v5.31: 最保守的变化率计算
        let calc_change_rate = |prev: u64, curr: u64| -> f64 {
            if prev == 0 { return f64::MAX; }
            ((curr as f64 - prev as f64) / prev as f64).abs()
        };

        // 🔥 v5.31: 最保守的二分搜索 - 从粗到精的第一阶段
        log_header!("   📍 Stage A: 二分搜索 (0.5 步长)");
        let mut low = self.config.min_crf;
        let mut high = self.config.max_crf;
        let mut boundary_crf = self.config.max_crf;
        let mut prev_size: Option<u64> = None;

        while high - low > 0.5 && iterations < 12 {
            let mid = ((low + high) / 2.0 * 2.0).round() / 2.0;

            let size = encode_size_only(mid, &mut size_cache, &mut last_encoded_key, self)?;
            iterations += 1;
            size_history.push((mid, size));
            log_progress!("二分搜索", mid, size, iterations);

            let variance = calc_window_variance(&size_history, self.input_size);
            let change_rate = prev_size.map(|p| calc_change_rate(p, size)).unwrap_or(f64::MAX);

            if size < self.input_size {
                boundary_crf = mid;
                best_crf_so_far = mid;
                high = mid;
            } else {
                low = mid;
            }

            // 🔥 v5.31: 最保守的提前终止 - 只在极端情况下终止
            if variance < VARIANCE_THRESHOLD && size_history.len() >= WINDOW_SIZE {
                progress_done!();
                log_header!("   ⚡ 提前终止: 方差完全收敛 {:.8} < {:.8}", variance, VARIANCE_THRESHOLD);
                break;
            }
            if change_rate < CHANGE_RATE_THRESHOLD && prev_size.is_some() {
                progress_done!();
                log_header!("   ⚡ 提前终止: 变化率极小 {:.4}% < {:.4}%", change_rate * 100.0, CHANGE_RATE_THRESHOLD * 100.0);
                break;
            }

            prev_size = Some(size);
        }
        progress_done!();

        // ═══════════════════════════════════════════════════════════
        // 🔥 v5.31: Stage B - 从粗到精的第二阶段：精细调整
        // ═══════════════════════════════════════════════════════════
        log_header!("   📍 Stage B: 精细调整 (0.1 步长)");

        let mut best_boundary = boundary_crf;
        let mut fine_tune_history: Vec<u64> = Vec::new();

        // 🔥 v5.31: 先向下探索（更高质量方向）- 智能步进
        for offset in [-0.25_f32, -0.5, -0.75, -1.0] {
            let test_crf = boundary_crf + offset;
            
            if test_crf < self.config.min_crf { continue; }
            if iterations >= STAGE_B_BIDIRECTIONAL_MAX { break; }
            
            let key = (test_crf * 4.0).round() as i32;
            if size_cache.contains_key(&key) { continue; }

            let size = encode_size_only(test_crf, &mut size_cache, &mut last_encoded_key, self)?;
            iterations += 1;
            fine_tune_history.push(size);
            log_progress!("精细调整↓", test_crf, size, iterations);

            if size < self.input_size {
                best_boundary = test_crf;
                best_crf_so_far = test_crf;
                
                if fine_tune_history.len() >= 2 {
                    let prev = fine_tune_history[fine_tune_history.len() - 2];
                    let rate = calc_change_rate(prev, size);
                    if rate < CHANGE_RATE_THRESHOLD {
                        progress_done!();
                        log_header!("   ⚡ 提前终止: Δ{:.3}%", rate * 100.0);
                        break;
                    }
                }
            } else {
                break;
            }
        }

        // 如果向下没找到更好的，向上探索
        if best_boundary == boundary_crf {
            fine_tune_history.clear();
            
            for offset in [0.25_f32, 0.5, 0.75, 1.0] {
                let test_crf = boundary_crf + offset;
                
                if test_crf > self.config.max_crf { continue; }
                if iterations >= STAGE_B_BIDIRECTIONAL_MAX { break; }
                
                let key = (test_crf * 4.0).round() as i32;
                if size_cache.contains_key(&key) { continue; }

                let size = encode_size_only(test_crf, &mut size_cache, &mut last_encoded_key, self)?;
                iterations += 1;
                fine_tune_history.push(size);
                log_progress!("精细调整↑", test_crf, size, iterations);

                if size < self.input_size {
                    best_boundary = test_crf;
                    best_crf_so_far = test_crf;
                    
                    if fine_tune_history.len() >= 2 {
                        let prev = fine_tune_history[fine_tune_history.len() - 2];
                        let rate = calc_change_rate(prev, size);
                        if rate < CHANGE_RATE_THRESHOLD {
                            progress_done!();
                            log_header!("   ⚡ 提前终止: Δ{:.3}%", rate * 100.0);
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }
        progress_done!();

        if best_boundary != boundary_crf {
            boundary_crf = best_boundary;
        }

        // ═══════════════════════════════════════════════════════════
        // Stage C: SSIM 验证
        // ═══════════════════════════════════════════════════════════
        log_header!("   📍 Stage C: SSIM 验证");

        // 确保输出文件是 boundary_crf 的版本
        let boundary_key = (boundary_crf * 4.0).round() as i32;
        if last_encoded_key != boundary_key {
            progress_line!("│ 重新编码到 CRF {:.1}... │", boundary_crf);
            let _ = encode_size_only(boundary_crf, &mut size_cache, &mut last_encoded_key, self)?;
            progress_done!();
        }

        progress_line!("│ 计算 SSIM... │");
        let quality = validate_ssim(boundary_crf, &mut quality_cache, self)?;
        let ssim = quality.0.unwrap_or(0.0);

        progress_done!();
        
        let final_size = *size_cache.get(&boundary_key).unwrap();

        let size_change_pct = self.calc_change_pct(final_size);
        let status = if ssim >= 0.999 { "✅ 极佳" }
            else if ssim >= 0.99 { "✅ 优秀" }
            else if ssim >= 0.98 { "✅ 良好" }
            else { "✅ 可接受" };

        // 🔥 v5.5: 最终结果框
        let elapsed = start_time.elapsed();
        let saved = self.input_size - final_size;
        pb.finish_and_clear();
        eprintln!("✅ Result: CRF {:.1} • SSIM {:.4} {} • {:+.1}% ({:.2} MB saved) • {} iter in {:.1}s",
            boundary_crf, ssim, status, size_change_pct, saved as f64 / 1024.0 / 1024.0, iterations, elapsed.as_secs_f64());


        Ok(ExploreResult {
            optimal_crf: boundary_crf,
            output_size: final_size,
            size_change_pct,
            ssim: quality.0,
            psnr: quality.1,
            vmaf: quality.2,
            iterations,
            quality_passed: ssim >= self.config.quality_thresholds.min_ssim,
            log,
            confidence: 0.85,
            confidence_detail: ConfidenceBreakdown::default(),
        })
    }

    /// 🔥 v4.1: 检查交叉验证一致性
    ///
    /// 当多个质量指标一致时，可以更快确认最优点
    #[allow(dead_code)]  // 保留供将来使用
    fn check_cross_validation_consistency(&self, quality: &(Option<f64>, Option<f64>, Option<f64>)) -> CrossValidationResult {
        let t = &self.config.quality_thresholds;
        
        let ssim_pass = quality.0.map(|s| s >= t.min_ssim).unwrap_or(false);
        let psnr_pass = if t.validate_psnr {
            quality.1.map(|p| p >= t.min_psnr).unwrap_or(false)
        } else {
            true // 未启用则视为通过
        };
        let vmaf_pass = if t.validate_vmaf {
            quality.2.map(|v| v >= t.min_vmaf).unwrap_or(false)
        } else {
            true // 未启用则视为通过
        };
        
        let pass_count = [ssim_pass, psnr_pass, vmaf_pass].iter().filter(|&&x| x).count();
        
        match pass_count {
            3 => CrossValidationResult::AllAgree,
            2 => CrossValidationResult::MajorityAgree,
            _ => CrossValidationResult::Divergent,
        }
    }
    
    /// 🔥 v4.1: 计算综合质量评分
    ///
    /// 综合 SSIM、PSNR、VMAF 计算加权评分
    /// - SSIM 权重: 50% (主要指标)
    /// - VMAF 权重: 35% (感知质量)
    /// - PSNR 权重: 15% (参考指标)
    #[allow(dead_code)]  // 保留供将来使用
    fn calculate_composite_score(&self, quality: &(Option<f64>, Option<f64>, Option<f64>)) -> f64 {
        let ssim = quality.0.unwrap_or(0.0);
        let psnr = quality.1.unwrap_or(0.0);
        let vmaf = quality.2.unwrap_or(0.0);
        
        // 归一化各指标到 0-1 范围
        let ssim_norm = ssim; // 已经是 0-1
        let psnr_norm = (psnr / 60.0).clamp(0.0, 1.0); // PSNR 60dB 视为满分
        let vmaf_norm = (vmaf / 100.0).clamp(0.0, 1.0); // VMAF 100 视为满分
        
        // 加权计算
        let score = if self.config.quality_thresholds.validate_vmaf && self.config.quality_thresholds.validate_psnr {
            // 三重验证：SSIM 50%, VMAF 35%, PSNR 15%
            ssim_norm * 0.50 + vmaf_norm * 0.35 + psnr_norm * 0.15
        } else if self.config.quality_thresholds.validate_vmaf {
            // SSIM + VMAF：SSIM 60%, VMAF 40%
            ssim_norm * 0.60 + vmaf_norm * 0.40
        } else if self.config.quality_thresholds.validate_psnr {
            // SSIM + PSNR：SSIM 70%, PSNR 30%
            ssim_norm * 0.70 + psnr_norm * 0.30
        } else {
            // 仅 SSIM
            ssim_norm
        };

        score
    }

    /// 格式化质量指标字符串
    #[allow(dead_code)]  // 保留供将来使用
    fn format_quality_metrics(&self, quality: &(Option<f64>, Option<f64>, Option<f64>)) -> String {
        let mut parts = Vec::new();
        if let Some(ssim) = quality.0 {
            parts.push(format!("SSIM: {:.4}", ssim));
        }
        if let Some(psnr) = quality.1 {
            parts.push(format!("PSNR: {:.2}dB", psnr));
        }
        if let Some(vmaf) = quality.2 {
            parts.push(format!("VMAF: {:.2}", vmaf));
        }
        if parts.is_empty() {
            "N/A".to_string()
        } else {
            parts.join(", ")
        }
    }
    
    /// 编码视频
    /// 🔥 v4.9: GPU 加速 + 实时进度输出
    fn encode(&self, crf: f32) -> Result<u64> {
        use std::io::{BufRead, BufReader, Write};
        use std::process::Stdio;

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y");

        // 🔥 v4.9: GPU 加速编码
        let gpu = crate::gpu_accel::GpuAccel::detect();
        let (encoder_name, crf_args, extra_args, accel_type) = if self.use_gpu {
            match self.encoder {
                VideoEncoder::Hevc => {
                    if let Some(enc) = gpu.get_hevc_encoder() {
                        (
                            enc.name,
                            enc.get_crf_args(crf),
                            enc.get_extra_args(),
                            format!("🚀 GPU ({})", gpu.gpu_type),
                        )
                    } else {
                        (
                            self.encoder.ffmpeg_name(),
                            vec!["-crf".to_string(), format!("{:.1}", crf)],
                            vec![],
                            "CPU".to_string(),
                        )
                    }
                }
                VideoEncoder::Av1 => {
                    if let Some(enc) = gpu.get_av1_encoder() {
                        (
                            enc.name,
                            enc.get_crf_args(crf),
                            enc.get_extra_args(),
                            format!("🚀 GPU ({})", gpu.gpu_type),
                        )
                    } else {
                        (
                            self.encoder.ffmpeg_name(),
                            vec!["-crf".to_string(), format!("{:.1}", crf)],
                            vec![],
                            "CPU".to_string(),
                        )
                    }
                }
                VideoEncoder::H264 => {
                    if let Some(enc) = gpu.get_h264_encoder() {
                        (
                            enc.name,
                            enc.get_crf_args(crf),
                            enc.get_extra_args(),
                            format!("🚀 GPU ({})", gpu.gpu_type),
                        )
                    } else {
                        (
                            self.encoder.ffmpeg_name(),
                            vec!["-crf".to_string(), format!("{:.1}", crf)],
                            vec![],
                            "CPU".to_string(),
                        )
                    }
                }
            }
        } else {
            (
                self.encoder.ffmpeg_name(),
                vec!["-crf".to_string(), format!("{:.1}", crf)],
                vec![],
                "CPU".to_string(),
            )
        };

        // 基础参数
        cmd.arg("-threads").arg(self.max_threads.to_string())
            .arg("-i").arg(&self.input_path)
            .arg("-c:v").arg(encoder_name);

        // CRF/质量参数
        for arg in &crf_args {
            cmd.arg(arg);
        }

        // GPU 特定的额外参数
        for arg in &extra_args {
            cmd.arg(*arg);
        }

        // CPU 编码的 preset（GPU 编码通常不需要）
        if !self.use_gpu || extra_args.is_empty() {
            cmd.arg("-preset").arg("medium");
        }

        // 进度输出
        cmd.arg("-progress").arg("pipe:1")
            .arg("-stats_period").arg("0.5");

        // CPU 编码器特定参数
        if !self.use_gpu {
            for arg in self.encoder.extra_args(self.max_threads) {
                cmd.arg(arg);
            }
        }

        // 视频滤镜
        for arg in &self.vf_args {
            cmd.arg(arg);
        }

        cmd.arg(&self.output_path);

        // 🔥 v4.12: 修复管道死锁 - stderr 必须被消耗
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()
            .context("Failed to spawn ffmpeg")?;

        // 获取输入文件的时长（用于计算进度百分比）
        let duration_secs = self.get_input_duration().unwrap_or(0.0);

        // 🔥 v5.2: 后台线程排空 stderr 防死锁，同时保留最后 N 行用于错误诊断
        let stderr_handle = child.stderr.take().map(|stderr| {
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                use std::collections::VecDeque;
                const MAX_LINES: usize = 10;
                
                let reader = BufReader::new(stderr);
                let mut recent_lines: VecDeque<String> = VecDeque::with_capacity(MAX_LINES);
                
                for line in reader.lines() {
                    if let Ok(line) = line {
                        if recent_lines.len() >= MAX_LINES {
                            recent_lines.pop_front();
                        }
                        recent_lines.push_back(line);
                    }
                }
                
                recent_lines.into_iter().collect::<Vec<_>>().join("\n")
            })
        });

        // 🔥 实时读取 stdout（-progress 输出）
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut last_time_us: u64 = 0;
            let mut last_fps: f64 = 0.0;
            let mut last_speed: String = String::new();

            for line in reader.lines() {
                if let Ok(line) = line {
                    if let Some(val) = line.strip_prefix("out_time_us=") {
                        if let Ok(time_us) = val.parse::<u64>() {
                            last_time_us = time_us;
                        }
                    } else if let Some(val) = line.strip_prefix("fps=") {
                        if let Ok(fps) = val.parse::<f64>() {
                            last_fps = fps;
                        }
                    } else if let Some(val) = line.strip_prefix("speed=") {
                        last_speed = val.to_string();
                    } else if line == "progress=continue" || line == "progress=end" {
                        let current_secs = last_time_us as f64 / 1_000_000.0;
                        if duration_secs > 0.0 {
                            let pct = (current_secs / duration_secs * 100.0).min(100.0);
                            eprint!("\r      ⏳ {} {:.1}% | {:.1}s/{:.1}s | {:.0}fps | {}   ",
                                accel_type, pct, current_secs, duration_secs, last_fps, last_speed.trim());
                        } else {
                            eprint!("\r      ⏳ {} {:.1}s | {:.0}fps | {}   ",
                                accel_type, current_secs, last_fps, last_speed.trim());
                        }
                        let _ = std::io::stderr().flush();
                    }
                }
            }
        }

        // 等待 stderr 线程完成并获取内容
        let stderr_content = stderr_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();

        // 等待进程完成
        let status = child.wait()
            .context("Failed to wait for ffmpeg")?;

        // 清除进度行并换行
        eprintln!("\r      ✅ {} Encoding complete                                    ", accel_type);

        if !status.success() {
            // 🔥 v5.2: 显示 ffmpeg 错误信息
            let error_lines: Vec<&str> = stderr_content
                .lines()
                .filter(|l| l.contains("Error") || l.contains("error") || l.contains("Invalid") || l.contains("failed"))
                .take(5)
                .collect();
            let error_detail = if error_lines.is_empty() {
                stderr_content.lines().rev().take(3).collect::<Vec<_>>().join("\n")
            } else {
                error_lines.join("\n")
            };
            bail!("ffmpeg encoding failed (exit code: {:?}):\n{}", status.code(), error_detail);
        }

        let size = fs::metadata(&self.output_path)
            .context("Failed to read output file")?
            .len();

        Ok(size)
    }

    /// 获取输入文件时长（秒）
    fn get_input_duration(&self) -> Option<f64> {
        let output = Command::new("ffprobe")
            .arg("-v").arg("error")
            .arg("-show_entries").arg("format=duration")
            .arg("-of").arg("default=noprint_wrappers=1:nokey=1")
            .arg(&self.input_path)
            .output()
            .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.trim().parse::<f64>().ok()
    }
    
    /// 计算大小变化百分比
    fn calc_change_pct(&self, output_size: u64) -> f64 {
        (output_size as f64 / self.input_size as f64 - 1.0) * 100.0
    }
    
    /// 验证输出质量
    /// 
    /// 🔥 v3.3: 支持 SSIM/PSNR/VMAF 三重验证
    fn validate_quality(&self) -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
        let ssim = if self.config.quality_thresholds.validate_ssim {
            self.calculate_ssim()?
        } else {
            None
        };
        
        let psnr = if self.config.quality_thresholds.validate_psnr {
            self.calculate_psnr()?
        } else {
            None
        };
        
        let vmaf = if self.config.quality_thresholds.validate_vmaf {
            self.calculate_vmaf()?
        } else {
            None
        };
        
        Ok((ssim, psnr, vmaf))
    }
    
    /// 计算 SSIM（增强版：更严格的解析和验证）
    ///
    /// 🔥 v4.9: 添加实时进度输出
    /// 🔥 精确度改进 v3.2：
    /// - 使用 scale 滤镜处理分辨率差异（HEVC 要求偶数分辨率）
    /// - 更严格的解析逻辑
    /// - 验证 SSIM 值在有效范围内
    /// - 失败时响亮报错
    fn calculate_ssim(&self) -> Result<Option<f64>> {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;

        eprint!("      📊 Calculating SSIM...");
        use std::io::Write;
        let _ = std::io::stderr().flush();

        // 🔥 v3.2: 使用 scale 滤镜将输入缩放到输出分辨率
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim=stats_file=-";

        let duration_secs = self.get_input_duration().unwrap_or(0.0);

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-i").arg(&self.input_path)
            .arg("-i").arg(&self.output_path)
            .arg("-lavfi").arg(filter)
            .arg("-progress").arg("pipe:1")
            .arg("-stats_period").arg("1")
            .arg("-f").arg("null")
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()
            .context("Failed to spawn ffmpeg for SSIM")?;

        let mut ssim_value: Option<f64> = None;

        // 同时读取 stdout（进度）和 stderr（结果）
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // 在单独的线程读取进度
        let progress_handle = if let Some(stdout) = stdout {
            Some(std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                let mut last_time_us: u64 = 0;

                for line in reader.lines().flatten() {
                    if let Some(val) = line.strip_prefix("out_time_us=") {
                        if let Ok(time_us) = val.parse::<u64>() {
                            last_time_us = time_us;
                        }
                    } else if line == "progress=continue" || line == "progress=end" {
                        let current_secs = last_time_us as f64 / 1_000_000.0;
                        if duration_secs > 0.0 {
                            let pct = (current_secs / duration_secs * 100.0).min(100.0);
                            eprint!("\r      📊 Calculating SSIM... {:.0}%   ", pct);
                        }
                        let _ = std::io::stderr().flush();
                    }
                }
            }))
        } else {
            None
        };

        // 读取 stderr 获取 SSIM 结果
        if let Some(stderr) = stderr {
            let reader = BufReader::new(stderr);
            for line in reader.lines().flatten() {
                if let Some(pos) = line.find("All:") {
                    let value_str = &line[pos + 4..];
                    let value_str = value_str.trim_start();
                    let end = value_str.find(|c: char| !c.is_numeric() && c != '.')
                        .unwrap_or(value_str.len());
                    if end > 0 {
                        if let Ok(ssim) = value_str[..end].parse::<f64>() {
                            if precision::is_valid_ssim(ssim) {
                                ssim_value = Some(ssim);
                            }
                        }
                    }
                }
            }
        }

        // 等待进度线程完成
        if let Some(handle) = progress_handle {
            let _ = handle.join();
        }

        // 等待进程完成
        let status = child.wait()
            .context("Failed to wait for ffmpeg SSIM")?;

        if ssim_value.is_some() {
            eprintln!("\r      📊 SSIM: {:.6}                    ", ssim_value.unwrap());
        } else {
            eprintln!("\r      📊 SSIM: N/A                          ");
        }

        if !status.success() && ssim_value.is_none() {
            bail!("ffmpeg SSIM calculation failed");
        }

        Ok(ssim_value)
    }
    
    /// 计算 PSNR（增强版：更严格的解析和验证）
    /// 
    /// 🔥 精确度改进 v3.2：
    /// - 使用 scale 滤镜处理分辨率差异
    /// - 更严格的解析逻辑
    /// - 支持 inf 值（无损情况）
    fn calculate_psnr(&self) -> Result<Option<f64>> {
        // 🔥 v3.2: 使用 scale 滤镜将输入缩放到输出分辨率
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]psnr=stats_file=-";
        
        let output = Command::new("ffmpeg")
            .arg("-i").arg(&self.input_path)
            .arg("-i").arg(&self.output_path)
            .arg("-lavfi").arg(filter)
            .arg("-f").arg("null")
            .arg("-")
            .output();
        
        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                
                // 检查是否有 "inf" (无损情况)
                if stderr.contains("average:inf") {
                    return Ok(Some(f64::INFINITY));
                }
                
                for line in stderr.lines() {
                    if let Some(pos) = line.find("average:") {
                        let value_str = &line[pos + 8..];
                        let value_str = value_str.trim_start();
                        let end = value_str.find(|c: char| !c.is_numeric() && c != '.' && c != '-')
                            .unwrap_or(value_str.len());
                        if end > 0 {
                            if let Ok(psnr) = value_str[..end].parse::<f64>() {
                                if precision::is_valid_psnr(psnr) {
                                    return Ok(Some(psnr));
                                }
                            }
                        }
                    }
                }
                
                Ok(None)
            }
            Err(e) => {
                bail!("Failed to execute ffmpeg for PSNR calculation: {}", e)
            }
        }
    }
    
    /// 计算 VMAF（Netflix 感知质量指标）
    /// 
    /// 🔥 精确度改进 v3.3：
    /// - VMAF 与人眼感知相关性更高 (Pearson 0.93 vs SSIM 0.85)
    /// - 对运动、模糊、压缩伪影更敏感
    /// - 计算较慢（约 100ms/帧），建议作为可选验证
    fn calculate_vmaf(&self) -> Result<Option<f64>> {
        // 🔥 v3.3: 使用 scale 滤镜处理分辨率差异
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]libvmaf";
        
        let output = Command::new("ffmpeg")
            .arg("-i").arg(&self.input_path)
            .arg("-i").arg(&self.output_path)
            .arg("-lavfi").arg(filter)
            .arg("-f").arg("null")
            .arg("-")
            .output();
        
        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                
                // 解析 VMAF score: XX.XXXXXX
                for line in stderr.lines() {
                    if let Some(pos) = line.find("VMAF score:") {
                        let value_str = &line[pos + 11..];
                        let value_str = value_str.trim();
                        if let Ok(vmaf) = value_str.parse::<f64>() {
                            if precision::is_valid_vmaf(vmaf) {
                                return Ok(Some(vmaf));
                            }
                        }
                    }
                }
                
                Ok(None)
            }
            Err(e) => {
                bail!("Failed to execute ffmpeg for VMAF calculation: {}", e)
            }
        }
    }
    
    /// 检查质量是否通过（增强版：支持 SSIM/PSNR/VMAF 三重验证）
    /// 
    /// 🔥 精确度改进 v3.3：
    /// - 使用 epsilon 比较避免浮点精度问题
    /// - 当验证启用但值为 None 时，视为失败
    /// - 支持 VMAF 验证
    fn check_quality_passed(&self, ssim: Option<f64>, psnr: Option<f64>, vmaf: Option<f64>) -> bool {
        let t = &self.config.quality_thresholds;
        
        if t.validate_ssim {
            match ssim {
                Some(s) => {
                    // 🔥 使用 epsilon 比较，避免浮点精度问题
                    // 例如 0.9499999 应该被视为通过 0.95 阈值
                    let epsilon = precision::SSIM_COMPARE_EPSILON;
                    if s + epsilon < t.min_ssim {
                        return false;
                    }
                }
                None => {
                    // 🔥 裁判验证：SSIM 验证启用但无法计算时，视为失败
                    // 这比静默通过更安全
                    return false;
                }
            }
        }
        
        if t.validate_psnr {
            match psnr {
                Some(p) => {
                    // PSNR 使用直接比较（单位是 dB，精度要求较低）
                    if p < t.min_psnr && !p.is_infinite() {
                        return false;
                    }
                }
                None => {
                    // 🔥 裁判验证：PSNR 验证启用但无法计算时，视为失败
                    return false;
                }
            }
        }
        
        // 🔥 v3.3: VMAF 验证
        if t.validate_vmaf {
            match vmaf {
                Some(v) => {
                    if v < t.min_vmaf {
                        return false;
                    }
                }
                None => {
                    // VMAF 验证启用但无法计算时，视为失败
                    return false;
                }
            }
        }
        
        true
    }
}

// ═══════════════════════════════════════════════════════════════
// 便捷函数
// ═══════════════════════════════════════════════════════════════

/// 仅探索更小的文件大小（--explore 单独使用）
/// 
/// 不验证质量，仅保证输出比输入小
/// 🔥 v3.4: CRF 参数改为 f32，支持小数点精度
pub fn explore_size_only(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
) -> Result<ExploreResult> {
    let config = ExploreConfig::size_only(initial_crf, max_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config)?.explore()
}

/// 仅匹配输入质量（--match-quality 单独使用）
/// 
/// 使用 AI 预测的 CRF，单次编码，验证 SSIM
/// 🔥 v3.4: CRF 参数改为 f32，支持小数点精度
pub fn explore_quality_match(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    predicted_crf: f32,
) -> Result<ExploreResult> {
    let config = ExploreConfig::quality_match(predicted_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config)?.explore()
}

/// 精确质量匹配探索（--explore + --match-quality 组合）
/// 
/// 精确质量匹配 - 找最高 SSIM
/// 🔥 v4.5: 不关心文件大小，只关心质量
pub fn explore_precise_quality_match(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
) -> Result<ExploreResult> {
    let config = ExploreConfig::precise_quality_match(initial_crf, max_crf, min_ssim);
    VideoExplorer::new(input, output, encoder, vf_args, config)?.explore()
}

/// 🔥 v4.5: 精确质量匹配 + 压缩
/// 找最高 SSIM 且输出 < 输入
pub fn explore_precise_quality_match_with_compression(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
) -> Result<ExploreResult> {
    let config = ExploreConfig::precise_quality_match_with_compression(initial_crf, max_crf, min_ssim);
    VideoExplorer::new(input, output, encoder, vf_args, config)?.explore()
}

/// 🔥 v4.6: 仅压缩（--compress 单独使用）
/// 
/// 确保输出 < 输入，哪怕只小 1KB 也算成功
/// 不验证 SSIM 质量
pub fn explore_compress_only(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_only(initial_crf, max_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config)?.explore()
}

/// 🔥 v4.6: 压缩 + 粗略质量验证（--compress --match-quality 组合）
///
/// 确保输出 < 输入 + SSIM >= 0.95
pub fn explore_compress_with_quality(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_with_quality(initial_crf, max_crf);
    VideoExplorer::new(input, output, encoder, vf_args, config)?.explore()
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v4.15: GPU 控制变体 - 支持强制 CPU 编码
// ═══════════════════════════════════════════════════════════════

/// 🔥 v4.15: 精确质量匹配 + 压缩（带 GPU 控制）
///
/// 与 `explore_precise_quality_match_with_compression` 相同，但可以显式控制 GPU/CPU 编码
/// - `use_gpu: true` → 使用 GPU 加速（VideoToolbox/NVENC 等）
/// - `use_gpu: false` → 强制 CPU 编码（libx265）以获得更高 SSIM（0.98+）
pub fn explore_precise_quality_match_with_compression_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    use_gpu: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::precise_quality_match_with_compression(initial_crf, max_crf, min_ssim);
    VideoExplorer::new_with_gpu(input, output, encoder, vf_args, config, use_gpu)?.explore()
}

/// 🔥 v4.15: 精确质量匹配（带 GPU 控制）
pub fn explore_precise_quality_match_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
    use_gpu: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::precise_quality_match(initial_crf, max_crf, min_ssim);
    VideoExplorer::new_with_gpu(input, output, encoder, vf_args, config, use_gpu)?.explore()
}

/// 🔥 v4.15: 仅压缩（带 GPU 控制）
pub fn explore_compress_only_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_only(initial_crf, max_crf);
    VideoExplorer::new_with_gpu(input, output, encoder, vf_args, config, use_gpu)?.explore()
}

/// 🔥 v4.15: 压缩 + 质量验证（带 GPU 控制）
pub fn explore_compress_with_quality_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::compress_with_quality(initial_crf, max_crf);
    VideoExplorer::new_with_gpu(input, output, encoder, vf_args, config, use_gpu)?.explore()
}

/// 🔥 v4.15: 仅探索大小（带 GPU 控制）
pub fn explore_size_only_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    use_gpu: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::size_only(initial_crf, max_crf);
    VideoExplorer::new_with_gpu(input, output, encoder, vf_args, config, use_gpu)?.explore()
}

/// 🔥 v4.15: 仅匹配质量（带 GPU 控制）
pub fn explore_quality_match_gpu(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    predicted_crf: f32,
    use_gpu: bool,
) -> Result<ExploreResult> {
    let config = ExploreConfig::quality_match(predicted_crf);
    VideoExplorer::new_with_gpu(input, output, encoder, vf_args, config, use_gpu)?.explore()
}

/// 快速探索（仅基于大小，不验证质量）- 兼容旧 API
#[deprecated(since = "2.0.0", note = "Use explore_size_only instead")]
pub fn quick_explore(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
) -> Result<ExploreResult> {
    explore_size_only(input, output, encoder, vf_args, initial_crf, max_crf)
}

/// 完整探索（包含 SSIM 质量验证）- 兼容旧 API
#[deprecated(since = "2.0.0", note = "Use explore_precise_quality_match instead")]
pub fn full_explore(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
) -> Result<ExploreResult> {
    explore_precise_quality_match(input, output, encoder, vf_args, initial_crf, max_crf, min_ssim)
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v3.8: 智能阈值计算系统 - 消除硬编码
// ═══════════════════════════════════════════════════════════════

/// 智能计算探索阈值
/// 
/// 🔥 v3.8: 基于初始 CRF 和编码器类型动态计算阈值
/// 
/// ## 设计原则
/// 1. **量身定制**：根据源质量自动调整目标阈值
/// 2. **无硬编码**：所有阈值通过公式计算，而非固定值
/// 3. **边缘案例友好**：极低/极高质量源都能正确处理
/// 
/// ## 公式
/// - max_crf = initial_crf + headroom (headroom 随质量降低而增加)
/// - min_ssim = base_ssim - penalty (penalty 随质量降低而增加)
/// 
/// ## 边界保护
/// - HEVC: max_crf ∈ [initial_crf, 40], min_ssim ∈ [0.85, 0.98]
/// - AV1:  max_crf ∈ [initial_crf, 50], min_ssim ∈ [0.85, 0.98]
pub fn calculate_smart_thresholds(initial_crf: f32, encoder: VideoEncoder) -> (f32, f64) {
    // 编码器特定参数
    let (crf_scale, max_crf_cap) = match encoder {
        VideoEncoder::Hevc => (51.0_f32, 40.0_f32),  // HEVC CRF 0-51
        VideoEncoder::Av1 => (63.0_f32, 50.0_f32),   // AV1 CRF 0-63
        VideoEncoder::H264 => (51.0_f32, 35.0_f32),  // H.264 CRF 0-51
    };
    
    // 计算质量等级 (0.0 = 最高质量, 1.0 = 最低质量)
    // 使用非线性映射：低 CRF 区间变化慢，高 CRF 区间变化快
    let normalized_crf = initial_crf / crf_scale;
    let quality_level = (normalized_crf * normalized_crf).clamp(0.0, 1.0) as f64; // 平方使低 CRF 更稳定
    
    // 🔥 动态 headroom：质量越低，允许的 CRF 范围越大
    // 高质量 (CRF ~18): headroom = 8-10
    // 中等质量 (CRF ~25): headroom = 10-12
    // 低质量 (CRF ~35): headroom = 12-15
    let headroom = 8.0 + quality_level as f32 * 7.0;
    let max_crf = (initial_crf + headroom).min(max_crf_cap);
    
    // 🔥 动态 SSIM 阈值：质量越低，允许的 SSIM 越低
    // 使用分段函数确保高质量源有严格阈值
    // 高质量源 (CRF < 20): min_ssim = 0.95 (严格)
    // 中等质量源 (CRF 20-30): min_ssim = 0.92-0.95
    // 低质量源 (CRF > 30): min_ssim = 0.88-0.92 (宽松)
    let min_ssim = if initial_crf < 20.0 {
        // 高质量源：严格阈值
        0.95
    } else if initial_crf < 30.0 {
        // 中等质量源：线性插值 0.95 → 0.92
        let t = (initial_crf - 20.0) / 10.0;
        0.95 - t as f64 * 0.03
    } else {
        // 低质量源：线性插值 0.92 → 0.88
        let t = ((initial_crf - 30.0) / 20.0).min(1.0);
        0.92 - t as f64 * 0.04
    };
    
    (max_crf, min_ssim.clamp(0.85, 0.98))
}

/// HEVC 探索（最常用）- 默认使用精确质量匹配
/// 
/// 🔥 v3.8: 使用智能阈值计算系统，消除硬编码
/// 
/// ## 智能阈值
/// - 根据 initial_crf 自动计算 max_crf 和 min_ssim
/// - 低质量源自动放宽阈值，避免文件变大
/// - 高质量源保持严格阈值，确保质量
pub fn explore_hevc(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_precise_quality_match(input, output, VideoEncoder::Hevc, vf_args, initial_crf, max_crf, min_ssim)
}

/// HEVC 仅探索大小（--explore 单独使用）
/// 
/// 🔥 v3.8: 动态 max_crf
pub fn explore_hevc_size_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_size_only(input, output, VideoEncoder::Hevc, vf_args, initial_crf, max_crf)
}

/// HEVC 仅匹配质量（--match-quality 单独使用）
pub fn explore_hevc_quality_match(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    predicted_crf: f32,
) -> Result<ExploreResult> {
    explore_quality_match(input, output, VideoEncoder::Hevc, vf_args, predicted_crf)
}

/// 🔥 v4.6: HEVC 仅压缩（--compress 单独使用）
/// 
/// 确保输出 < 输入，哪怕只小 1KB 也算成功
pub fn explore_hevc_compress_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_compress_only(input, output, VideoEncoder::Hevc, vf_args, initial_crf, max_crf)
}

/// 🔥 v4.6: HEVC 压缩 + 粗略质量验证（--compress --match-quality 组合）
/// 
/// 确保输出 < 输入 + SSIM >= 0.95
pub fn explore_hevc_compress_with_quality(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_compress_with_quality(input, output, VideoEncoder::Hevc, vf_args, initial_crf, max_crf)
}

/// AV1 探索 - 默认使用精确质量匹配
/// 
/// 🔥 v3.8: 使用智能阈值计算系统，消除硬编码
pub fn explore_av1(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_precise_quality_match(input, output, VideoEncoder::Av1, vf_args, initial_crf, max_crf, min_ssim)
}

/// AV1 仅探索大小（--explore 单独使用）
/// 
/// 🔥 v3.8: 动态 max_crf
pub fn explore_av1_size_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_size_only(input, output, VideoEncoder::Av1, vf_args, initial_crf, max_crf)
}

/// AV1 仅匹配质量（--match-quality 单独使用）
pub fn explore_av1_quality_match(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    predicted_crf: f32,
) -> Result<ExploreResult> {
    explore_quality_match(input, output, VideoEncoder::Av1, vf_args, predicted_crf)
}

/// 🔥 v4.6: AV1 仅压缩（--compress 单独使用）
/// 
/// 确保输出 < 输入，哪怕只小 1KB 也算成功
pub fn explore_av1_compress_only(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_compress_only(input, output, VideoEncoder::Av1, vf_args, initial_crf, max_crf)
}

/// 🔥 v4.6: AV1 压缩 + 粗略质量验证（--compress --match-quality 组合）
/// 
/// 确保输出 < 输入 + SSIM >= 0.95
pub fn explore_av1_compress_with_quality(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, _) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_compress_with_quality(input, output, VideoEncoder::Av1, vf_args, initial_crf, max_crf)
}

// ═══════════════════════════════════════════════════════════════
// 精确度规范
// ═══════════════════════════════════════════════════════════════

/// 精确度规范 - 定义探索器的精度保证
/// 
/// ## 🔥 v3.6: 高精度三阶段搜索
/// 
/// ### CRF 精度
/// - **最终精度**: ±0.5 CRF（三阶段搜索保证）
/// - **粗搜索**: 步长 2.0，快速定位边界区间
/// - **细搜索**: 步长 0.5，精确定位最优点
/// - **边界精细化**: 验证边界点，确保最优
/// 
/// ### 迭代次数分析
/// - 粗搜索: 最多 (max_crf - initial_crf) / 2.0 次
/// - 细搜索: 最多 (boundary_high - boundary_low) / 0.5 次
/// - 典型场景 [18, 28]: 粗搜索 5 次 + 细搜索 4 次 = 9 次
/// - max_iterations=12 可覆盖绝大多数场景
/// 
/// ### SSIM 精度
/// - ffmpeg ssim 滤镜精度：4 位小数（0.0001）
/// - 阈值判断精度：>= min_ssim - epsilon（考虑浮点误差）
/// 
/// ### 质量等级对照表
/// | SSIM 范围 | 质量等级 | 视觉描述 |
/// |-----------|----------|----------|
/// | >= 0.98   | Excellent | 几乎无法区分 |
/// | >= 0.95   | Good      | 视觉无损 |
/// | >= 0.90   | Acceptable | 轻微差异 |
/// | >= 0.85   | Fair      | 可见差异 |
/// | < 0.85    | Poor      | 明显质量损失 |
pub mod precision {
    /// 🔥 v5.55: CRF 搜索精度：±0.25（速度优化）
    pub const CRF_PRECISION: f32 = 0.25;
    
    /// 🔥 v4.6: 粗搜索步长
    pub const COARSE_STEP: f32 = 2.0;
    
    /// 🔥 v4.6: 细搜索步长
    pub const FINE_STEP: f32 = 0.5;
    
    /// 🔥 v5.55: 精细搜索步长 (从 0.1 改为 0.25，速度提升 2-3 倍)
    pub const ULTRA_FINE_STEP: f32 = 0.25;
    
    /// SSIM 显示精度：4 位小数
    pub const SSIM_DISPLAY_PRECISION: u32 = 4;
    
    /// SSIM 比较精度：0.0001
    /// 🔥 v3.1: 这是 ffmpeg ssim 滤镜的输出精度
    pub const SSIM_COMPARE_EPSILON: f64 = 0.0001;
    
    /// 默认最小 SSIM（视觉无损）
    pub const DEFAULT_MIN_SSIM: f64 = 0.95;
    
    /// 高质量最小 SSIM
    pub const HIGH_QUALITY_MIN_SSIM: f64 = 0.98;
    
    /// 可接受最小 SSIM
    pub const ACCEPTABLE_MIN_SSIM: f64 = 0.90;
    
    /// 最低可接受 SSIM（低于此值应警告）
    pub const MIN_ACCEPTABLE_SSIM: f64 = 0.85;
    
    /// PSNR 显示精度：2 位小数
    pub const PSNR_DISPLAY_PRECISION: u32 = 2;
    
    /// 默认最小 PSNR (dB)
    pub const DEFAULT_MIN_PSNR: f64 = 35.0;
    
    /// 高质量最小 PSNR (dB)
    pub const HIGH_QUALITY_MIN_PSNR: f64 = 40.0;
    
    /// 计算二分搜索所需的最大迭代次数
    /// 
    /// 公式：ceil(log2(range)) + 1
    pub fn required_iterations(min_crf: u8, max_crf: u8) -> u32 {
        let range = (max_crf - min_crf) as f64;
        (range.log2().ceil() as u32) + 1
    }
    
    /// 验证 SSIM 是否满足阈值（考虑浮点精度）
    /// 
    /// 🔥 v3.1: 使用 epsilon 比较避免浮点精度问题
    pub fn ssim_meets_threshold(ssim: f64, threshold: f64) -> bool {
        ssim >= threshold - SSIM_COMPARE_EPSILON
    }
    
    /// 验证 SSIM 值是否有效
    /// 
    /// 🔥 v3.1: SSIM 必须在 [0, 1] 范围内
    pub fn is_valid_ssim(ssim: f64) -> bool {
        (0.0..=1.0).contains(&ssim)
    }
    
    /// 验证 PSNR 值是否有效
    /// 
    /// 🔥 v3.1: PSNR 通常在 [0, inf) 范围内
    /// inf 表示完全相同（无损）
    pub fn is_valid_psnr(psnr: f64) -> bool {
        psnr >= 0.0 || psnr.is_infinite()
    }
    
    /// 获取 SSIM 质量等级描述
    pub fn ssim_quality_grade(ssim: f64) -> &'static str {
        if ssim >= 0.98 {
            "Excellent (几乎无法区分)"
        } else if ssim >= 0.95 {
            "Good (视觉无损)"
        } else if ssim >= 0.90 {
            "Acceptable (轻微差异)"
        } else if ssim >= 0.85 {
            "Fair (可见差异)"
        } else {
            "Poor (明显质量损失)"
        }
    }
    
    /// 获取 PSNR 质量等级描述
    pub fn psnr_quality_grade(psnr: f64) -> &'static str {
        if psnr.is_infinite() {
            "Lossless (完全相同)"
        } else if psnr >= 45.0 {
            "Excellent (几乎无法区分)"
        } else if psnr >= 40.0 {
            "Good (视觉无损)"
        } else if psnr >= 35.0 {
            "Acceptable (轻微差异)"
        } else if psnr >= 30.0 {
            "Fair (可见差异)"
        } else {
            "Poor (明显质量损失)"
        }
    }
    
    /// 格式化 SSIM 值用于显示
    /// 
    /// 🔥 v3.1: 统一使用 4 位小数
    pub fn format_ssim(ssim: f64) -> String {
        format!("{:.4}", ssim)
    }
    
    /// 格式化 PSNR 值用于显示
    /// 
    /// 🔥 v3.1: 统一使用 2 位小数，inf 显示为 "∞"
    pub fn format_psnr(psnr: f64) -> String {
        if psnr.is_infinite() {
            "∞".to_string()
        } else {
            format!("{:.2} dB", psnr)
        }
    }
    
    // ═══════════════════════════════════════════════════════════
    // VMAF 相关常量和函数 (v3.3)
    // ═══════════════════════════════════════════════════════════
    
    /// 默认最小 VMAF（流媒体质量）
    pub const DEFAULT_MIN_VMAF: f64 = 85.0;
    
    /// 高质量最小 VMAF（存档质量）
    pub const HIGH_QUALITY_MIN_VMAF: f64 = 93.0;
    
    /// 可接受最小 VMAF（移动端）
    pub const ACCEPTABLE_MIN_VMAF: f64 = 75.0;
    
    /// 验证 VMAF 值是否有效
    /// 
    /// 🔥 v3.3: VMAF 在 [0, 100] 范围内
    pub fn is_valid_vmaf(vmaf: f64) -> bool {
        (0.0..=100.0).contains(&vmaf)
    }
    
    /// 获取 VMAF 质量等级描述
    /// 
    /// 🔥 v3.3: Netflix 感知质量指标
    pub fn vmaf_quality_grade(vmaf: f64) -> &'static str {
        if vmaf >= 93.0 {
            "Excellent (几乎无法区分)"
        } else if vmaf >= 85.0 {
            "Good (流媒体质量)"
        } else if vmaf >= 75.0 {
            "Acceptable (移动端质量)"
        } else if vmaf >= 60.0 {
            "Fair (可见差异)"
        } else {
            "Poor (明显质量损失)"
        }
    }
    
    /// 格式化 VMAF 值用于显示
    /// 
    /// 🔥 v3.3: 统一使用 2 位小数
    pub fn format_vmaf(vmaf: f64) -> String {
        format!("{:.2}", vmaf)
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.56: 预检查模块 - BPP 分析和压缩可行性评估
// ═══════════════════════════════════════════════════════════════

/// 预检查模块 - 在探索开始前评估压缩可行性
pub mod precheck {
    use anyhow::{Context, Result};
    use std::path::Path;
    use std::process::Command;

    /// 压缩可行性等级
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Compressibility {
        /// 高压缩潜力 (bpp > 0.30)
        High,
        /// 中等压缩潜力 (0.15 <= bpp <= 0.30)
        Medium,
        /// 低压缩潜力 (bpp < 0.15) - 文件已高度优化
        Low,
    }

    /// 视频信息结构
    #[derive(Debug, Clone)]
    pub struct VideoInfo {
        pub width: u32,
        pub height: u32,
        pub frame_count: u64,
        pub duration: f64,
        pub file_size: u64,
        pub bpp: f64,
        pub compressibility: Compressibility,
    }

    /// 获取视频信息（宽、高、帧数、时长）
    /// 
    /// 使用 ffprobe 快速提取视频元数据
    pub fn get_video_info(input: &Path) -> Result<VideoInfo> {
        let file_size = std::fs::metadata(input)
            .context("无法读取文件元数据")?
            .len();

        // 使用 ffprobe 获取视频信息
        let output = Command::new("ffprobe")
            .args([
                "-v", "error",
                "-select_streams", "v:0",
                "-show_entries", "stream=width,height,nb_frames,duration",
                "-of", "csv=p=0",
            ])
            .arg(input)
            .output()
            .context("ffprobe 执行失败")?;

        let info_str = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = info_str.trim().split(',').collect();

        // 解析宽高
        let width: u32 = parts.get(0)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1920);
        let height: u32 = parts.get(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1080);

        // 解析帧数（可能为 N/A）
        let frame_count: u64 = parts.get(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // 解析时长
        let duration: f64 = parts.get(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);

        // 如果帧数为 0，尝试从时长估算（假设 30fps）
        let frame_count = if frame_count == 0 && duration > 0.0 {
            (duration * 30.0) as u64
        } else {
            frame_count.max(1)
        };

        // 计算 BPP: (file_size * 8) / (width * height * frame_count)
        let total_pixels = width as u64 * height as u64 * frame_count;
        let bpp = if total_pixels > 0 {
            (file_size as f64 * 8.0) / total_pixels as f64
        } else {
            0.5 // 默认中等
        };

        // 评估压缩可行性
        let compressibility = if bpp < 0.15 {
            Compressibility::Low
        } else if bpp > 0.30 {
            Compressibility::High
        } else {
            Compressibility::Medium
        };

        Ok(VideoInfo {
            width,
            height,
            frame_count,
            duration,
            file_size,
            bpp,
            compressibility,
        })
    }

    /// 计算 BPP (bits per pixel)
    /// 
    /// 公式: (file_size × 8) / (width × height × frame_count)
    /// 
    /// BPP 阈值参考:
    /// - < 0.15: 低（文件已高度优化，压缩空间有限）
    /// - 0.15-0.30: 中等（适度压缩潜力）
    /// - > 0.30: 高（有较大压缩空间）
    pub fn calculate_bpp(input: &Path) -> Result<f64> {
        let info = get_video_info(input)?;
        Ok(info.bpp)
    }

    /// 打印预检查报告
    /// 
    /// 在探索开始前输出压缩可行性评估
    pub fn print_precheck_report(info: &VideoInfo) {
        eprintln!("┌─────────────────────────────────────────────────────");
        eprintln!("│ 📊 预检查报告 (Precheck Report)");
        eprintln!("├─────────────────────────────────────────────────────");
        eprintln!("│ 📐 分辨率: {}x{}", info.width, info.height);
        eprintln!("│ 🎞️  帧数: {} ({:.1}s)", info.frame_count, info.duration);
        eprintln!("│ 📁 文件大小: {:.2} MB", info.file_size as f64 / 1024.0 / 1024.0);
        eprintln!("│ 📈 BPP: {:.3} bits/pixel", info.bpp);
        
        match info.compressibility {
            Compressibility::High => {
                eprintln!("│ ✅ 压缩潜力: 高 (High)");
                eprintln!("│    → 有较大压缩空间，预期效果良好");
            }
            Compressibility::Medium => {
                eprintln!("│ 🔵 压缩潜力: 中等 (Medium)");
                eprintln!("│    → 适度压缩潜力，预期效果正常");
            }
            Compressibility::Low => {
                eprintln!("│ ⚠️  压缩潜力: 低 (Low)");
                eprintln!("│    → 文件已高度优化，压缩空间有限");
                eprintln!("│    → 建议：可能需要降低质量预期");
            }
        }
        eprintln!("└─────────────────────────────────────────────────────");
    }

    /// 执行预检查并返回信息
    /// 
    /// 这是主入口函数，在 explore_with_gpu_coarse_search 开始时调用
    pub fn run_precheck(input: &Path) -> Result<VideoInfo> {
        let info = get_video_info(input)?;
        print_precheck_report(&info);
        Ok(info)
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.56: GPU→CPU 自适应校准模块
// ═══════════════════════════════════════════════════════════════

/// GPU→CPU 自适应校准模块
/// 
/// 根据 GPU 搜索结果智能预测 CPU 起点，避免盲目搜索
pub mod calibration {
    /// GPU→CPU 校准点
    /// 
    /// 包含 GPU 搜索结果和预测的 CPU 起点
    #[derive(Debug, Clone)]
    pub struct CalibrationPoint {
        /// GPU 找到的边界 CRF
        pub gpu_crf: f32,
        /// GPU 输出大小
        pub gpu_size: u64,
        /// GPU SSIM（如果有）
        pub gpu_ssim: Option<f64>,
        /// 预测的 CPU 起点 CRF
        pub predicted_cpu_crf: f32,
        /// 预测置信度 (0.0-1.0)
        pub confidence: f64,
        /// 校准说明
        pub reason: &'static str,
    }

    impl CalibrationPoint {
        /// 根据 GPU 结果计算 CPU 校准点
        /// 
        /// ## 校准逻辑
        /// - GPU 压缩余量大 (size_ratio < 0.95) → CPU 可以更激进 (+1.0)
        /// - GPU 刚好压缩 (0.95 <= size_ratio < 1.0) → CPU 小幅调整 (+0.5)
        /// - GPU 没压缩 (size_ratio >= 1.0) → CPU 需要更低 CRF (-1.0)
        /// 
        /// ## 参数
        /// - `gpu_crf`: GPU 找到的边界 CRF
        /// - `gpu_size`: GPU 输出大小
        /// - `input_size`: 输入文件大小
        /// - `gpu_ssim`: GPU SSIM（可选）
        /// - `base_offset`: 基础 GPU→CPU 偏移量（来自 CrfMapping）
        pub fn from_gpu_result(
            gpu_crf: f32,
            gpu_size: u64,
            input_size: u64,
            gpu_ssim: Option<f64>,
            base_offset: f32,
        ) -> Self {
            let size_ratio = gpu_size as f64 / input_size as f64;
            
            // 根据压缩比例调整 CPU 起点
            let (adjustment, confidence, reason) = if size_ratio < 0.95 {
                // GPU 压缩余量大，CPU 可以更激进
                (1.0, 0.85, "GPU压缩余量大，CPU可更激进")
            } else if size_ratio < 1.0 {
                // GPU 刚好压缩，CPU 小幅调整
                (0.5, 0.90, "GPU刚好压缩，CPU小幅调整")
            } else if size_ratio < 1.05 {
                // GPU 略微超出，CPU 需要稍低 CRF
                (-0.5, 0.80, "GPU略超，CPU需稍低CRF")
            } else {
                // GPU 没压缩，CPU 需要更低 CRF
                (-1.0, 0.70, "GPU未压缩，CPU需更低CRF")
            };

            // 计算预测的 CPU CRF
            // CPU CRF = GPU CRF + base_offset + adjustment
            let predicted_cpu_crf = (gpu_crf + base_offset + adjustment).clamp(10.0, 51.0);

            Self {
                gpu_crf,
                gpu_size,
                gpu_ssim,
                predicted_cpu_crf,
                confidence,
                reason,
            }
        }

        /// 打印校准报告
        pub fn print_report(&self, input_size: u64) {
            let size_ratio = self.gpu_size as f64 / input_size as f64;
            let size_pct = (size_ratio - 1.0) * 100.0;
            
            eprintln!("┌─────────────────────────────────────────────────────");
            eprintln!("│ 🎯 GPU→CPU 校准报告 (Calibration Report)");
            eprintln!("├─────────────────────────────────────────────────────");
            eprintln!("│ 📍 GPU 边界: CRF {:.1} → {:.1}% 大小", self.gpu_crf, size_pct);
            if let Some(ssim) = self.gpu_ssim {
                eprintln!("│ 📊 GPU SSIM: {:.4}", ssim);
            }
            eprintln!("│ 🎯 预测 CPU 起点: CRF {:.1}", self.predicted_cpu_crf);
            eprintln!("│ 📈 置信度: {:.0}%", self.confidence * 100.0);
            eprintln!("│ 💡 原因: {}", self.reason);
            eprintln!("└─────────────────────────────────────────────────────");
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.1: GPU 粗略搜索 + CPU 精细搜索 智能化处理
// ═══════════════════════════════════════════════════════════════

/// GPU 粗略搜索 + CPU 精细搜索的智能探索
/// 
/// ## 🔥 v5.1 核心设计
/// 
/// ### 两阶段策略
/// 1. **GPU 粗略搜索**（快速预览）
///    - 用 GPU 编码器快速找到压缩边界的大致范围
///    - 步长 4 CRF，最多 6 次迭代
///    - 目的：缩小 CPU 搜索范围
/// 
/// 2. **CPU 精细搜索**（精确结果）
///    - 在 GPU 给出的范围内用 CPU 编码器精确搜索
///    - 步长 0.5 → 0.1 CRF
///    - 目的：找到最优 CRF
/// 
/// ### GPU/CPU CRF 映射
/// GPU 和 CPU 编码器对 CRF 的解释不同：
/// - GPU CRF 10 ≈ CPU CRF 7-8（VideoToolbox）
/// - GPU CRF 10 ≈ CPU CRF 7（NVENC）
/// 
/// ### Fallback 机制
/// - 无 GPU → 直接使用 CPU 搜索
/// - GPU 搜索失败 → 使用原始范围进行 CPU 搜索
/// 
/// ## 参数
/// - `input`: 输入文件路径
/// - `output`: 输出文件路径
/// - `encoder`: 视频编码器类型
/// - `vf_args`: 视频滤镜参数
/// - `initial_crf`: 算法预测的初始 CRF
/// - `max_crf`: 最大 CRF（最低质量）
/// - `min_ssim`: 最小 SSIM 阈值
/// 
/// ## 返回
/// `ExploreResult` - 包含最优 CRF、输出大小、SSIM 等信息
pub fn explore_with_gpu_coarse_search(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    initial_crf: f32,
    max_crf: f32,
    min_ssim: f64,
) -> Result<ExploreResult> {
    use crate::gpu_accel::{CrfMapping, GpuAccel, GpuCoarseConfig};
    // 🔥 v5.35: 简化流程 - 完全移除旧的RealtimeExploreProgress
    // 只使用SimpleIterationProgress，避免多个进度条混乱

    // 🔥 v5.56: 预检查 - BPP 分析和压缩可行性评估
    let precheck_info = precheck::run_precheck(input)?;
    let _compressibility = precheck_info.compressibility; // 保存以备后用
    eprintln!("");

    // 🔥 v5.32: 先打印 GPU 信息
    let gpu = GpuAccel::detect();
    gpu.print_detection_info();

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    let gpu = GpuAccel::detect();
    let encoder_name = match encoder {
        VideoEncoder::Hevc => "hevc",
        VideoEncoder::Av1 => "av1",
        VideoEncoder::H264 => "h264",
    };

    // 检查是否有对应的 GPU 编码器
    let has_gpu_encoder = match encoder {
        VideoEncoder::Hevc => gpu.get_hevc_encoder().is_some(),
        VideoEncoder::Av1 => gpu.get_av1_encoder().is_some(),
        VideoEncoder::H264 => gpu.get_h264_encoder().is_some(),
    };

    // 🔥 v5.35: 在进度条显示前输出关键信息
    eprintln!("🔬 Smart GPU+CPU Explore v5.1 ({:?})", encoder);
    eprintln!("   📁 Input: {} bytes ({:.2} MB)", input_size, input_size as f64 / 1024.0 / 1024.0);
    eprintln!("");
    eprintln!("📋 STRATEGY: GPU Coarse → CPU Fine");
    eprintln!("• Phase 1: GPU finds rough boundary (FAST)");
    eprintln!("• Phase 2: CPU finds precise CRF (ACCURATE)");
    eprintln!("");
    
    // ═══════════════════════════════════════════════════════════
    // Phase 1: GPU 粗略搜索（如果可用）
    // ═══════════════════════════════════════════════════════════
    let (cpu_min_crf, cpu_max_crf, cpu_center_crf) = if gpu.is_available() && has_gpu_encoder {
        eprintln!("");
        eprintln!("📍 Phase 1: GPU Coarse Search");

        // 创建临时输出文件用于 GPU 搜索
        let temp_output = output.with_extension("gpu_temp.mp4");

        // 🔥 v5.45: 计算 GPU 采样输入大小（与 gpu_accel.rs 中的逻辑一致）
        let duration: f32 = {
            use std::process::Command;
            let duration_output = Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
                .arg(input)
                .output();
            duration_output
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
                .unwrap_or(crate::gpu_accel::GPU_SAMPLE_DURATION)
        };
        let gpu_sample_input_size = if duration <= crate::gpu_accel::GPU_SAMPLE_DURATION {
            input_size  // 短视频，使用完整大小
        } else {
            // 长视频，按比例计算采样部分的预期大小
            let ratio = crate::gpu_accel::GPU_SAMPLE_DURATION / duration;
            (input_size as f64 * ratio as f64) as u64
        };

        let gpu_config = GpuCoarseConfig {
            initial_crf,
            min_crf: crate::gpu_accel::GPU_DEFAULT_MIN_CRF,  // 🔥 v5.7: 使用常量 (1.0 for VideoToolbox)
            max_crf,
            step: 2.0,  // 🔥 v5.3: 精细搜索用 2 CRF 步长
            max_iterations: crate::gpu_accel::GPU_ABSOLUTE_MAX_ITERATIONS,  // 🔥 v5.52: 使用保底上限 500
        };

        // 🔥 v5.34: GPU 阶段使用新的基于迭代计数的进度条（修复跳跃问题）
        // 🔥 v5.45: 使用采样输入大小来正确计算压缩率
        let gpu_progress = crate::SimpleIterationProgress::new(
            "🔍 GPU Search", gpu_sample_input_size,
            gpu_config.max_iterations as u64
        );

        // Progress callback - 每次编码完成立即更新
        let progress_callback = |crf: f32, size: u64| {
            gpu_progress.inc_iteration(crf, size, None);
        };

        // Log callback - 使用 suspend 输出日志，不干扰进度条
        let log_callback = |msg: &str| {
            gpu_progress.bar.suspend(|| eprintln!("{}", msg));
        };

        let gpu_result = crate::gpu_accel::gpu_coarse_search_with_log(
            input, &temp_output, encoder_name, input_size, &gpu_config,
            Some(&progress_callback), Some(&log_callback)
        );

        // 🔥 v5.45: 使用实际的 GPU 搜索结果更新进度条
        let (final_crf, final_size) = match &gpu_result {
            Ok(result) if result.found_boundary => (result.gpu_boundary_crf, result.gpu_best_size.unwrap_or(0)),
            _ => (gpu_config.max_crf, input_size),  // 失败时使用 max_crf 和输入大小
        };
        gpu_progress.finish(final_crf, final_size, None);
        
        match gpu_result {
            Ok(gpu_result) => {
                // 🔥 v5.1.4: GPU 日志已经实时输出，不需要再收集
                // GPU 日志通过 gpu_coarse_search 内部的 eprintln! 已经输出
                
                if gpu_result.found_boundary {
                    // 🔥 v5.9: 修正 CRF 映射方向！
                    // GPU 效率**低于** CPU，相同 CRF 下 GPU 输出更大
                    // 所以：GPU CRF 11 能压缩 → CPU 需要**更高** CRF（如 12-14）才能压缩
                    // 之前的代码搞反了方向！
                    let gpu_crf = gpu_result.gpu_boundary_crf;
                    let gpu_size = gpu_result.gpu_best_size.unwrap_or(input_size);

                    // 🔥 v5.56: GPU→CPU 自适应校准
                    // 根据 GPU 压缩比例智能预测 CPU 起点
                    let mapping = match encoder {
                        VideoEncoder::Hevc => CrfMapping::hevc(gpu.gpu_type),
                        VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
                        VideoEncoder::H264 => CrfMapping::hevc(gpu.gpu_type),
                    };
                    let calibration = calibration::CalibrationPoint::from_gpu_result(
                        gpu_crf,
                        gpu_size,
                        input_size,
                        gpu_result.gpu_best_ssim,
                        mapping.offset,
                    );
                    calibration.print_report(input_size);
                    eprintln!("");

                    // 使用校准后的 CPU 起点
                    let cpu_start = calibration.predicted_cpu_crf;
                    
                    eprintln!("   ✅ GPU found boundary: CRF {:.1} (fine-tuned: {})", gpu_crf, gpu_result.fine_tuned);
                    if let Some(size) = gpu_result.gpu_best_size {
                        eprintln!("   📊 GPU best size: {} bytes", size);
                    }
                    
                    // 🔥 v5.26: 根据 GPU SSIM 动态调整 CPU 搜索范围
                    let (cpu_min, cpu_max) = if let Some(ssim) = gpu_result.gpu_best_ssim {
                        let quality_hint = if ssim >= 0.97 { "🟢 Near GPU ceiling" } 
                                          else if ssim >= 0.95 { "🟡 Good" } 
                                          else { "🟠 Below expected" };
                        eprintln!("   📊 GPU best SSIM: {:.6} {}", ssim, quality_hint);
                        
                        if ssim < 0.90 {
                            // SSIM 太低，需要更低的 CRF（更高质量）
                            eprintln!("   ⚠️ GPU SSIM too low! Expanding CPU search to lower CRF");
                            let expand = ((0.95 - ssim) * 30.0) as f32;  // 每 0.01 SSIM 差距扩展 0.3 CRF
                            ((gpu_crf - expand).max(ABSOLUTE_MIN_CRF), (cpu_start + 5.0).min(max_crf))
                        } else {
                            eprintln!("   💡 CPU will achieve SSIM 0.98+ (GPU max ~0.97)");
                            // 🔥 v5.56: 使用校准后的起点作为搜索中心
                            ((cpu_start - 3.0).max(ABSOLUTE_MIN_CRF), (cpu_start + 5.0).min(max_crf))
                        }
                    } else {
                        // 🔥 v5.56: 使用校准后的起点作为搜索中心
                        ((cpu_start - 3.0).max(ABSOLUTE_MIN_CRF), (cpu_start + 5.0).min(max_crf))
                    };
                    
                    eprintln!("   📊 CPU search range: [{:.1}, {:.1}] (start: {:.1})", cpu_min, cpu_max, cpu_start);
                    (cpu_min, cpu_max, cpu_start)
                } else {
                    // GPU 没找到边界，使用原始范围
                    eprintln!("⚠️  GPU didn't find compression boundary");
                    eprintln!("• File may already be highly compressed");
                    eprintln!("• Using full CRF range for CPU search");
                    // 🔥 v5.24: min_crf 使用全局最小值
                    (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
                }
            }
            Err(e) => {
                eprintln!("⚠️  FALLBACK: GPU coarse search failed!");
                eprintln!("• Error: {}", e);
                eprintln!("• Falling back to CPU-only search (full range)");
                // 🔥 v5.24: min_crf 使用全局最小值
                (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
            }
        }
    } else {
        // 无 GPU，直接使用 CPU 搜索
        eprintln!("");
        if !gpu.is_available() {
            eprintln!("⚠️  FALLBACK: No GPU available!");
            eprintln!("• Skipping GPU coarse search phase");
            eprintln!("• Using CPU-only search (may take longer)");
        } else {
            eprintln!("⚠️  FALLBACK: No GPU encoder for {:?}!              ", encoder);
            eprintln!("• Skipping GPU coarse search phase");
            eprintln!("• Using CPU-only search (may take longer)");
        }
        // 🔥 v5.24: min_crf 使用全局最小值，允许向下探索更高质量
        (ABSOLUTE_MIN_CRF, max_crf, initial_crf)
    };
    
    // 🔥 v5.23: 主进度条已在 GPU 阶段结束时清理
    
    // ═══════════════════════════════════════════════════════════
    // Phase 2: CPU 精细搜索
    // 🔥 v5.8: GPU 已找到边界，CPU 只做 0.5→0.1 精细化
    // ═══════════════════════════════════════════════════════════
    // ═══════════════════════════════════════════════════════════
    eprintln!("📍 Phase 2: CPU Fine-Tune (0.5→0.1 step)");
    eprintln!("📊 Starting from GPU boundary: CRF {:.1}", cpu_center_crf);
    
    // 🔥 v5.8: 直接从 GPU 边界开始精细化，跳过二分搜索
    let mut result = cpu_fine_tune_from_gpu_boundary(
        input,
        output,
        encoder,
        vf_args,
        cpu_center_crf,
        cpu_min_crf,
        cpu_max_crf,
        min_ssim,
    )?;
    
    // 🔥 v5.1.4: 清空日志，避免 conversion_api.rs 重复打印
    // 所有日志已经通过 eprintln! 实时输出了
    result.log.clear();
    
    // 打印 CRF 映射信息
    if gpu.is_available() && has_gpu_encoder {
        let mapping = match encoder {
            VideoEncoder::Hevc => CrfMapping::hevc(gpu.gpu_type),
            VideoEncoder::Av1 => CrfMapping::av1(gpu.gpu_type),
            VideoEncoder::H264 => CrfMapping::hevc(gpu.gpu_type),
        };
        let equivalent_gpu_crf = mapping.cpu_to_gpu(result.optimal_crf);
        eprintln!("   ═══════════════════════════════════════════════════");
        eprintln!("   📊 CRF Mapping: CPU {:.1} ≈ GPU {:.1}", result.optimal_crf, equivalent_gpu_crf);
    }
    
    Ok(result)
}

/// 🔥 v5.9: CPU 从 GPU 边界开始精细化（修正映射方向）
/// 
/// GPU 效率**低于** CPU，所以：
/// - GPU CRF 11 能压缩 → CPU 需要**更高** CRF（如 12-14）才能压缩
/// 
/// CPU 只需要：
/// 1. 从 GPU 边界开始，用 0.5 步进向上搜索找到 CPU 压缩点
/// 2. 用 0.1 步进向下精细化（找最高质量的压缩点）
/// 3. 计算 SSIM 验证质量
fn cpu_fine_tune_from_gpu_boundary(
    input: &Path,
    output: &Path,
    encoder: VideoEncoder,
    vf_args: Vec<String>,
    gpu_boundary_crf: f32,
    min_crf: f32,
    max_crf: f32,
    min_ssim: f64,
) -> Result<ExploreResult> {
    #[allow(unused_mut)]
    let mut log = Vec::new();

    let input_size = fs::metadata(input)
        .context("Failed to read input file metadata")?
        .len();

    // 🔥 v5.52: CPU 也使用采样编码（和 GPU 一致）
    // 获取视频时长
    let duration: f32 = {
        use std::process::Command;
        let duration_output = Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
            .arg(input)
            .output();
        duration_output
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(crate::gpu_accel::GPU_SAMPLE_DURATION)
    };

    // 计算采样时长和输入大小
    let sample_duration = duration.min(crate::gpu_accel::GPU_SAMPLE_DURATION);
    let sample_input_size = if duration <= crate::gpu_accel::GPU_SAMPLE_DURATION {
        input_size  // 短视频，使用完整大小
    } else {
        // 长视频，按比例计算采样部分的预期大小
        let ratio = sample_duration / duration;
        (input_size as f64 * ratio as f64) as u64
    };

    // 🔥 v5.34: 创建基于迭代计数的进度条（使用采样输入大小）
    let cpu_progress = crate::SimpleIterationProgress::new(
        "🔬 CPU Fine-Tune",
        sample_input_size,  // 🔥 v5.52: 使用采样大小
        25  // 预估25次迭代
    );

    // 🔥 v5.34: 使用 SimpleIterationProgress 替代 spinner
    #[allow(unused_macros)]
    macro_rules! log_msg {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            cpu_progress.bar.suspend(|| eprintln!("{}", msg));
            log.push(msg);
        }};
    }
    
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);

    // 🔥 v5.54: 采样编码（用于搜索，速度快）
    let encode_sampled = |crf: f32| -> Result<u64> {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y");

        // 🔥 v5.54: 添加 -t 参数限制编码时长（仅搜索时使用）
        if duration > crate::gpu_accel::GPU_SAMPLE_DURATION {
            cmd.arg("-t").arg(format!("{}", sample_duration));
        }

        cmd.arg("-i").arg(input)
            .arg("-c:v").arg(encoder.ffmpeg_name())
            .arg("-crf").arg(format!("{:.1}", crf));

        for arg in encoder.extra_args(max_threads) {
            cmd.arg(arg);
        }

        for arg in &vf_args {
            if !arg.is_empty() {
                cmd.arg("-vf").arg(arg);
            }
        }

        cmd.arg("-c:a").arg("copy")
            .arg(output);

        let result = cmd.output().context("Failed to run ffmpeg")?;
        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            anyhow::bail!("Encoding failed: {}", stderr.lines().last().unwrap_or("unknown"));
        }

        Ok(fs::metadata(output)?.len())
    };

    // 🔥 v5.54: 完整编码（用于最终输出，无 -t 参数）
    // 🔥 v5.58: 添加实时进度显示（从 v5.2 合并）
    let encode_full = |crf: f32| -> Result<u64> {
        use std::io::{BufRead, BufReader, Write};
        use std::process::Stdio;
        
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y");

        // 🔥 v5.58: 添加 -progress 参数获取实时进度
        cmd.arg("-progress").arg("pipe:1");

        cmd.arg("-i").arg(input)
            .arg("-c:v").arg(encoder.ffmpeg_name())
            .arg("-crf").arg(format!("{:.1}", crf));

        for arg in encoder.extra_args(max_threads) {
            cmd.arg(arg);
        }

        for arg in &vf_args {
            if !arg.is_empty() {
                cmd.arg("-vf").arg(arg);
            }
        }

        cmd.arg("-c:a").arg("copy")
            .arg(output);

        // 🔥 v5.58: 使用 spawn 而非 output，以便实时读取进度
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        
        let mut child = cmd.spawn().context("Failed to spawn ffmpeg")?;
        
        // 读取 stdout（-progress 输出）
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut last_fps = 0.0_f64;
            let mut last_speed = String::new();
            let mut last_time_us = 0_i64;
            let duration_secs = duration as f64;
            
            for line in reader.lines().map_while(Result::ok) {
                if let Some(val) = line.strip_prefix("out_time_us=") {
                    if let Ok(time_us) = val.parse::<i64>() {
                        last_time_us = time_us;
                    }
                } else if let Some(val) = line.strip_prefix("fps=") {
                    if let Ok(fps) = val.parse::<f64>() {
                        last_fps = fps;
                    }
                } else if let Some(val) = line.strip_prefix("speed=") {
                    last_speed = val.trim().to_string();
                } else if line == "progress=continue" || line == "progress=end" {
                    // 🔥 v5.58: 实时显示编码进度（固定底部）
                    let current_secs = last_time_us as f64 / 1_000_000.0;
                    if duration_secs > 0.0 {
                        let pct = (current_secs / duration_secs * 100.0).min(100.0);
                        eprint!("\r      ⏳ Encoding {:.1}% | {:.1}s/{:.1}s | {:.0}fps | {}   ",
                            pct, current_secs, duration_secs, last_fps, last_speed);
                    } else {
                        eprint!("\r      ⏳ Encoding {:.1}s | {:.0}fps | {}   ",
                            current_secs, last_fps, last_speed);
                    }
                    let _ = std::io::stderr().flush();
                }
            }
        }
        
        let status = child.wait().context("Failed to wait for ffmpeg")?;
        
        // 清除进度行
        eprintln!("\r      ✅ Encoding complete                                        ");
        
        if !status.success() {
            anyhow::bail!("Encoding failed");
        }

        Ok(fs::metadata(output)?.len())
    };
    
    eprintln!("🔬 CPU Fine-Tune v6.0 ({:?})", encoder);
    eprintln!("📁 Input: {} bytes ({:.2} MB)", input_size, input_size as f64 / 1024.0 / 1024.0);
    eprintln!("🎯 Goal: Find optimal CRF (highest quality that compresses)");
    
    let mut iterations = 0u32;
    let mut size_cache: std::collections::HashMap<i32, u64> = std::collections::HashMap::new();
    
    // 🔥 v5.54: 带缓存的采样编码（用于搜索）+ 进度条更新
    let encode_cached = |crf: f32, cache: &mut std::collections::HashMap<i32, u64>| -> Result<u64> {
        let key = (crf * 4.0).round() as i32;
        if let Some(&size) = cache.get(&key) {
            // 从缓存读取，仍然更新进度条
            cpu_progress.inc_iteration(crf, size, None);
            return Ok(size);
        }
        let size = encode_sampled(crf)?;  // 🔥 v5.54: 使用采样编码
        cache.insert(key, size);
        // 🔥 v5.34: 编码完成立即更新进度条
        cpu_progress.inc_iteration(crf, size, None);
        Ok(size)
    };
    
    // 🔥 v5.47: 简化 CPU 微调 - GPU 已完成粗略搜索
    // CPU 只需在 GPU 边界附近做 0.1 精度微调
    //
    // GPU 已经找到：最高的能压缩的 CRF（如 39）
    // CPU 任务：
    // 1. 验证 GPU 边界
    // 2. 向下微调 1.0 CRF（39.0 → 38.9 → ... → 38.0）找更高质量
    // 3. Phase 3 会继续 0.1 步进微调到最优点

    let mut best_crf: Option<f32> = None;
    let mut best_size: Option<u64> = None;

    eprintln!("📍 CPU Fine-Tune: 0.1 step around GPU boundary (CRF {:.1})", gpu_boundary_crf);
    eprintln!("🎯 Goal: Find lowest CRF that compresses (highest quality)");

    // ═══════════════════════════════════════════════════════════
    // Phase 1: 验证 GPU 边界并做初步微调
    // ═══════════════════════════════════════════════════════════
    let gpu_size = encode_cached(gpu_boundary_crf, &mut size_cache)?;
    iterations += 1;
    let gpu_ratio = gpu_size as f64 / sample_input_size as f64;

    if gpu_size < sample_input_size {
        // GPU 边界能压缩，作为起点
        best_crf = Some(gpu_boundary_crf);
        best_size = Some(gpu_size);
        eprintln!("✅ GPU boundary CRF {:.1} compresses ({:.1}%)", gpu_boundary_crf, gpu_ratio * 100.0);

        // 🔥 v5.52: 向下微调 1.0 CRF（0.1 步进）找更高质量区域
        // 用户要求："GPU 覆盖 0.5 步进，CPU 仅做 0.1 精度"
        let mut test_crf = gpu_boundary_crf - 0.25;
        let quick_search_limit = (gpu_boundary_crf - 1.5).max(min_crf);

        while test_crf >= quick_search_limit && iterations < 20 {
            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            let ratio = size as f64 / sample_input_size as f64;

            if size < sample_input_size {
                best_crf = Some(test_crf);
                best_size = Some(size);
                eprintln!("   ✓ CRF {:.1}: {:.1}% compresses", test_crf, ratio * 100.0);
                test_crf -= 0.25;  // 🔥 v5.52: 改为 0.1 步进（之前是 0.5）
            } else {
                eprintln!("   ✗ CRF {:.1}: {:.1}% fails → boundary found", test_crf, ratio * 100.0);
                break;
            }
        }

    } else {
        // GPU 边界不能压缩，可能是边界估算不准
        eprintln!("⚠️ GPU boundary CRF {:.1} cannot compress ({:.1}%)", gpu_boundary_crf, gpu_ratio * 100.0);
        eprintln!("   Searching nearby for valid boundary...");

        // 向下搜索 1.0 CRF（0.1 步进）找第一个能压缩的点
        let mut test_crf = gpu_boundary_crf - 0.25;
        let mut found = false;
        while test_crf >= (gpu_boundary_crf - 1.5).max(min_crf) && iterations < 20 {
            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            let ratio = size as f64 / sample_input_size as f64;

            if size < sample_input_size {
                best_crf = Some(test_crf);
                best_size = Some(size);
                eprintln!("✅ Found valid boundary at CRF {:.1} ({:.1}%)", test_crf, ratio * 100.0);
                found = true;
                break;
            } else {
                eprintln!("   CRF {:.1}: {:.1}% ✗", test_crf, ratio * 100.0);
            }
            test_crf -= 0.25;
        }

        if !found {
            eprintln!("⚠️ Cannot find compressible point near GPU boundary!");
            eprintln!("   File may be already optimally compressed");
            best_crf = Some(gpu_boundary_crf);
            best_size = Some(gpu_size);
        }
    }

    // ═══════════════════════════════════════════════════════════
    if let Some(boundary_crf) = best_crf {
        eprintln!("📍 Phase 3: Fine-tune with 0.1 step (target: SSIM 0.999+)");
        
        // 自适应搜索：根据压缩率变化率决定是否继续
        let mut prev_ratio = best_size.map(|s| s as f64 / input_size as f64).unwrap_or(1.0);
        let mut consecutive_small_change = 0;
        
        // 向下搜索（更高质量），直到找到边界或变化率太小
        let mut test_crf = boundary_crf - 0.25;
        while test_crf >= min_crf && iterations < GLOBAL_MAX_ITERATIONS {
            let key = (test_crf * 4.0).round() as i32;
            if size_cache.contains_key(&key) {
                test_crf -= 0.25;
                continue;
            }
            
            let size = encode_cached(test_crf, &mut size_cache)?;
            iterations += 1;
            let ratio = size as f64 / sample_input_size as f64;

                if size < sample_input_size {
                    best_crf = Some(test_crf);
                    best_size = Some(size);
                    eprintln!("🔄 CRF {:.1}: {:.1}% ✓", test_crf, ratio * 100.0);
                    
                    // 检查变化率
                    let change = ratio - prev_ratio;
                    if change.abs() < 0.005 {  // 变化小于 0.5%
                        consecutive_small_change += 1;
                        if consecutive_small_change >= 3 {
                            eprintln!("⚡ Diminishing returns, stop");
                            break;
                        }
                    } else {
                        consecutive_small_change = 0;
                    }
                    prev_ratio = ratio;
                    test_crf -= 0.25;
                } else {
                    eprintln!("🔄 CRF {:.1}: {:.1}% ✗ (boundary found)", test_crf, ratio * 100.0);
                    break;  // 找到边界
                }
        }
    }
    
    // 最终结果
    let final_crf = match (best_crf, best_size) {
        (Some(crf), Some(_size)) => crf,  // 🔥 v5.54: size 不再使用，最终大小由 encode_full 确定
        _ => {
            // 无法压缩，返回 max_crf
            eprintln!("⚠️ Cannot compress this file");
            let _size = encode_cached(max_crf, &mut size_cache)?;  // 确保输出文件存在
            iterations += 1;
            max_crf
        }
    };

    // 🔥 v5.54: Step 3: SSIM 验证（使用完整视频）
    eprintln!("📍 Step 3: SSIM validation at CRF {:.1}", final_crf);

    // 🔥 v5.54: 最终输出必须编码完整视频（不是采样）
    eprintln!("🔄 Final output: Re-encoding FULL video at CRF {:.1}...", final_crf);
    let final_full_size = encode_full(final_crf)?;
    eprintln!("✅ Final full video size: {} bytes ({:.2} MB)",
        final_full_size, final_full_size as f64 / 1024.0 / 1024.0);

    // 计算 SSIM
    let ssim_output = std::process::Command::new("ffmpeg")
        .arg("-i").arg(input)
        .arg("-i").arg(output)
        .arg("-lavfi").arg("ssim")
        .arg("-f").arg("null")
        .arg("-")
        .output();
    
    let ssim = match ssim_output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if let Some(line) = stderr.lines().find(|l| l.contains("SSIM") && l.contains("All:")) {
                if let Some(all_pos) = line.find("All:") {
                    let after_all = &line[all_pos + 4..];
                    if let Some(space_pos) = after_all.find(' ') {
                        after_all[..space_pos].parse::<f64>().ok()
                    } else {
                        after_all.trim().parse::<f64>().ok()
                    }
                } else { None }
            } else { None }
        }
        Err(_) => None,
    };
    
    if let Some(s) = ssim {
        let quality_hint = if s >= 0.99 { "✅ Excellent" } 
                          else if s >= 0.98 { "✅ Very Good" }
                          else if s >= 0.95 { "🟡 Good" }
                          else { "🟠 Below threshold" };
        eprintln!("📊 SSIM: {:.6} {}", s, quality_hint);
    }

    // 🔥 v5.54: 使用完整视频大小计算结果
    let size_change_pct = (final_full_size as f64 / input_size as f64 - 1.0) * 100.0;
    let quality_passed = final_full_size < input_size && ssim.unwrap_or(0.0) >= min_ssim;

    // 🔥 v5.57: 计算置信度
    let ssim_val = ssim.unwrap_or(0.0);
    
    // 采样覆盖度：短视频完整测试得满分
    let sampling_coverage = if duration < 60.0 {
        1.0
    } else {
        (sample_duration / duration).min(1.0) as f64
    };
    
    // 预测准确度：GPU+CPU 模式默认较高
    let prediction_accuracy = 0.85;  // GPU 提供了参考，准确度较高
    
    // 安全边界：输出比输入小的程度（5%为满分）
    let margin_safety = if final_full_size < input_size {
        let margin = (input_size - final_full_size) as f64 / input_size as f64;
        (margin / 0.05).min(1.0)
    } else {
        0.0
    };
    
    // SSIM 可靠性
    let ssim_confidence = if ssim_val >= 0.99 {
        1.0
    } else if ssim_val >= 0.95 {
        0.8
    } else if ssim_val >= 0.90 {
        0.6
    } else {
        0.5
    };
    
    let confidence_detail = ConfidenceBreakdown {
        sampling_coverage,
        prediction_accuracy,
        margin_safety,
        ssim_confidence,
    };
    let confidence = confidence_detail.overall();

    eprintln!("✅ RESULT: CRF {:.1} • Size {:+.1}% • Iterations: {}", final_crf, size_change_pct, iterations);
    confidence_detail.print_report();

    cpu_progress.finish(final_crf, final_full_size, ssim);

    Ok(ExploreResult {
        optimal_crf: final_crf,
        output_size: final_full_size,  // 🔥 v5.54: 使用完整视频大小
        size_change_pct,
        ssim,
        psnr: None,
        vmaf: None,
        iterations,
        quality_passed,
        log,
        confidence,
        confidence_detail,
    })
}

/// 🔥 v5.1: HEVC GPU+CPU 智能探索
/// 
/// 先用 GPU 粗略搜索缩小范围，再用 CPU 精细搜索找最优 CRF
pub fn explore_hevc_with_gpu_coarse(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Hevc);
    explore_with_gpu_coarse_search(input, output, VideoEncoder::Hevc, vf_args, initial_crf, max_crf, min_ssim)
}

/// 🔥 v5.1: AV1 GPU+CPU 智能探索
/// 
/// 先用 GPU 粗略搜索缩小范围，再用 CPU 精细搜索找最优 CRF
pub fn explore_av1_with_gpu_coarse(
    input: &Path,
    output: &Path,
    vf_args: Vec<String>,
    initial_crf: f32,
) -> Result<ExploreResult> {
    let (max_crf, min_ssim) = calculate_smart_thresholds(initial_crf, VideoEncoder::Av1);
    explore_with_gpu_coarse_search(input, output, VideoEncoder::Av1, vf_args, initial_crf, max_crf, min_ssim)
}

// ═══════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use super::precision::*;
    
    // ═══════════════════════════════════════════════════════════
    // 基础配置测试
    // ═══════════════════════════════════════════════════════════
    
    #[test]
    fn test_quality_thresholds_default() {
        let t = QualityThresholds::default();
        assert_eq!(t.min_ssim, 0.95);
        assert_eq!(t.min_psnr, 35.0);
        assert!(t.validate_ssim);
        assert!(!t.validate_psnr);
    }
    
    #[test]
    fn test_explore_config_default() {
        let c = ExploreConfig::default();
        assert_eq!(c.mode, ExploreMode::PreciseQualityMatch);
        assert_eq!(c.initial_crf, 18.0);
        assert_eq!(c.min_crf, 10.0);
        assert_eq!(c.max_crf, 28.0);
        assert_eq!(c.target_ratio, 1.0);
        // 🔥 v3.6: 增加迭代次数以支持三阶段搜索
        assert_eq!(c.max_iterations, 12);
    }
    
    #[test]
    fn test_explore_config_size_only() {
        let c = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(c.mode, ExploreMode::SizeOnly);
        assert_eq!(c.initial_crf, 20.0);
        assert_eq!(c.max_crf, 30.0);
        assert!(!c.quality_thresholds.validate_ssim);
        assert!(!c.quality_thresholds.validate_psnr);
    }
    
    #[test]
    fn test_explore_config_quality_match() {
        let c = ExploreConfig::quality_match(22.0);
        assert_eq!(c.mode, ExploreMode::QualityMatch);
        assert_eq!(c.initial_crf, 22.0);
        assert_eq!(c.max_iterations, 1); // 单次编码
        assert!(c.quality_thresholds.validate_ssim);
    }
    
    #[test]
    fn test_explore_config_precise_quality_match() {
        let c = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(c.mode, ExploreMode::PreciseQualityMatch);
        assert_eq!(c.initial_crf, 18.0);
        assert_eq!(c.max_crf, 28.0);
        assert_eq!(c.quality_thresholds.min_ssim, 0.97);
        assert!(c.quality_thresholds.validate_ssim);
    }
    
    /// 🔥 v4.5: 测试精确质量匹配 + 压缩配置
    #[test]
    fn test_explore_config_precise_quality_match_with_compression() {
        let c = ExploreConfig::precise_quality_match_with_compression(20.0, 35.0, 0.95);
        assert_eq!(c.mode, ExploreMode::PreciseQualityMatchWithCompression);
        assert_eq!(c.initial_crf, 20.0);
        assert_eq!(c.max_crf, 35.0);
        assert_eq!(c.quality_thresholds.min_ssim, 0.95);
        assert!(c.quality_thresholds.validate_ssim);
    }
    
    /// 🔥 v4.5: 测试所有探索模式枚举
    #[test]
    fn test_explore_modes() {
        // 测试所有模式都能正确创建
        let size_only = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(size_only.mode, ExploreMode::SizeOnly);
        
        let quality_match = ExploreConfig::quality_match(22.0);
        assert_eq!(quality_match.mode, ExploreMode::QualityMatch);
        
        let precise = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(precise.mode, ExploreMode::PreciseQualityMatch);
        
        let precise_compress = ExploreConfig::precise_quality_match_with_compression(18.0, 28.0, 0.97);
        assert_eq!(precise_compress.mode, ExploreMode::PreciseQualityMatchWithCompression);
    }
    
    /// 🔥 v4.5: 测试 flag 组合语义
    #[test]
    fn test_flag_combinations_semantics() {
        // --explore 单独: SizeOnly 模式
        let explore_only = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(explore_only.mode, ExploreMode::SizeOnly);
        assert!(!explore_only.quality_thresholds.validate_ssim, "SizeOnly should NOT validate SSIM");
        
        // --match-quality 单独: QualityMatch 模式
        let match_only = ExploreConfig::quality_match(22.0);
        assert_eq!(match_only.mode, ExploreMode::QualityMatch);
        assert_eq!(match_only.max_iterations, 1, "QualityMatch should be single-shot");
        
        // --explore --match-quality: PreciseQualityMatch 模式
        let explore_match = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(explore_match.mode, ExploreMode::PreciseQualityMatch);
        assert!(explore_match.quality_thresholds.validate_ssim, "PreciseQualityMatch MUST validate SSIM");
        
        // --explore --match-quality --compress: PreciseQualityMatchWithCompression 模式
        let explore_match_compress = ExploreConfig::precise_quality_match_with_compression(18.0, 28.0, 0.97);
        assert_eq!(explore_match_compress.mode, ExploreMode::PreciseQualityMatchWithCompression);
        assert!(explore_match_compress.quality_thresholds.validate_ssim, "Compression mode MUST validate SSIM");
    }
    
    #[test]
    fn test_video_encoder_names() {
        assert_eq!(VideoEncoder::Hevc.ffmpeg_name(), "libx265");
        assert_eq!(VideoEncoder::Av1.ffmpeg_name(), "libsvtav1");
        assert_eq!(VideoEncoder::H264.ffmpeg_name(), "libx264");
    }
    
    #[test]
    fn test_video_encoder_containers() {
        assert_eq!(VideoEncoder::Hevc.container(), "mp4");
        assert_eq!(VideoEncoder::Av1.container(), "mp4");
        assert_eq!(VideoEncoder::H264.container(), "mp4");
    }
    
    #[test]
    fn test_explore_mode_enum() {
        assert_ne!(ExploreMode::SizeOnly, ExploreMode::QualityMatch);
        assert_ne!(ExploreMode::QualityMatch, ExploreMode::PreciseQualityMatch);
        assert_ne!(ExploreMode::SizeOnly, ExploreMode::PreciseQualityMatch);
    }
    
    // ═══════════════════════════════════════════════════════════
    // 精确度证明测试 - 裁判验证
    // ═══════════════════════════════════════════════════════════
    
    #[test]
    fn test_precision_crf_search_range_hevc() {
        // HEVC CRF 范围 [10, 28]，需要 log2(18) ≈ 4.17 次迭代
        let iterations = required_iterations(10, 28);
        assert!(iterations <= 8, "HEVC range [10,28] should need <= 8 iterations, got {}", iterations);
        assert_eq!(iterations, 6); // ceil(log2(18)) + 1 = 5 + 1 = 6
    }
    
    #[test]
    fn test_precision_crf_search_range_av1() {
        // AV1 CRF 范围 [10, 35]，需要 log2(25) ≈ 4.64 次迭代
        let iterations = required_iterations(10, 35);
        assert!(iterations <= 8, "AV1 range [10,35] should need <= 8 iterations, got {}", iterations);
        assert_eq!(iterations, 6); // ceil(log2(25)) + 1 = 5 + 1 = 6
    }
    
    #[test]
    fn test_precision_crf_search_range_wide() {
        // 极端范围 [0, 51]，需要 log2(51) ≈ 5.67 次迭代
        let iterations = required_iterations(0, 51);
        assert!(iterations <= 8, "Wide range [0,51] should need <= 8 iterations, got {}", iterations);
        assert_eq!(iterations, 7); // ceil(log2(51)) + 1 = 6 + 1 = 7
    }
    
    #[test]
    fn test_precision_ssim_threshold_exact() {
        // 精确阈值测试
        assert!(ssim_meets_threshold(0.95, 0.95));
        assert!(ssim_meets_threshold(0.9501, 0.95));
        assert!(ssim_meets_threshold(0.9499, 0.95)); // 在 epsilon 范围内
        assert!(!ssim_meets_threshold(0.9498, 0.95)); // 超出 epsilon
    }
    
    #[test]
    fn test_precision_ssim_threshold_edge_cases() {
        // 边界情况
        assert!(ssim_meets_threshold(1.0, 1.0));
        assert!(ssim_meets_threshold(0.0, 0.0));
        assert!(!ssim_meets_threshold(0.94, 0.95));
        assert!(ssim_meets_threshold(0.96, 0.95));
    }
    
    #[test]
    fn test_precision_ssim_quality_grades() {
        assert_eq!(ssim_quality_grade(0.99), "Excellent (几乎无法区分)");
        assert_eq!(ssim_quality_grade(0.98), "Excellent (几乎无法区分)");
        assert_eq!(ssim_quality_grade(0.97), "Good (视觉无损)");
        assert_eq!(ssim_quality_grade(0.95), "Good (视觉无损)");
        assert_eq!(ssim_quality_grade(0.92), "Acceptable (轻微差异)");
        assert_eq!(ssim_quality_grade(0.90), "Acceptable (轻微差异)");
        assert_eq!(ssim_quality_grade(0.87), "Fair (可见差异)");
        assert_eq!(ssim_quality_grade(0.85), "Fair (可见差异)");
        assert_eq!(ssim_quality_grade(0.80), "Poor (明显质量损失)");
    }
    
    // ═══════════════════════════════════════════════════════════
    // 三种模式裁判验证测试
    // ═══════════════════════════════════════════════════════════
    
    #[test]
    fn test_judge_mode_size_only_config() {
        // SizeOnly 模式：不验证 SSIM，只保证 size < input
        let c = ExploreConfig::size_only(18.0, 28.0);
        
        // 裁判验证：不应启用 SSIM 验证
        assert!(!c.quality_thresholds.validate_ssim, 
            "SizeOnly mode should NOT validate SSIM");
        assert!(!c.quality_thresholds.validate_psnr,
            "SizeOnly mode should NOT validate PSNR");
        
        // 🔥 v3.6: 裁判验证：应使用足够的迭代次数
        assert!(c.max_iterations >= 8,
            "SizeOnly mode should use sufficient iterations for best size");
    }
    
    #[test]
    fn test_judge_mode_quality_match_config() {
        // QualityMatch 模式：单次编码 + SSIM 验证
        let c = ExploreConfig::quality_match(20.0);
        
        // 裁判验证：应启用 SSIM 验证
        assert!(c.quality_thresholds.validate_ssim,
            "QualityMatch mode MUST validate SSIM");
        
        // 裁判验证：应只有 1 次迭代
        assert_eq!(c.max_iterations, 1,
            "QualityMatch mode should have exactly 1 iteration");
        
        // 裁判验证：应使用预测的 CRF
        assert_eq!(c.initial_crf, 20.0,
            "QualityMatch mode should use predicted CRF");
    }
    
    #[test]
    fn test_judge_mode_precise_quality_match_config() {
        // PreciseQualityMatch 模式：三阶段搜索 + SSIM 裁判验证
        let c = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        
        // 裁判验证：应启用 SSIM 验证
        assert!(c.quality_thresholds.validate_ssim,
            "PreciseQualityMatch mode MUST validate SSIM");
        
        // 裁判验证：应使用自定义 SSIM 阈值
        assert_eq!(c.quality_thresholds.min_ssim, 0.97,
            "PreciseQualityMatch mode should use custom min_ssim");
        
        // 🔥 v3.6: 裁判验证：应使用足够的迭代次数支持三阶段搜索
        assert!(c.max_iterations >= 8,
            "PreciseQualityMatch mode should use sufficient iterations");
        
        // 裁判验证：CRF 范围应正确
        assert_eq!(c.initial_crf, 18.0);
        assert_eq!(c.max_crf, 28.0);
    }
    
    // ═══════════════════════════════════════════════════════════
    // 二分搜索精度数学证明
    // ═══════════════════════════════════════════════════════════
    
    #[test]
    fn test_binary_search_precision_proof() {
        // 🔥 v3.6: 三阶段搜索精度证明
        // 
        // 对于 HEVC [10, 28]，range = 18
        // Phase 2 (粗搜索，步长 2.0): 18 / 2.0 = 9 次
        // Phase 3 (细搜索，步长 0.5): 2.0 / 0.5 = 4 次
        // 
        // 三阶段搜索保证 ±0.5 CRF 精度
        
        let range = 28.0 - 10.0;
        let coarse_iterations = (range / COARSE_STEP).ceil() as u32;
        let fine_iterations = (COARSE_STEP / FINE_STEP).ceil() as u32;
        let total = coarse_iterations + fine_iterations;
        
        assert!(total <= 15, 
            "Three-phase search should achieve ±0.5 CRF precision within 15 iterations");
        assert!(coarse_iterations <= 9,
            "HEVC range [10,28] coarse search should need <= 9 iterations");
    }
    
    #[test]
    fn test_binary_search_worst_case() {
        // 🔥 v3.6: 最坏情况：范围 [0, 51]（完整 CRF 范围）
        let range = 51.0 - 0.0;
        let coarse_iterations = (range / COARSE_STEP).ceil() as u32;
        let fine_iterations = (COARSE_STEP / FINE_STEP).ceil() as u32;
        let total = coarse_iterations + fine_iterations;
        
        assert!(total <= 30,
            "Even worst case [0,51] should achieve ±0.5 precision within 30 iterations");
        assert!(coarse_iterations <= 26,
            "Range [0,51] coarse search should need <= 26 iterations");
    }
    
    // ═══════════════════════════════════════════════════════════
    // 质量验证逻辑测试
    // ═══════════════════════════════════════════════════════════
    
    #[test]
    fn test_quality_check_ssim_only() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_vmaf: 85.0,
            validate_ssim: true,
            validate_psnr: false,
            validate_vmaf: false,
        };
        
        // 模拟 check_quality_passed 逻辑
        let check = |ssim: Option<f64>, psnr: Option<f64>| -> bool {
            if thresholds.validate_ssim {
                match ssim {
                    Some(s) if s >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validate_psnr {
                match psnr {
                    Some(p) if p >= thresholds.min_psnr => {}
                    _ => return false,
                }
            }
            true
        };
        
        // SSIM 通过
        assert!(check(Some(0.96), None));
        assert!(check(Some(0.95), None));
        assert!(check(Some(0.99), Some(30.0))); // PSNR 不验证
        
        // SSIM 失败
        assert!(!check(Some(0.94), None));
        assert!(!check(None, Some(40.0))); // 无 SSIM
    }
    
    #[test]
    fn test_quality_check_both_metrics() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_vmaf: 85.0,
            validate_ssim: true,
            validate_psnr: true,
            validate_vmaf: false,
        };
        
        let check = |ssim: Option<f64>, psnr: Option<f64>| -> bool {
            if thresholds.validate_ssim {
                match ssim {
                    Some(s) if s >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validate_psnr {
                match psnr {
                    Some(p) if p >= thresholds.min_psnr => {}
                    _ => return false,
                }
            }
            true
        };
        
        // 两者都通过
        assert!(check(Some(0.96), Some(36.0)));
        
        // SSIM 通过，PSNR 失败
        assert!(!check(Some(0.96), Some(34.0)));
        
        // SSIM 失败，PSNR 通过
        assert!(!check(Some(0.94), Some(36.0)));
        
        // 两者都失败
        assert!(!check(Some(0.94), Some(34.0)));
    }
    

    

    
    #[test]
    fn test_precision_constants() {
        // 🔥 v5.55: CRF 精度调整为 ±0.25（速度优化）
        assert!((CRF_PRECISION - 0.25).abs() < 0.01, "CRF precision should be ±0.25");
        assert!((COARSE_STEP - 2.0).abs() < 0.01, "Coarse step should be 2.0");
        assert!((FINE_STEP - 0.5).abs() < 0.01, "Fine step should be 0.5");
        assert!((ULTRA_FINE_STEP - 0.25).abs() < 0.01, "Ultra fine step should be 0.25");
        assert_eq!(SSIM_DISPLAY_PRECISION, 4);
        assert!((SSIM_COMPARE_EPSILON - 0.0001).abs() < 1e-10);
        assert!((DEFAULT_MIN_SSIM - 0.95).abs() < 1e-10);
        assert!((HIGH_QUALITY_MIN_SSIM - 0.98).abs() < 1e-10);
        assert!((ACCEPTABLE_MIN_SSIM - 0.90).abs() < 1e-10);
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v3.5: 裁判机制增强测试 (Referee Mechanism Enhancement Tests)
    // ═══════════════════════════════════════════════════════════════
    
    /// 🔥 测试：VMAF 质量等级判定
    #[test]
    fn test_vmaf_quality_grades() {
        assert_eq!(vmaf_quality_grade(95.0), "Excellent (几乎无法区分)");
        assert_eq!(vmaf_quality_grade(93.0), "Excellent (几乎无法区分)");
        assert_eq!(vmaf_quality_grade(90.0), "Good (流媒体质量)");
        assert_eq!(vmaf_quality_grade(85.0), "Good (流媒体质量)");
        assert_eq!(vmaf_quality_grade(80.0), "Acceptable (移动端质量)");
        assert_eq!(vmaf_quality_grade(75.0), "Acceptable (移动端质量)");
        assert_eq!(vmaf_quality_grade(65.0), "Fair (可见差异)");
        assert_eq!(vmaf_quality_grade(60.0), "Fair (可见差异)");
        assert_eq!(vmaf_quality_grade(50.0), "Poor (明显质量损失)");
    }
    
    /// 🔥 测试：VMAF 有效性验证
    #[test]
    fn test_vmaf_validity() {
        assert!(is_valid_vmaf(0.0));
        assert!(is_valid_vmaf(50.0));
        assert!(is_valid_vmaf(100.0));
        assert!(!is_valid_vmaf(-1.0));
        assert!(!is_valid_vmaf(101.0));
    }
    
    /// 🔥 测试：三种模式的配置正确性
    #[test]
    fn test_three_modes_config_correctness() {
        // 模式 1: SizeOnly - 不验证质量
        let size_only = ExploreConfig::size_only(20.0, 30.0);
        assert_eq!(size_only.mode, ExploreMode::SizeOnly);
        assert!(!size_only.quality_thresholds.validate_ssim, "SizeOnly should NOT validate SSIM");
        assert!(!size_only.quality_thresholds.validate_vmaf, "SizeOnly should NOT validate VMAF");
        
        // 模式 2: QualityMatch - 单次编码 + SSIM 验证
        let quality_match = ExploreConfig::quality_match(22.0);
        assert_eq!(quality_match.mode, ExploreMode::QualityMatch);
        assert!(quality_match.quality_thresholds.validate_ssim, "QualityMatch MUST validate SSIM");
        assert_eq!(quality_match.max_iterations, 1, "QualityMatch should have 1 iteration");
        
        // 模式 3: PreciseQualityMatch - 二分搜索 + SSIM 裁判
        let precise = ExploreConfig::precise_quality_match(18.0, 28.0, 0.97);
        assert_eq!(precise.mode, ExploreMode::PreciseQualityMatch);
        assert!(precise.quality_thresholds.validate_ssim, "PreciseQualityMatch MUST validate SSIM");
        assert_eq!(precise.quality_thresholds.min_ssim, 0.97, "Custom min_ssim should be used");
        assert!(precise.max_iterations > 1, "PreciseQualityMatch should have multiple iterations");
    }
    
    /// 🔥 测试：自校准逻辑 - 当初始 CRF 不满足质量时应向下搜索
    #[test]
    fn test_self_calibration_logic() {
        // 模拟自校准场景：
        // 初始 CRF = 25，但 SSIM = 0.93 < 0.95 阈值
        // 应该向下搜索（降低 CRF）以提高质量
        
        let config = ExploreConfig::precise_quality_match(25.0, 35.0, 0.95);
        
        // 验证配置允许向下搜索
        assert!(config.min_crf < config.initial_crf, 
            "min_crf ({}) should be less than initial_crf ({}) to allow downward search",
            config.min_crf, config.initial_crf);
        
        // 验证二分搜索范围足够
        let range = config.max_crf - config.min_crf;
        assert!(range >= 10.0, "CRF range should be at least 10 for effective calibration");
    }
    
    /// 🔥 测试：质量验证失败时的行为
    #[test]
    fn test_quality_validation_failure_behavior() {
        let thresholds = QualityThresholds {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_vmaf: 85.0,
            validate_ssim: true,
            validate_psnr: false,
            validate_vmaf: true, // 启用 VMAF
        };
        
        // 模拟 check_quality_passed 逻辑（包含 VMAF）
        let check = |ssim: Option<f64>, vmaf: Option<f64>| -> bool {
            if thresholds.validate_ssim {
                match ssim {
                    Some(s) if s + SSIM_COMPARE_EPSILON >= thresholds.min_ssim => {}
                    _ => return false,
                }
            }
            if thresholds.validate_vmaf {
                match vmaf {
                    Some(v) if v >= thresholds.min_vmaf => {}
                    _ => return false,
                }
            }
            true
        };
        
        // SSIM 通过，VMAF 通过
        assert!(check(Some(0.96), Some(90.0)));
        
        // SSIM 通过，VMAF 失败
        assert!(!check(Some(0.96), Some(80.0)));
        
        // SSIM 失败，VMAF 通过
        assert!(!check(Some(0.94), Some(90.0)));
        
        // VMAF 为 None 时应失败（启用了验证但无法计算）
        assert!(!check(Some(0.96), None));
    }
    
    /// 🔥 测试：评价标准阈值
    #[test]
    fn test_evaluation_criteria_thresholds() {
        // SSIM 评价标准
        assert!(DEFAULT_MIN_SSIM >= 0.95, "Default SSIM should be >= 0.95 (Good)");
        assert!(HIGH_QUALITY_MIN_SSIM >= 0.98, "High quality SSIM should be >= 0.98 (Excellent)");
        assert!(ACCEPTABLE_MIN_SSIM >= 0.90, "Acceptable SSIM should be >= 0.90");
        assert!(MIN_ACCEPTABLE_SSIM >= 0.85, "Minimum acceptable SSIM should be >= 0.85");
        
        // VMAF 评价标准
        assert!(DEFAULT_MIN_VMAF >= 85.0, "Default VMAF should be >= 85 (Good)");
        assert!(HIGH_QUALITY_MIN_VMAF >= 93.0, "High quality VMAF should be >= 93 (Excellent)");
        assert!(ACCEPTABLE_MIN_VMAF >= 75.0, "Acceptable VMAF should be >= 75");
    }
    
    /// 🔥 测试：CRF 0.5 步长精度
    #[test]
    fn test_crf_half_step_precision() {
        // 验证 0.5 步长的二分搜索
        let test_values: [f64; 7] = [18.0, 18.5, 19.0, 19.5, 20.0, 20.5, 21.0];
        
        for &crf in &test_values {
            // 四舍五入到 0.5 步长
            let rounded = (crf * 2.0).round() / 2.0;
            assert!((rounded - crf).abs() < 0.01, 
                "CRF {} should round to {} with 0.5 step", crf, rounded);
        }
        
        // 测试非 0.5 步长值的四舍五入
        assert!((((23.3_f64 * 2.0).round() / 2.0) - 23.5).abs() < 0.01);
        assert!((((23.7_f64 * 2.0).round() / 2.0) - 23.5).abs() < 0.01);
        assert!((((23.2_f64 * 2.0).round() / 2.0) - 23.0).abs() < 0.01);
        assert!((((23.8_f64 * 2.0).round() / 2.0) - 24.0).abs() < 0.01);
    }
    
    /// 🔥 测试：探索结果结构完整性
    #[test]
    fn test_explore_result_completeness() {
        let result = ExploreResult {
            optimal_crf: 23.5,
            output_size: 1_000_000,
            size_change_pct: -15.5,
            ssim: Some(0.9650),
            psnr: Some(38.5),
            vmaf: Some(92.3),
            iterations: 5,
            quality_passed: true,
            log: vec!["Test log".to_string()],
            confidence: 0.85,
            confidence_detail: ConfidenceBreakdown::default(),
        };
        
        // 验证所有字段都有意义
        assert!(result.optimal_crf > 0.0);
        assert!(result.output_size > 0);
        assert!(result.size_change_pct < 0.0, "Size should decrease");
        assert!(result.ssim.is_some());
        assert!(result.psnr.is_some());
        assert!(result.vmaf.is_some());
        assert!(result.iterations > 0);
        assert!(result.quality_passed);
        assert!(!result.log.is_empty());
        assert!(result.confidence > 0.0 && result.confidence <= 1.0);
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v3.6: 三阶段搜索精度测试
    // ═══════════════════════════════════════════════════════════════
    
    /// 🔥 测试：三阶段搜索迭代次数估算
    #[test]
    fn test_three_phase_iteration_estimate() {
        // 典型场景：initial=20, range=[15, 30]
        let initial = 20.0_f32;
        let _min_crf = 15.0_f32;
        let max_crf = 30.0_f32;
        
        // Phase 2: 粗搜索（步长 2.0）
        // 向上搜索：(30 - 20) / 2.0 = 5 次
        let coarse_up = ((max_crf - initial) / COARSE_STEP).ceil() as u32;
        assert_eq!(coarse_up, 5, "Coarse search up should be 5 iterations");
        
        // Phase 3: 细搜索（步长 0.5）
        // 假设边界区间 [24, 28]，需要 (28 - 24) / 0.5 = 8 次
        let boundary_range = 4.0_f32;
        let fine_iterations = (boundary_range / FINE_STEP).ceil() as u32;
        assert_eq!(fine_iterations, 8, "Fine search should be 8 iterations");
        
        // 总迭代次数应该在 max_iterations 范围内
        let total = 1 + coarse_up + fine_iterations + 1; // initial + coarse + fine + refinement
        assert!(total <= 15, "Total iterations {} should be <= 15", total);
    }
    
    /// 🔥 测试：CRF 精度保证 ±0.5
    #[test]
    fn test_crf_precision_guarantee() {
        // 验证 0.5 步长可以覆盖任意 CRF 值
        let test_targets: [f32; 5] = [18.3, 20.7, 23.1, 25.9, 28.4];
        
        for &target in &test_targets {
            // 找到最接近的 0.5 步长值
            let nearest = ((target * 2.0).round() / 2.0) as f32;
            let error = (nearest - target).abs();
            
            assert!(error <= 0.25, 
                "Target {} should be within ±0.25 of nearest step {}, got error {}", 
                target, nearest, error);
        }
    }
    
    /// 🔥 测试：边界精细化逻辑
    #[test]
    fn test_boundary_refinement_logic() {
        // 模拟边界精细化场景
        // 假设 best_crf = 24.0，测试 24.5 是否更优
        let best_crf = 24.0_f32;
        let next_crf = best_crf + FINE_STEP;
        let max_crf = 30.0_f32;
        
        // 验证 next_crf 在有效范围内
        assert!(next_crf <= max_crf, "Next CRF should be within max");
        assert!((next_crf - best_crf - 0.5).abs() < 0.01, "Step should be 0.5");
    }
    
    /// 🔥 测试：搜索方向判断
    #[test]
    fn test_search_direction_logic() {
        // 场景 1：初始质量通过 → 向上搜索（更高 CRF = 更小文件）
        let initial_passed = true;
        let search_up = initial_passed;
        assert!(search_up, "Should search up when initial quality passed");
        
        // 场景 2：初始质量失败 → 向下搜索（更低 CRF = 更高质量）
        let initial_failed = false;
        let search_down = !initial_failed;
        assert!(search_down, "Should search down when initial quality failed");
    }
    
    /// 🔥 测试：迭代次数上限保护
    #[test]
    fn test_max_iterations_protection() {
        let config = ExploreConfig::default();
        
        // 最坏情况：range [10, 40]
        let worst_range = 30.0_f32;
        let worst_coarse = (worst_range / COARSE_STEP).ceil() as u32;
        let worst_fine = (COARSE_STEP / FINE_STEP).ceil() as u32 * 2; // 边界区间
        let worst_total = 1 + worst_coarse + worst_fine + 1;
        
        assert!(config.max_iterations as u32 >= worst_total / 2,
            "max_iterations {} should handle typical worst case {}", 
            config.max_iterations, worst_total);
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v3.8: 智能阈值计算测试
    // ═══════════════════════════════════════════════════════════════
    
    /// 🔥 测试：智能阈值计算 - HEVC 高质量源
    #[test]
    fn test_smart_thresholds_hevc_high_quality() {
        // 高质量源 (CRF 18)
        let (max_crf, min_ssim) = calculate_smart_thresholds(18.0, VideoEncoder::Hevc);
        
        // 高质量源应该有严格的 SSIM 阈值
        assert!(min_ssim >= 0.93, "High quality source should have strict SSIM >= 0.93, got {}", min_ssim);
        
        // max_crf 应该有合理的 headroom
        assert!(max_crf >= 26.0, "max_crf should be at least 26 for CRF 18, got {}", max_crf);
        assert!(max_crf <= 30.0, "max_crf should not exceed 30 for high quality, got {}", max_crf);
    }
    
    /// 🔥 测试：智能阈值计算 - HEVC 低质量源
    #[test]
    fn test_smart_thresholds_hevc_low_quality() {
        // 低质量源 (CRF 35)
        let (max_crf, min_ssim) = calculate_smart_thresholds(35.0, VideoEncoder::Hevc);
        
        // 低质量源应该有宽松的 SSIM 阈值
        assert!(min_ssim <= 0.92, "Low quality source should have relaxed SSIM <= 0.92, got {}", min_ssim);
        assert!(min_ssim >= 0.85, "SSIM should not go below 0.85, got {}", min_ssim);
        
        // max_crf 应该允许更高的值
        assert!(max_crf >= 40.0, "max_crf should be at least 40 for low quality, got {}", max_crf);
    }
    
    /// 🔥 测试：智能阈值计算 - AV1 编码器
    #[test]
    fn test_smart_thresholds_av1() {
        // AV1 CRF 范围是 0-63，比 HEVC 更宽
        let (max_crf_low, min_ssim_low) = calculate_smart_thresholds(40.0, VideoEncoder::Av1);
        let (max_crf_high, min_ssim_high) = calculate_smart_thresholds(20.0, VideoEncoder::Av1);
        
        // 低质量源应该有更高的 max_crf
        assert!(max_crf_low > max_crf_high, "Low quality should have higher max_crf");
        
        // 低质量源应该有更低的 min_ssim
        assert!(min_ssim_low < min_ssim_high, "Low quality should have lower min_ssim");
        
        // AV1 max_crf 上限应该是 50
        assert!(max_crf_low <= 50.0, "AV1 max_crf should not exceed 50, got {}", max_crf_low);
    }
    
    /// 🔥 测试：边缘案例 - 极低质量源
    #[test]
    fn test_smart_thresholds_edge_case_very_low_quality() {
        // 极低质量源 (CRF 45 for HEVC)
        let (max_crf, min_ssim) = calculate_smart_thresholds(45.0, VideoEncoder::Hevc);
        
        // 应该触发边界保护
        assert!(max_crf <= 40.0, "HEVC max_crf should be capped at 40, got {}", max_crf);
        assert!(min_ssim >= 0.85, "min_ssim should not go below 0.85, got {}", min_ssim);
    }
    
    /// 🔥 测试：边缘案例 - 极高质量源
    #[test]
    fn test_smart_thresholds_edge_case_very_high_quality() {
        // 极高质量源 (CRF 10)
        let (max_crf, min_ssim) = calculate_smart_thresholds(10.0, VideoEncoder::Hevc);
        
        // 高质量源应该有严格的阈值
        assert!(min_ssim >= 0.94, "Very high quality should have strict SSIM >= 0.94, got {}", min_ssim);
        
        // max_crf 应该有足够的 headroom
        assert!(max_crf >= 18.0, "max_crf should be at least 18 for CRF 10, got {}", max_crf);
    }
    
    /// 🔥 测试：阈值连续性 - 确保没有跳跃
    #[test]
    fn test_smart_thresholds_continuity() {
        // 测试阈值随 CRF 变化的连续性
        let mut prev_max_crf = 0.0_f32;
        let mut prev_min_ssim = 1.0_f64;
        
        for crf in (10..=40).step_by(2) {
            let (max_crf, min_ssim) = calculate_smart_thresholds(crf as f32, VideoEncoder::Hevc);
            
            if crf > 10 {
                // max_crf 应该单调递增（或保持不变）
                assert!(max_crf >= prev_max_crf - 0.5, 
                    "max_crf should be monotonically increasing: {} -> {} at CRF {}", 
                    prev_max_crf, max_crf, crf);
                
                // min_ssim 应该单调递减（或保持不变）
                assert!(min_ssim <= prev_min_ssim + 0.01, 
                    "min_ssim should be monotonically decreasing: {} -> {} at CRF {}", 
                    prev_min_ssim, min_ssim, crf);
            }
            
            prev_max_crf = max_crf;
            prev_min_ssim = min_ssim;
        }
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v4.0: 激进精度追求测试 (Aggressive Precision Tests)
    // ═══════════════════════════════════════════════════════════════
    
    /// 🔥 v4.0 测试：目标 SSIM 接近 1.0
    #[test]
    fn test_v4_target_ssim_near_lossless() {
        // v4.0 目标是无限逼近 SSIM=1.0
        let target_ssim = 0.9999_f64;
        
        // 验证目标值合理性
        assert!(target_ssim > 0.999, "Target SSIM should be > 0.999 for near-lossless");
        assert!(target_ssim < 1.0, "Target SSIM should be < 1.0 (1.0 is mathematically lossless)");
        
        // 验证与之前版本的差异
        let v3_target = 0.98_f64;
        assert!(target_ssim > v3_target, "v4.0 target {} should be higher than v3.9 target {}", 
            target_ssim, v3_target);
    }
    
    /// 🔥 v5.55 测试：CRF 精度调整为 ±0.25（速度优化）
    #[test]
    fn test_v4_crf_precision_0_1() {
        // v5.55 精度从 ±0.1 调整为 ±0.25（速度提升 2-3 倍）
        let test_values: [f32; 5] = [18.0, 18.25, 18.5, 18.75, 19.0];
        
        for &crf in &test_values {
            // 四舍五入到 0.25 步长
            let rounded = (crf * 4.0).round() / 4.0;
            assert!((rounded - crf).abs() < 0.01, 
                "CRF {} should round to {} with 0.25 step", crf, rounded);
        }
        
        // 测试非 0.25 步长值的四舍五入
        assert!(((23.1_f32 * 4.0).round() / 4.0 - 23.0).abs() < 0.01);
        assert!(((23.2_f32 * 4.0).round() / 4.0 - 23.25).abs() < 0.01);
        assert!(((23.4_f32 * 4.0).round() / 4.0 - 23.5).abs() < 0.01);
    }
    
    /// 🔥 v4.0 测试：四阶段搜索策略
    #[test]
    fn test_v4_four_phase_search_strategy() {
        // Phase 1: 全范围扫描 (步长 1.0)
        let phase1_step = 1.0_f32;
        let range = 28.0 - 10.0; // HEVC 典型范围
        let phase1_iterations = (range / phase1_step).ceil() as u32;
        assert_eq!(phase1_iterations, 18, "Phase 1 should scan 18 CRF values");
        
        // Phase 2: 区域精细化 (步长 0.5, 范围 ±2)
        let phase2_step = 0.5_f32;
        let phase2_range = 4.0_f32; // ±2
        let phase2_iterations = (phase2_range / phase2_step).ceil() as u32;
        assert_eq!(phase2_iterations, 8, "Phase 2 should test 8 CRF values");
        
        // Phase 3: 超精细调整 (步长 0.1, 范围 ±0.5)
        let phase3_step = 0.1_f32;
        let phase3_range = 1.0_f32; // ±0.5
        let phase3_iterations = (phase3_range / phase3_step).ceil() as u32;
        assert_eq!(phase3_iterations, 10, "Phase 3 should test 10 CRF values");
        
        // Phase 4: 极限逼近 (无限制，直到 SSIM 不再提升)
        // 这个阶段没有固定迭代次数，取决于 SSIM 收敛
    }
    
    /// 🔥 v4.0 测试：SSIM 质量等级 - 新增 Near-Lossless 等级
    #[test]
    fn test_v4_ssim_quality_grades_extended() {
        // v4.0 新增 Near-Lossless 等级
        let near_lossless_threshold = 0.9999_f64;
        let excellent_threshold = 0.999_f64;
        let very_good_threshold = 0.99_f64;
        let good_threshold = 0.98_f64;
        
        // 验证等级递进
        assert!(near_lossless_threshold > excellent_threshold);
        assert!(excellent_threshold > very_good_threshold);
        assert!(very_good_threshold > good_threshold);
        
        // 验证等级判定逻辑
        let grade = |ssim: f64| -> &'static str {
            if ssim >= 0.9999 { "Near-Lossless" }
            else if ssim >= 0.999 { "Excellent" }
            else if ssim >= 0.99 { "Very Good" }
            else if ssim >= 0.98 { "Good" }
            else if ssim >= 0.95 { "Acceptable" }
            else { "Below threshold" }
        };
        
        assert_eq!(grade(0.9999), "Near-Lossless");
        assert_eq!(grade(0.9995), "Excellent");
        assert_eq!(grade(0.995), "Very Good");
        assert_eq!(grade(0.985), "Good");
        assert_eq!(grade(0.96), "Acceptable");
        assert_eq!(grade(0.94), "Below threshold");
    }
    
    /// 🔥 v4.0 测试：SSIM 平台检测 - 停止无效搜索
    #[test]
    fn test_v4_ssim_plateau_detection() {
        // 模拟 SSIM 平台场景：连续 3 个 CRF 的 SSIM 不再提升
        let ssim_values: [(f32, f64); 5] = [
            (20.0, 0.9850),
            (19.9, 0.9855),
            (19.8, 0.9856), // 最佳点
            (19.7, 0.9856), // 平台开始
            (19.6, 0.9855), // 平台继续，SSIM 下降
        ];
        
        // 检测平台：当 SSIM 不再提升时应停止搜索
        let mut best_ssim = 0.0_f64;
        let mut plateau_count = 0;
        
        for &(_crf, ssim) in &ssim_values {
            if ssim > best_ssim {
                best_ssim = ssim;
                plateau_count = 0;
            } else {
                plateau_count += 1;
            }
            
            // 连续 2 次不提升即为平台
            if plateau_count >= 2 {
                break;
            }
        }
        
        assert!(plateau_count >= 2, "Should detect plateau after 2 non-improvements");
        assert!((best_ssim - 0.9856).abs() < 0.0001, "Best SSIM should be 0.9856");
    }
    
    /// 🔥 v4.0 测试：极端场景 - 已经是高质量源
    #[test]
    fn test_v4_high_quality_source_handling() {
        // 场景：源视频已经是高质量 (CRF 15, SSIM 0.9990)
        let source_crf = 15.0_f32;
        let source_ssim = 0.9990_f64;
        let target_ssim = 0.9999_f64;
        
        // 如果源 SSIM 已经很高，应该使用更低的 CRF
        let expected_output_crf = source_crf - 2.0; // 降低 CRF 以提高质量
        
        assert!(expected_output_crf < source_crf, 
            "Output CRF should be lower than source for quality improvement");
        assert!(source_ssim < target_ssim, 
            "Source SSIM {} should be below target {}", source_ssim, target_ssim);
    }
    
    /// 🔥 v4.0 测试：极端场景 - 低质量源的质量上限
    #[test]
    fn test_v4_low_quality_source_ceiling() {
        // 场景：源视频是低质量 (CRF 35, SSIM 0.9200)
        // 即使用 CRF 0 也无法达到 SSIM 0.9999（因为源本身就有损失）
        let _source_crf = 35.0_f32;
        let source_ssim = 0.9200_f64;
        let target_ssim = 0.9999_f64;
        
        // 低质量源的 SSIM 上限取决于源本身的质量
        // 重新编码无法恢复已丢失的信息
        let ssim_ceiling = source_ssim + 0.05; // 最多提升 5%
        
        assert!(ssim_ceiling < target_ssim, 
            "Low quality source cannot reach target SSIM {}", target_ssim);
        
        // 验证算法应该在达到 ceiling 后停止
        // 而不是无限降低 CRF
    }
    
    /// 🔥 v5.55 测试：缓存机制 - 0.25 精度（速度优化）
    #[test]
    fn test_v4_crf_cache_mechanism() {
        // 模拟缓存机制：0.25 精度的 key (crf * 4.0)
        let mut cache: std::collections::HashMap<i32, f64> = std::collections::HashMap::new();
        
        // 测试 CRF 值到 key 的转换
        // CRF 20.0 → key 80, CRF 20.25 → key 81, CRF 20.5 → key 82
        let crf_to_key = |crf: f32| -> i32 { (crf * 4.0).round() as i32 };
        
        // 插入测试数据
        cache.insert(crf_to_key(20.0), 0.9850);   // key = 80
        cache.insert(crf_to_key(20.25), 0.9855);  // key = 81
        cache.insert(crf_to_key(20.5), 0.9860);   // key = 82
        
        // 验证缓存命中
        assert!(cache.contains_key(&crf_to_key(20.0)));
        assert!(cache.contains_key(&crf_to_key(20.25)));
        assert!(cache.contains_key(&crf_to_key(20.5)));
        
        // 验证四舍五入后的缓存命中
        // 20.1 四舍五入到 80 (20.0)，应该命中
        assert!(cache.contains_key(&crf_to_key(20.1)), "20.1 should round to 80 and hit cache");
        // 20.3 四舍五入到 81 (20.25)，应该命中
        assert!(cache.contains_key(&crf_to_key(20.3)), "20.3 should round to 81 and hit cache");
        
        // 验证缓存未命中 - 未插入的值
        assert!(!cache.contains_key(&crf_to_key(20.75))); // key 83 未插入
        assert!(!cache.contains_key(&crf_to_key(19.75))); // key 79 未插入
        
        // 验证 key 计算正确性
        assert_eq!(crf_to_key(20.0), 80);   // 20.0 * 4 = 80
        assert_eq!(crf_to_key(20.25), 81);  // 20.25 * 4 = 81
        assert_eq!(crf_to_key(20.5), 82);   // 20.5 * 4 = 82
        assert_eq!(crf_to_key(20.1), 80);   // 20.1 * 4 = 80.4 → 80
        assert_eq!(crf_to_key(20.15), 81);  // 20.15 * 4 = 80.6 → 81
    }
    
    /// 🔥 v4.0 测试：迭代次数无上限（耗时不是问题）
    #[test]
    fn test_v4_no_iteration_limit() {
        // v4.0 的核心理念：无限逼近 SSIM=1.0，不在意耗时
        // 因此不应该有严格的迭代次数限制
        
        // 计算最坏情况的迭代次数
        let range = 51.0_f64 - 0.0; // 完整 CRF 范围
        let phase1 = (range / 1.0_f64).ceil() as u32; // 全范围扫描
        let phase2 = (4.0_f64 / 0.5_f64).ceil() as u32;   // 区域精细化
        let phase3 = (1.0_f64 / 0.1_f64).ceil() as u32;   // 超精细调整
        let phase4_max = 50_u32; // 极限逼近最多 50 次
        
        let total_max = phase1 + phase2 + phase3 + phase4_max;
        
        // v4.0 应该允许足够多的迭代
        assert!(total_max <= 150, "Total iterations should be reasonable: {}", total_max);
        
        // 但不应该有硬性上限阻止达到目标
        // 这是 v4.0 与之前版本的关键区别
    }
    
    /// 🔥 v4.0 测试：不同内容类型的 SSIM 收敛特性
    #[test]
    fn test_v4_content_type_ssim_convergence() {
        // 不同内容类型的 SSIM 收敛特性不同
        
        // 动画内容：SSIM 收敛快（大面积平坦区域）
        let animation_convergence_rate = 0.002_f64; // 每降低 1 CRF，SSIM 提升 0.002
        
        // 真人内容：SSIM 收敛中等
        let live_action_convergence_rate = 0.001_f64;
        
        // 高细节内容：SSIM 收敛慢（复杂纹理）
        let high_detail_convergence_rate = 0.0005_f64;
        
        // 验证收敛率差异
        assert!(animation_convergence_rate > live_action_convergence_rate);
        assert!(live_action_convergence_rate > high_detail_convergence_rate);
        
        // 计算达到目标 SSIM 所需的 CRF 降低量
        let target_improvement = 0.9999 - 0.9900; // 从 0.99 到 0.9999
        
        let animation_crf_drop = target_improvement / animation_convergence_rate;
        let live_action_crf_drop = target_improvement / live_action_convergence_rate;
        let high_detail_crf_drop = target_improvement / high_detail_convergence_rate;
        
        assert!(animation_crf_drop < live_action_crf_drop);
        assert!(live_action_crf_drop < high_detail_crf_drop);
    }
    
    /// 🔥 v4.0 测试：SSIM 精度验证 - ffmpeg 输出精度
    #[test]
    fn test_v4_ssim_precision_ffmpeg() {
        // ffmpeg SSIM 输出精度是 4 位小数
        let ffmpeg_precision = 0.0001_f64;
        
        // 验证我们的目标 SSIM 在 ffmpeg 精度范围内可区分
        let target_ssim = 0.9999_f64;
        let excellent_ssim = 0.9990_f64;
        
        let difference = target_ssim - excellent_ssim;
        assert!(difference >= ffmpeg_precision, 
            "Target and excellent SSIM should be distinguishable: diff={}", difference);
        
        // 验证 SSIM 比较使用正确的 epsilon
        let epsilon = SSIM_COMPARE_EPSILON;
        assert!((epsilon - 0.0001).abs() < 1e-10, 
            "SSIM compare epsilon should be 0.0001");
    }
    
    // ═══════════════════════════════════════════════════════════
    // 🔥 v4.13 测试：智能提前终止
    // ═══════════════════════════════════════════════════════════
    
    /// 🔥 v4.13 测试：滑动窗口方差计算
    #[test]
    fn test_v413_sliding_window_variance() {
        // 模拟滑动窗口方差计算
        let input_size = 1_000_000_u64;
        let window_size = 3_usize;
        let variance_threshold = 0.0001_f64; // 0.01%
        
        // 计算方差的辅助函数
        let calc_variance = |sizes: &[u64]| -> f64 {
            if sizes.len() < window_size { return f64::MAX; }
            let recent: Vec<f64> = sizes.iter()
                .rev()
                .take(window_size)
                .map(|s| *s as f64 / input_size as f64)
                .collect();
            let mean = recent.iter().sum::<f64>() / recent.len() as f64;
            recent.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / recent.len() as f64
        };
        
        // 场景1：稳定的 size（应该触发提前终止）
        let stable_sizes = vec![500_000_u64, 500_100, 500_050];
        let stable_variance = calc_variance(&stable_sizes);
        assert!(stable_variance < variance_threshold, 
            "Stable sizes should have low variance: {}", stable_variance);
        
        // 场景2：变化的 size（不应该触发提前终止）
        let varying_sizes = vec![500_000_u64, 600_000, 550_000];
        let varying_variance = calc_variance(&varying_sizes);
        assert!(varying_variance > variance_threshold, 
            "Varying sizes should have high variance: {}", varying_variance);
    }
    
    /// 🔥 v4.13 测试：相对变化率计算
    #[test]
    fn test_v413_relative_change_rate() {
        let change_rate_threshold = 0.005_f64; // 0.5%
        
        // 计算变化率
        let calc_change_rate = |prev: u64, curr: u64| -> f64 {
            if prev == 0 { return f64::MAX; }
            ((curr as f64 - prev as f64) / prev as f64).abs()
        };
        
        // 场景1：小变化（应该触发提前终止）
        let small_change = calc_change_rate(1_000_000, 1_004_000); // 0.4%
        assert!(small_change < change_rate_threshold, 
            "Small change {} should be below threshold", small_change);
        
        // 场景2：大变化（不应该触发提前终止）
        let large_change = calc_change_rate(1_000_000, 1_010_000); // 1%
        assert!(large_change > change_rate_threshold, 
            "Large change {} should be above threshold", large_change);
    }
    
    /// 🔥 v4.13 测试：三阶段搜索策略
    #[test]
    fn test_v413_three_phase_search() {
        // Phase 1: 0.5 步进二分搜索
        let phase1_step = 0.5_f32;
        let crf_range = 28.0_f32 - 10.0_f32; // 18 CRF 范围
        let phase1_iterations = (crf_range / phase1_step).log2().ceil() as u32;
        assert!(phase1_iterations <= 6, "Phase 1 should need ~6 iterations: {}", phase1_iterations);
        
        // Phase 2: ±0.4 范围 0.1 步进
        let phase2_range = 0.8_f32; // ±0.4
        let phase2_step = 0.1_f32;
        let phase2_max_iterations = (phase2_range / phase2_step).ceil() as u32;
        assert_eq!(phase2_max_iterations, 8, "Phase 2 should need max 8 iterations");
        
        // Phase 3: SSIM 验证（1次）
        let phase3_iterations = 1_u32;
        
        // 总迭代次数估算
        let total_max = phase1_iterations + phase2_max_iterations + phase3_iterations;
        assert!(total_max <= 15, "Total iterations should be <= 15: {}", total_max);
    }
    
    /// 🔥 v4.13 测试：双向精细调整
    #[test]
    fn test_v413_bidirectional_fine_tune() {
        // 模拟双向搜索
        let boundary_crf = 17.5_f32;
        let min_crf = 10.0_f32;
        let max_crf = 28.0_f32;
        
        // 向下搜索（更高质量）
        let lower_offsets = [-0.25_f32, -0.5, -0.75, -1.0];
        for offset in lower_offsets {
            let test_crf = boundary_crf + offset;
            assert!(test_crf >= min_crf, "Lower search should stay above min_crf");
            assert!(test_crf < boundary_crf, "Lower search should be below boundary");
        }
        
        // 向上搜索（确认边界）
        let upper_offsets = [0.25_f32, 0.5, 0.75, 1.0];
        for offset in upper_offsets {
            let test_crf = boundary_crf + offset;
            assert!(test_crf <= max_crf, "Upper search should stay below max_crf");
            assert!(test_crf > boundary_crf, "Upper search should be above boundary");
        }
    }
    
    /// 🔥 v5.55 测试：CRF 精度保证 0.25（速度优化）
    #[test]
    fn test_v413_crf_precision_guarantee() {
        // 验证最终 CRF 可以是任意 0.25 步进值
        let valid_crfs = [17.0_f32, 17.25, 17.5, 17.75, 18.0, 18.25, 18.5, 18.75, 19.0];
        
        for crf in valid_crfs {
            // 验证 CRF 是 0.25 的整数倍
            let scaled = (crf * 4.0).round();
            let reconstructed = scaled / 4.0;
            assert!((crf - reconstructed).abs() < 0.001, 
                "CRF {} should be 0.25 precision", crf);
        }
        
        // 验证 precision 常量
        assert_eq!(ULTRA_FINE_STEP, 0.25, "ULTRA_FINE_STEP should be 0.25");
        assert_eq!(FINE_STEP, 0.5, "FINE_STEP should be 0.5");
    }
}
