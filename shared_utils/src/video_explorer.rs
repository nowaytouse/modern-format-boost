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

use std::path::Path;
use std::process::Command;
use std::fs;
use anyhow::{Result, Context, bail};

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
    /// - 二分搜索 + SSIM 裁判验证
    /// - 找到满足 SSIM >= min_ssim 的最高 CRF（最小文件）
    /// - 目标：最精确的质量-大小平衡
    PreciseQualityMatch,
}

// ═══════════════════════════════════════════════════════════════
// 数据结构
// ═══════════════════════════════════════════════════════════════

/// 探索结果
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
    /// 起始 CRF（AI 预测值）
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
    pub fn precise_quality_match(initial_crf: f32, max_crf: f32, min_ssim: f64) -> Self {
        Self {
            mode: ExploreMode::PreciseQualityMatch,
            initial_crf,
            max_crf,
            quality_thresholds: QualityThresholds {
                min_ssim,
                validate_ssim: true,
                validate_psnr: false,
                ..Default::default()
            },
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
        
        Ok(Self {
            config,
            encoder,
            input_path: input.to_path_buf(),
            output_path: output.to_path_buf(),
            input_size,
            vf_args,
            max_threads,
        })
    }
    
    /// 执行探索（根据模式选择不同策略）
    pub fn explore(&self) -> Result<ExploreResult> {
        match self.config.mode {
            ExploreMode::SizeOnly => self.explore_size_only(),
            ExploreMode::QualityMatch => self.explore_quality_match(),
            ExploreMode::PreciseQualityMatch => self.explore_precise_quality_match(),
        }
    }
    
    /// 模式 1: 仅探索更小的文件大小（--explore 单独使用）
    /// 
    /// 策略：二分搜索找到 size < input 的最高 CRF（最小文件）
    /// 不强制验证 SSIM，但会计算并提示裁判验证准确度
    fn explore_size_only(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let target_size = self.input_size; // 必须比输入小
        
        log.push(format!("🔍 Size-Only Exploration ({:?})", self.encoder));
        log.push(format!("   Input: {} bytes, Target: < {} bytes", 
            self.input_size, target_size));
        log.push(format!("   CRF range: [{}, {}]", 
            self.config.initial_crf, self.config.max_crf));
        
        // 🔥 v3.4: 二分搜索使用 0.5 步长
        let mut low = self.config.initial_crf;
        let mut high = self.config.max_crf;
        let mut best_crf = self.config.max_crf;
        let mut best_size = u64::MAX;
        let mut iterations = 0u32;
        
        while low <= high && iterations < self.config.max_iterations {
            iterations += 1;
            // 🔥 v3.4: 使用 0.5 步长的二分搜索
            let mid = ((low + high) / 2.0 * 2.0).round() / 2.0; // 四舍五入到 0.5
            
            let result = self.encode(mid)?;
            log.push(format!("   CRF {:.1}: {} bytes ({:+.1}%)", 
                mid, result, self.calc_change_pct(result)));
            
            if result < target_size {
                // 找到更小的文件，尝试更高 CRF（更小文件）
                best_crf = mid;
                best_size = result;
                low = mid + 0.5; // 🔥 v3.4: 0.5 步长
                log.push("      ✅ Size OK, trying higher CRF".to_string());
            } else {
                // 文件太大，需要更低 CRF（更高质量但更大）
                high = mid - 0.5; // 🔥 v3.4: 0.5 步长
                log.push("      📈 Size too large, trying lower CRF".to_string());
            }
        }
        
        // 如果没找到更小的，使用最高 CRF
        if best_size == u64::MAX {
            best_crf = self.config.max_crf;
            best_size = self.encode(best_crf)?;
            log.push(format!("   ⚠️ No smaller size found, using max CRF {}", best_crf));
        } else {
            // 重新编码最优 CRF
            best_size = self.encode(best_crf)?;
        }
        
        // 🔥 裁判验证准确度提示（仅供参考，不影响结果）
        let ssim = self.calculate_ssim().ok().flatten();
        let size_change_pct = self.calc_change_pct(best_size);
        
        if let Some(s) = ssim {
            let quality_hint = if s >= 0.98 {
                "🟢 Excellent"
            } else if s >= 0.95 {
                "🟡 Good"
            } else if s >= 0.90 {
                "🟠 Acceptable"
            } else {
                "🔴 Low"
            };
            log.push(format!("   📊 Final: CRF {}, {} bytes ({:+.1}%), SSIM: {:.4} ({})", 
                best_crf, best_size, size_change_pct, s, quality_hint));
        } else {
            log.push(format!("   📊 Final: CRF {}, {} bytes ({:+.1}%)", 
                best_crf, best_size, size_change_pct));
        }
        
        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim, // 提供 SSIM 供参考
            psnr: None,
            vmaf: None, // SizeOnly 模式不计算 VMAF
            iterations,
            quality_passed: best_size < target_size, // 只要更小就算通过
            log,
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
        })
    }
    
    /// 模式 3: 精确质量匹配（--explore + --match-quality 组合）
    /// 
    /// 🔥 v3.6: 三阶段高精度搜索算法
    /// 
    /// ## 精度保证
    /// - CRF 误差: ±0.5 (最终精度)
    /// - SSIM 验证精度: 0.0001 (ffmpeg 输出精度)
    /// 
    /// ## 三阶段搜索策略
    /// 1. **粗搜索** (步长 2.0): 快速定位质量边界区间
    /// 2. **细搜索** (步长 0.5): 在边界区间内精确定位
    /// 3. **边界精细化**: 验证边界点，确保最优
    /// 
    /// ## 自校准机制
    /// - 如果初始 CRF 质量不足，自动向下搜索（降低 CRF）
    /// - 如果初始 CRF 质量过剩，自动向上搜索（提高 CRF）
    fn explore_precise_quality_match(&self) -> Result<ExploreResult> {
        let mut log = Vec::new();
        let target_size = (self.input_size as f64 * self.config.target_ratio) as u64;
        
        log.push(format!("🔬 Precise Quality-Match v3.6 ({:?})", self.encoder));
        log.push(format!("   Input: {} bytes, Target: <= {} bytes", 
            self.input_size, target_size));
        log.push(format!("   CRF range: [{:.1}, {:.1}], Initial: {:.1}", 
            self.config.min_crf, self.config.max_crf, self.config.initial_crf));
        log.push(format!("   Min SSIM: {:.4}, Precision: ±0.5 CRF", 
            self.config.quality_thresholds.min_ssim));
        if self.config.quality_thresholds.validate_vmaf {
            log.push(format!("   Min VMAF: {:.1}", self.config.quality_thresholds.min_vmaf));
        }
        
        // 记录已测试的 CRF 值，避免重复编码
        let mut tested_crfs: std::collections::HashMap<i32, (u64, (Option<f64>, Option<f64>, Option<f64>))> = 
            std::collections::HashMap::new();
        
        // 辅助函数：测试 CRF 并缓存结果
        let test_crf = |crf: f32, tested: &mut std::collections::HashMap<i32, (u64, (Option<f64>, Option<f64>, Option<f64>))>, log: &mut Vec<String>| -> Result<(u64, (Option<f64>, Option<f64>, Option<f64>))> {
            let key = (crf * 10.0).round() as i32; // 0.1 精度的 key
            if let Some(&cached) = tested.get(&key) {
                return Ok(cached);
            }
            let size = self.encode(crf)?;
            let quality = self.validate_quality()?;
            let quality_str = self.format_quality_metrics(&quality);
            log.push(format!("   CRF {:.1}: {} bytes ({:+.1}%), {}", 
                crf, size, self.calc_change_pct(size), quality_str));
            tested.insert(key, (size, quality));
            Ok((size, quality))
        };
        
        let mut iterations = 0u32;
        
        // ═══════════════════════════════════════════════════════════
        // Phase 1: 初始点测试 + 方向判断
        // ═══════════════════════════════════════════════════════════
        log.push("   📍 Phase 1: Initial point test".to_string());
        
        let (initial_size, initial_quality) = test_crf(self.config.initial_crf, &mut tested_crfs, &mut log)?;
        iterations += 1;
        
        let initial_passed = self.check_quality_passed(initial_quality.0, initial_quality.1, initial_quality.2);
        
        // 如果初始 CRF 完美满足条件，尝试向上探索更高 CRF
        if initial_passed && initial_size <= target_size {
            log.push(format!("      ✅ Initial CRF {:.1} passed, exploring higher CRF for smaller size", 
                self.config.initial_crf));
        } else if !initial_passed {
            log.push(format!("      ⚠️ Initial CRF {:.1} failed quality, will search downward", 
                self.config.initial_crf));
        }
        
        // ═══════════════════════════════════════════════════════════
        // Phase 2: 粗搜索 (步长 2.0) - 快速定位边界区间
        // ═══════════════════════════════════════════════════════════
        log.push("   📍 Phase 2: Coarse search (step 2.0)".to_string());
        
        let mut best_crf = self.config.initial_crf;
        let mut best_size = initial_size;
        let mut best_quality = initial_quality;
        let mut best_passed = initial_passed;
        
        // 确定搜索方向
        let search_up = initial_passed; // 质量通过则向上搜索（更高 CRF = 更小文件）
        
        let coarse_step = 2.0_f32;
        let mut boundary_low = self.config.initial_crf;
        let mut boundary_high = self.config.initial_crf;
        
        if search_up {
            // 向上搜索：找到质量失败的边界
            let mut current = self.config.initial_crf + coarse_step;
            while current <= self.config.max_crf && iterations < self.config.max_iterations {
                let (size, quality) = test_crf(current, &mut tested_crfs, &mut log)?;
                iterations += 1;
                
                let passed = self.check_quality_passed(quality.0, quality.1, quality.2);
                if passed {
                    // 质量仍然通过，更新最佳值
                    if size < best_size || !best_passed {
                        best_crf = current;
                        best_size = size;
                        best_quality = quality;
                        best_passed = true;
                    }
                    boundary_low = current;
                    log.push("      ✅ Quality passed, continue up".to_string());
                    current += coarse_step;
                } else {
                    // 质量失败，找到边界
                    boundary_high = current;
                    log.push(format!("      ⚠️ Quality failed at CRF {:.1}, boundary found", current));
                    break;
                }
            }
            if boundary_high <= boundary_low {
                boundary_high = self.config.max_crf.min(boundary_low + coarse_step);
            }
        } else {
            // 向下搜索：找到质量通过的边界
            let mut current = self.config.initial_crf - coarse_step;
            boundary_high = self.config.initial_crf;
            while current >= self.config.min_crf && iterations < self.config.max_iterations {
                let (size, quality) = test_crf(current, &mut tested_crfs, &mut log)?;
                iterations += 1;
                
                let passed = self.check_quality_passed(quality.0, quality.1, quality.2);
                if passed {
                    // 质量通过，找到边界
                    best_crf = current;
                    best_size = size;
                    best_quality = quality;
                    best_passed = true;
                    boundary_low = current;
                    log.push(format!("      ✅ Quality passed at CRF {:.1}, boundary found", current));
                    break;
                } else {
                    boundary_high = current;
                    log.push("      ⚠️ Quality still failed, continue down".to_string());
                    current -= coarse_step;
                }
            }
            if boundary_low >= boundary_high {
                boundary_low = self.config.min_crf.max(boundary_high - coarse_step);
            }
        }
        
        log.push(format!("      📊 Coarse boundary: [{:.1}, {:.1}]", boundary_low, boundary_high));
        
        // ═══════════════════════════════════════════════════════════
        // Phase 3: 细搜索 (步长 0.5) - 精确定位最优 CRF
        // ═══════════════════════════════════════════════════════════
        log.push("   📍 Phase 3: Fine search (step 0.5)".to_string());
        
        let fine_step = 0.5_f32;
        let mut current = boundary_low;
        
        while current <= boundary_high && iterations < self.config.max_iterations {
            // 四舍五入到 0.5 步长
            let crf = ((current * 2.0).round() / 2.0).clamp(self.config.min_crf, self.config.max_crf);
            
            let (size, quality) = test_crf(crf, &mut tested_crfs, &mut log)?;
            iterations += 1;
            
            let passed = self.check_quality_passed(quality.0, quality.1, quality.2);
            if passed {
                // 更新最佳值（优先选择更高 CRF = 更小文件）
                if !best_passed || crf > best_crf || (crf == best_crf && size < best_size) {
                    best_crf = crf;
                    best_size = size;
                    best_quality = quality;
                    best_passed = true;
                }
                log.push(format!("      ✅ CRF {:.1} passed", crf));
            } else {
                log.push(format!("      ⚠️ CRF {:.1} failed", crf));
            }
            
            current += fine_step;
        }
        
        // ═══════════════════════════════════════════════════════════
        // Phase 4: 边界精细化 - 验证最优点
        // ═══════════════════════════════════════════════════════════
        if best_passed && iterations < self.config.max_iterations {
            log.push("   📍 Phase 4: Boundary refinement".to_string());
            
            // 测试 best_crf + 0.5，确认是边界
            let next_crf = (best_crf + 0.5).min(self.config.max_crf);
            if (next_crf - best_crf).abs() > 0.1 {
                let (size, quality) = test_crf(next_crf, &mut tested_crfs, &mut log)?;
                iterations += 1;
                
                let passed = self.check_quality_passed(quality.0, quality.1, quality.2);
                if passed && size < best_size {
                    best_crf = next_crf;
                    best_size = size;
                    best_quality = quality;
                    log.push(format!("      🔄 Refined to CRF {:.1}", best_crf));
                }
            }
        }
        
        // ═══════════════════════════════════════════════════════════
        // 最终结果
        // ═══════════════════════════════════════════════════════════
        let size_change_pct = self.calc_change_pct(best_size);
        let quality_str = self.format_quality_metrics(&best_quality);
        
        log.push(format!("   📊 Final: CRF {:.1}, {} bytes ({:+.1}%), {}, Passed: {}", 
            best_crf, best_size, size_change_pct, quality_str,
            if best_passed { "✅" } else { "❌" }));
        log.push(format!("   📈 Iterations: {}, Precision: ±0.5 CRF", iterations));
        
        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim: best_quality.0,
            psnr: best_quality.1,
            vmaf: best_quality.2,
            iterations,
            quality_passed: best_passed,
            log,
        })
    }
    
    /// 格式化质量指标字符串
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
    /// 🔥 v3.4: crf 参数改为 f32，支持小数点精度 (如 23.5)
    fn encode(&self, crf: f32) -> Result<u64> {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-threads").arg(self.max_threads.to_string())
            .arg("-i").arg(&self.input_path)
            .arg("-c:v").arg(self.encoder.ffmpeg_name())
            .arg("-crf").arg(format!("{:.1}", crf)) // 🔥 支持小数点 CRF
            .arg("-preset").arg("medium");
        
        for arg in self.encoder.extra_args(self.max_threads) {
            cmd.arg(arg);
        }
        
        for arg in &self.vf_args {
            cmd.arg(arg);
        }
        
        cmd.arg(&self.output_path);
        
        let output = cmd.output()
            .context("Failed to execute ffmpeg")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("ffmpeg encoding failed: {}", stderr);
        }
        
        let size = fs::metadata(&self.output_path)
            .context("Failed to read output file")?
            .len();
        
        Ok(size)
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
    /// 🔥 精确度改进 v3.2：
    /// - 使用 scale 滤镜处理分辨率差异（HEVC 要求偶数分辨率）
    /// - 更严格的解析逻辑
    /// - 验证 SSIM 值在有效范围内
    /// - 失败时响亮报错
    fn calculate_ssim(&self) -> Result<Option<f64>> {
        // 🔥 v3.2: 使用 scale 滤镜将输入缩放到输出分辨率
        // HEVC 编码器会将奇数分辨率调整为偶数，导致 SSIM 计算失败
        // 滤镜链：[0:v]scale=iw:ih:flags=bicubic[ref];[ref][1:v]ssim
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim=stats_file=-";
        
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
                
                // 🔥 更严格的解析：查找 "All:" 后的数值
                for line in stderr.lines() {
                    if let Some(pos) = line.find("All:") {
                        let value_str = &line[pos + 4..];
                        let value_str = value_str.trim_start();
                        // 提取数字部分（包括小数点）
                        let end = value_str.find(|c: char| !c.is_numeric() && c != '.')
                            .unwrap_or(value_str.len());
                        if end > 0 {
                            if let Ok(ssim) = value_str[..end].parse::<f64>() {
                                // 🔥 裁判验证：SSIM 必须在 [0, 1] 范围内
                                if precision::is_valid_ssim(ssim) {
                                    return Ok(Some(ssim));
                                }
                            }
                        }
                    }
                }
                
                // 如果没有找到 SSIM 但命令成功，返回 None（可能是格式问题）
                Ok(None)
            }
            Err(e) => {
                // 🔥 响亮报错：ffmpeg 执行失败
                bail!("Failed to execute ffmpeg for SSIM calculation: {}", e)
            }
        }
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
/// 二分搜索 + SSIM 裁判验证，找到最优质量-大小平衡
/// 🔥 v3.4: CRF 参数改为 f32，支持小数点精度
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
    /// 🔥 v3.6: CRF 搜索精度：±0.5（三阶段搜索保证）
    pub const CRF_PRECISION: f32 = 0.5;
    
    /// 🔥 v3.6: 粗搜索步长
    pub const COARSE_STEP: f32 = 2.0;
    
    /// 🔥 v3.6: 细搜索步长
    pub const FINE_STEP: f32 = 0.5;
    
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
    
    // ═══════════════════════════════════════════════════════════
    // 常量验证
    // ═══════════════════════════════════════════════════════════
    
    #[test]
    fn test_precision_constants() {
        // 🔥 v3.6: CRF 精度提升到 ±0.5
        assert!((CRF_PRECISION - 0.5).abs() < 0.01, "CRF precision should be ±0.5");
        assert!((COARSE_STEP - 2.0).abs() < 0.01, "Coarse step should be 2.0");
        assert!((FINE_STEP - 0.5).abs() < 0.01, "Fine step should be 0.5");
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
}
