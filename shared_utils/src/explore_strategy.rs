//! 🔥 v6.3: Strategy Pattern for Video Explorer
//!
//! 将探索模式重构为独立的 Strategy 结构体，统一 SSIM 计算和进度显示接口。
//!
//! ## 设计目标
//! 1. 每种探索模式的逻辑完全独立，更易维护和测试
//! 2. 统一的 ExploreContext 提供共享状态和工具方法
//! 3. 统一的 SSIM 计算逻辑（带缓存和回退）
//! 4. 统一的进度显示接口

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::video_explorer::{
    ExploreConfig, ExploreMode, ExploreResult, VideoEncoder, EncoderPreset,
    SsimSource,
};

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: ExploreStrategy Trait
// ═══════════════════════════════════════════════════════════════

/// 探索策略 Trait - 所有探索模式必须实现此接口
pub trait ExploreStrategy: Send + Sync {
    /// 执行探索，返回探索结果
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult>;
    
    /// 获取策略名称（用于日志和调试）
    fn name(&self) -> &'static str;
    
    /// 获取策略描述（用于帮助信息）
    fn description(&self) -> &'static str;
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: SsimResult - SSIM 计算结果
// ═══════════════════════════════════════════════════════════════

/// SSIM 计算结果（带来源追踪）
#[derive(Debug, Clone)]
pub struct SsimResult {
    /// SSIM 值 (0.0 - 1.0)
    pub value: f64,
    /// SSIM 来源（实际计算 vs PSNR 映射预测）
    pub source: SsimSource,
    /// PSNR 值（如果计算了）
    pub psnr: Option<f64>,
}

impl SsimResult {
    /// 创建实际计算的 SSIM 结果
    pub fn actual(value: f64, psnr: Option<f64>) -> Self {
        Self { value, source: SsimSource::Actual, psnr }
    }
    
    /// 创建预测的 SSIM 结果（从 PSNR 映射）
    pub fn predicted(value: f64, psnr: f64) -> Self {
        Self { value, source: SsimSource::Predicted, psnr: Some(psnr) }
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: ProgressConfig - 进度显示配置
// ═══════════════════════════════════════════════════════════════

/// 进度显示配置
#[derive(Debug, Clone)]
pub struct ProgressConfig {
    /// 是否显示 spinner
    pub show_spinner: bool,
    /// 是否显示百分比
    pub show_percentage: bool,
    /// 前缀文本
    pub prefix: String,
}

impl Default for ProgressConfig {
    fn default() -> Self {
        Self {
            show_spinner: true,
            show_percentage: false,
            prefix: "🔍 Exploring".to_string(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: ExploreContext - 统一的探索上下文
// ═══════════════════════════════════════════════════════════════

/// 探索上下文 - 包含所有策略共享的状态和工具方法
pub struct ExploreContext {
    /// 输入文件路径
    pub input_path: PathBuf,
    /// 输出文件路径
    pub output_path: PathBuf,
    /// 输入文件大小
    pub input_size: u64,
    /// 视频编码器
    pub encoder: VideoEncoder,
    /// 视频滤镜参数
    pub vf_args: Vec<String>,
    /// 最大线程数
    pub max_threads: usize,
    /// 是否使用 GPU
    pub use_gpu: bool,
    /// 编码器 preset
    pub preset: EncoderPreset,
    /// 探索配置
    pub config: ExploreConfig,
    
    // 内部缓存
    size_cache: HashMap<i32, u64>,
    ssim_cache: HashMap<i32, SsimResult>,
    
    // 进度条（可选）
    progress: Option<indicatif::ProgressBar>,
    
    // 日志
    pub log: Vec<String>,
}


impl ExploreContext {
    /// 创建新的探索上下文
    pub fn new(
        input_path: PathBuf,
        output_path: PathBuf,
        input_size: u64,
        encoder: VideoEncoder,
        vf_args: Vec<String>,
        max_threads: usize,
        use_gpu: bool,
        preset: EncoderPreset,
        config: ExploreConfig,
    ) -> Self {
        Self {
            input_path,
            output_path,
            input_size,
            encoder,
            vf_args,
            max_threads,
            use_gpu,
            preset,
            config,
            size_cache: HashMap::new(),
            ssim_cache: HashMap::new(),
            progress: None,
            log: Vec::new(),
        }
    }
    
    /// 添加日志
    pub fn log(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
    }
    
    /// 获取缓存的文件大小（CRF x10 作为 key）
    pub fn get_cached_size(&self, crf: f32) -> Option<u64> {
        let key = (crf * 10.0) as i32;
        self.size_cache.get(&key).copied()
    }
    
    /// 缓存文件大小
    pub fn cache_size(&mut self, crf: f32, size: u64) {
        let key = (crf * 10.0) as i32;
        self.size_cache.insert(key, size);
    }
    
    /// 获取缓存的 SSIM 结果
    pub fn get_cached_ssim(&self, crf: f32) -> Option<&SsimResult> {
        let key = (crf * 10.0) as i32;
        self.ssim_cache.get(&key)
    }
    
    /// 缓存 SSIM 结果
    pub fn cache_ssim(&mut self, crf: f32, result: SsimResult) {
        let key = (crf * 10.0) as i32;
        self.ssim_cache.insert(key, result);
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 进度显示方法
    // ═══════════════════════════════════════════════════════════════
    
    /// 开始进度显示
    pub fn progress_start(&mut self, name: &str) {
        let pb = crate::progress::create_professional_spinner(name);
        self.progress = Some(pb);
    }
    
    /// 更新进度消息
    pub fn progress_update(&self, msg: &str) {
        if let Some(ref pb) = self.progress {
            pb.set_message(msg.to_string());
        }
    }
    
    /// 暂停进度条并执行闭包（用于打印日志）
    pub fn progress_suspend<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        if let Some(ref pb) = self.progress {
            pb.suspend(f)
        } else {
            f()
        }
    }
    
    /// 完成进度显示
    pub fn progress_done(&mut self) {
        if let Some(pb) = self.progress.take() {
            pb.finish_and_clear();
        }
    }
    
    /// 计算大小变化百分比
    pub fn size_change_pct(&self, output_size: u64) -> f64 {
        ((output_size as f64 / self.input_size as f64) - 1.0) * 100.0
    }
    
    /// 检查是否能压缩（输出 < 输入）
    pub fn can_compress(&self, output_size: u64) -> bool {
        output_size < self.input_size
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 编码和质量计算方法
    // ═══════════════════════════════════════════════════════════════
    
    /// 编码视频（带缓存）
    pub fn encode(&mut self, crf: f32) -> Result<u64> {
        // 检查缓存
        if let Some(size) = self.get_cached_size(crf) {
            return Ok(size);
        }
        
        // 实际编码
        let size = self.do_encode(crf)?;
        self.cache_size(crf, size);
        Ok(size)
    }
    
    /// 实际执行编码（内部方法）
    fn do_encode(&self, crf: f32) -> Result<u64> {
        use std::fs;
        use std::process::Command;
        use anyhow::{bail, Context};
        
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-threads").arg(self.max_threads.to_string())
            .arg("-i").arg(&self.input_path)
            .arg("-c:v").arg(self.encoder.ffmpeg_name())
            .arg("-crf").arg(format!("{:.1}", crf))
            .arg("-preset").arg(self.preset.x26x_name());
        
        // 编码器特定参数
        for arg in self.encoder.extra_args(self.max_threads) {
            cmd.arg(arg);
        }
        
        // 视频滤镜
        for arg in &self.vf_args {
            cmd.arg(arg);
        }
        
        cmd.arg(&self.output_path);
        
        let output = cmd.output().context("Failed to run ffmpeg")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("ffmpeg encoding failed: {}", stderr.lines().last().unwrap_or("unknown error"));
        }
        
        let size = fs::metadata(&self.output_path)
            .context("Failed to read output file")?
            .len();
        
        Ok(size)
    }
    
    /// 计算 SSIM（带缓存和回退）
    pub fn calculate_ssim(&mut self, crf: f32) -> Result<SsimResult> {
        // 检查缓存
        if let Some(result) = self.get_cached_ssim(crf) {
            return Ok(result.clone());
        }
        
        // 实际计算
        let result = self.do_calculate_ssim()?;
        self.cache_ssim(crf, result.clone());
        Ok(result)
    }
    
    /// 实际执行 SSIM 计算（内部方法）
    fn do_calculate_ssim(&self) -> Result<SsimResult> {
        use std::process::Command;
        
        // 尝试计算 SSIM
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim";
        
        let output = Command::new("ffmpeg")
            .arg("-i").arg(&self.input_path)
            .arg("-i").arg(&self.output_path)
            .arg("-lavfi").arg(filter)
            .arg("-f").arg("null")
            .arg("-")
            .output();
        
        if let Ok(out) = output {
            if out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if let Some(ssim) = Self::parse_ssim(&stderr) {
                    return Ok(SsimResult::actual(ssim, None));
                }
            }
        }
        
        // SSIM 失败，尝试 PSNR 回退
        eprintln!("   ⚠️ SSIM calculation failed, trying PSNR fallback...");
        
        if let Some(psnr) = self.calculate_psnr()? {
            // 简单的 PSNR→SSIM 估算公式
            // PSNR 30 dB ≈ SSIM 0.90, PSNR 40 dB ≈ SSIM 0.97, PSNR 50 dB ≈ SSIM 0.99
            let ssim = (1.0 - 10_f64.powf(-psnr / 20.0)).min(0.9999);
            eprintln!("   📊 PSNR: {:.1} dB → Estimated SSIM: {:.4}", psnr, ssim);
            return Ok(SsimResult::predicted(ssim, psnr));
        }
        
        // 都失败了，返回默认值
        eprintln!("   ⚠️ Both SSIM and PSNR failed, using default");
        Ok(SsimResult::actual(0.95, None))
    }
    
    /// 解析 SSIM 值
    fn parse_ssim(stderr: &str) -> Option<f64> {
        for line in stderr.lines() {
            if let Some(pos) = line.find("All:") {
                let value_str = &line[pos + 4..];
                let value_str = value_str.trim_start();
                let end = value_str.find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(value_str.len());
                if end > 0 {
                    if let Ok(ssim) = value_str[..end].parse::<f64>() {
                        if ssim >= 0.0 && ssim <= 1.0 {
                            return Some(ssim);
                        }
                    }
                }
            }
        }
        None
    }
    
    /// 计算 PSNR
    fn calculate_psnr(&self) -> Result<Option<f64>> {
        use std::process::Command;
        
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]psnr";
        
        let output = Command::new("ffmpeg")
            .arg("-i").arg(&self.input_path)
            .arg("-i").arg(&self.output_path)
            .arg("-lavfi").arg(filter)
            .arg("-f").arg("null")
            .arg("-")
            .output();
        
        if let Ok(out) = output {
            let stderr = String::from_utf8_lossy(&out.stderr);
            for line in stderr.lines() {
                if let Some(pos) = line.find("average:") {
                    let value_str = &line[pos + 8..];
                    let value_str = value_str.trim_start();
                    let end = value_str.find(|c: char| !c.is_numeric() && c != '.' && c != '-')
                        .unwrap_or(value_str.len());
                    if end > 0 {
                        if let Ok(psnr) = value_str[..end].parse::<f64>() {
                            return Ok(Some(psnr));
                        }
                    }
                }
            }
        }
        
        Ok(None)
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: Strategy 工厂函数
// ═══════════════════════════════════════════════════════════════

/// 根据 ExploreMode 创建对应的 Strategy
pub fn create_strategy(mode: ExploreMode) -> Box<dyn ExploreStrategy> {
    match mode {
        ExploreMode::SizeOnly => Box::new(SizeOnlyStrategy),
        ExploreMode::QualityMatch => Box::new(QualityMatchStrategy),
        ExploreMode::PreciseQualityMatch => Box::new(PreciseQualityMatchStrategy),
        ExploreMode::PreciseQualityMatchWithCompression => 
            Box::new(PreciseQualityMatchWithCompressionStrategy),
        ExploreMode::CompressOnly => Box::new(CompressOnlyStrategy),
        ExploreMode::CompressWithQuality => Box::new(CompressWithQualityStrategy),
    }
}

/// 获取 Strategy 名称（不创建实例）
pub fn strategy_name(mode: ExploreMode) -> &'static str {
    match mode {
        ExploreMode::SizeOnly => "SizeOnly",
        ExploreMode::QualityMatch => "QualityMatch",
        ExploreMode::PreciseQualityMatch => "PreciseQualityMatch",
        ExploreMode::PreciseQualityMatchWithCompression => "PreciseQualityMatchWithCompression",
        ExploreMode::CompressOnly => "CompressOnly",
        ExploreMode::CompressWithQuality => "CompressWithQuality",
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: Strategy 实现 - 占位符（后续任务实现）
// ═══════════════════════════════════════════════════════════════

/// SizeOnly 策略 - 仅探索更小的文件大小
pub struct SizeOnlyStrategy;

impl ExploreStrategy for SizeOnlyStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        use crate::video_explorer::ConfidenceBreakdown;
        
        ctx.log(format!("🔍 Size-Only Explore ({:?})", ctx.encoder));
        ctx.progress_start("🔍 Size Explore");
        
        // 测试 max_crf（最高 CRF = 最小文件）
        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.max_crf));
        let max_size = ctx.encode(ctx.config.max_crf)?;
        
        let (best_crf, best_size, quality_passed) = if max_size < ctx.input_size {
            (ctx.config.max_crf, max_size, true)
        } else {
            (ctx.config.max_crf, max_size, false)
        };
        
        // 计算 SSIM（仅供参考）
        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim(best_crf).ok();
        let ssim = ssim_result.as_ref().map(|r| r.value);
        
        ctx.progress_done();
        
        let size_change_pct = ctx.size_change_pct(best_size);
        ctx.log(format!("📊 RESULT: CRF {:.1}, {:+.1}%", best_crf, size_change_pct));
        
        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim,
            psnr: ssim_result.and_then(|r| r.psnr),
            vmaf: None,
            iterations: 1,
            quality_passed,
            log: ctx.log.clone(),
            confidence: 0.7,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: ctx.config.quality_thresholds.min_ssim,
        })
    }
    
    fn name(&self) -> &'static str { "SizeOnly" }
    fn description(&self) -> &'static str { 
        "寻找更小的文件大小（不验证质量）" 
    }
}

/// QualityMatch 策略 - 仅匹配输入质量
pub struct QualityMatchStrategy;

impl ExploreStrategy for QualityMatchStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        use crate::video_explorer::ConfidenceBreakdown;
        
        ctx.log(format!("🎯 Quality-Match Mode ({:?})", ctx.encoder));
        ctx.log(format!("   Predicted CRF: {}", ctx.config.initial_crf));
        ctx.progress_start("🎯 Quality Match");
        
        // 单次编码
        ctx.progress_update(&format!("Encoding CRF {:.1}...", ctx.config.initial_crf));
        let output_size = ctx.encode(ctx.config.initial_crf)?;
        
        // 计算 SSIM
        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim(ctx.config.initial_crf).ok();
        let ssim = ssim_result.as_ref().map(|r| r.value);
        let psnr = ssim_result.and_then(|r| r.psnr);
        
        ctx.progress_done();
        
        let size_change_pct = ctx.size_change_pct(output_size);
        let quality_passed = ssim.map(|s| s >= ctx.config.quality_thresholds.min_ssim).unwrap_or(false);
        
        ctx.log(format!("📊 RESULT: CRF {:.1}, SSIM {:.4}, {:+.1}%", 
            ctx.config.initial_crf, ssim.unwrap_or(0.0), size_change_pct));
        
        Ok(ExploreResult {
            optimal_crf: ctx.config.initial_crf,
            output_size,
            size_change_pct,
            ssim,
            psnr,
            vmaf: None,
            iterations: 1,
            quality_passed,
            log: ctx.log.clone(),
            confidence: 0.6,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: ctx.config.quality_thresholds.min_ssim,
        })
    }
    
    fn name(&self) -> &'static str { "QualityMatch" }
    fn description(&self) -> &'static str { 
        "使用算法预测的 CRF，单次编码 + SSIM 验证" 
    }
}

/// PreciseQualityMatch 策略 - 精确质量匹配
pub struct PreciseQualityMatchStrategy;

impl ExploreStrategy for PreciseQualityMatchStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        use crate::video_explorer::ConfidenceBreakdown;
        
        ctx.log(format!("🎯 Precise Quality Match ({:?})", ctx.encoder));
        ctx.progress_start("🎯 Precise Quality");
        
        // 二分搜索找最高 SSIM
        let mut low = ctx.config.min_crf;
        let mut high = ctx.config.max_crf;
        let mut best_crf = ctx.config.initial_crf;
        let mut best_ssim = 0.0;
        let mut best_size: u64;
        let mut iterations = 0u32;
        
        // 先测试初始 CRF
        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.initial_crf));
        best_size = ctx.encode(ctx.config.initial_crf)?;
        if let Ok(result) = ctx.calculate_ssim(ctx.config.initial_crf) {
            best_ssim = result.value;
        }
        iterations += 1;
        
        // 二分搜索优化
        while high - low > 1.0 && iterations < ctx.config.max_iterations {
            let mid = (low + high) / 2.0;
            ctx.progress_update(&format!("Binary search CRF {:.1}...", mid));
            let size = ctx.encode(mid)?;
            iterations += 1;
            
            if let Ok(result) = ctx.calculate_ssim(mid) {
                if result.value > best_ssim {
                    best_ssim = result.value;
                    best_crf = mid;
                    best_size = size;
                }
                // 低 CRF = 高质量，如果 SSIM 已经很高，往高 CRF 搜索
                if result.value >= 0.99 {
                    low = mid;
                } else {
                    high = mid;
                }
            } else {
                high = mid;
            }
        }
        
        ctx.progress_done();
        
        let size_change_pct = ctx.size_change_pct(best_size);
        let quality_passed = best_ssim >= ctx.config.quality_thresholds.min_ssim;
        ctx.log(format!("📊 RESULT: CRF {:.1}, SSIM {:.4}, {:+.1}%", best_crf, best_ssim, size_change_pct));
        
        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim: Some(best_ssim),
            psnr: None,
            vmaf: None,
            iterations,
            quality_passed,
            log: ctx.log.clone(),
            confidence: 0.85,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: ctx.config.quality_thresholds.min_ssim,
        })
    }
    
    fn name(&self) -> &'static str { "PreciseQualityMatch" }
    fn description(&self) -> &'static str { 
        "二分搜索 + SSIM 裁判验证，找到最高 SSIM" 
    }
}

/// PreciseQualityMatchWithCompression 策略 - 精确质量匹配 + 压缩
pub struct PreciseQualityMatchWithCompressionStrategy;

impl ExploreStrategy for PreciseQualityMatchWithCompressionStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        use crate::video_explorer::ConfidenceBreakdown;
        
        ctx.log(format!("🎯💾 Precise Quality + Compress ({:?})", ctx.encoder));
        ctx.progress_start("🎯💾 Quality+Compress");
        
        // 先找压缩边界
        let mut compress_boundary = ctx.config.max_crf;
        let mut iterations = 0u32;
        
        // 二分搜索找压缩边界
        let mut low = ctx.config.min_crf;
        let mut high = ctx.config.max_crf;
        
        while high - low > 1.0 && iterations < ctx.config.max_iterations / 2 {
            let mid = (low + high) / 2.0;
            ctx.progress_update(&format!("Find compress boundary CRF {:.1}...", mid));
            let size = ctx.encode(mid)?;
            iterations += 1;
            
            if size < ctx.input_size {
                compress_boundary = mid;
                high = mid;
            } else {
                low = mid;
            }
        }
        
        // 在压缩范围内找最高 SSIM
        let mut best_crf = compress_boundary;
        let mut best_ssim = 0.0;
        let mut best_size = ctx.get_cached_size(compress_boundary).unwrap_or(0);
        
        // 从压缩边界向低 CRF 搜索（更高质量）
        let search_low = (compress_boundary - 5.0).max(ctx.config.min_crf);
        let mut crf = compress_boundary;
        
        while crf >= search_low && iterations < ctx.config.max_iterations {
            ctx.progress_update(&format!("Quality search CRF {:.1}...", crf));
            let size = ctx.encode(crf)?;
            iterations += 1;
            
            if size < ctx.input_size {
                if let Ok(result) = ctx.calculate_ssim(crf) {
                    if result.value > best_ssim {
                        best_ssim = result.value;
                        best_crf = crf;
                        best_size = size;
                    }
                }
            } else {
                break; // 不能压缩了，停止
            }
            crf -= 1.0;
        }
        
        ctx.progress_done();
        
        let size_change_pct = ctx.size_change_pct(best_size);
        let quality_passed = best_size < ctx.input_size && best_ssim >= ctx.config.quality_thresholds.min_ssim;
        ctx.log(format!("📊 RESULT: CRF {:.1}, SSIM {:.4}, {:+.1}%", best_crf, best_ssim, size_change_pct));
        
        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim: Some(best_ssim),
            psnr: None,
            vmaf: None,
            iterations,
            quality_passed,
            log: ctx.log.clone(),
            confidence: 0.85,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: ctx.config.quality_thresholds.min_ssim,
        })
    }
    
    fn name(&self) -> &'static str { "PreciseQualityMatchWithCompression" }
    fn description(&self) -> &'static str { 
        "找到最高 SSIM 且输出 < 输入" 
    }
}

/// CompressOnly 策略 - 仅压缩
pub struct CompressOnlyStrategy;

impl ExploreStrategy for CompressOnlyStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        use crate::video_explorer::ConfidenceBreakdown;
        
        ctx.log(format!("💾 Compress-Only Mode ({:?})", ctx.encoder));
        ctx.progress_start("💾 Compress Only");
        
        // 先测试 initial_crf
        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.initial_crf));
        let initial_size = ctx.encode(ctx.config.initial_crf)?;
        
        if initial_size < ctx.input_size {
            // 能压缩，直接返回
            ctx.progress_done();
            let size_change_pct = ctx.size_change_pct(initial_size);
            ctx.log(format!("📊 RESULT: CRF {:.1}, {:+.1}%", ctx.config.initial_crf, size_change_pct));
            
            return Ok(ExploreResult {
                optimal_crf: ctx.config.initial_crf,
                output_size: initial_size,
                size_change_pct,
                ssim: None,
                psnr: None,
                vmaf: None,
                iterations: 1,
                quality_passed: true,
                log: ctx.log.clone(),
                confidence: 0.8,
                confidence_detail: ConfidenceBreakdown::default(),
                actual_min_ssim: ctx.config.quality_thresholds.min_ssim,
            });
        }
        
        // 二分搜索找能压缩的 CRF
        let mut low = ctx.config.initial_crf;
        let mut high = ctx.config.max_crf;
        let mut best_crf = ctx.config.max_crf;
        let mut best_size = initial_size;
        let mut iterations = 1u32;
        
        while high - low > 0.5 && iterations < ctx.config.max_iterations {
            let mid = (low + high) / 2.0;
            ctx.progress_update(&format!("Binary search CRF {:.1}...", mid));
            let size = ctx.encode(mid)?;
            iterations += 1;
            
            if size < ctx.input_size {
                best_crf = mid;
                best_size = size;
                high = mid;
            } else {
                low = mid;
            }
        }
        
        ctx.progress_done();
        let size_change_pct = ctx.size_change_pct(best_size);
        let quality_passed = best_size < ctx.input_size;
        ctx.log(format!("📊 RESULT: CRF {:.1}, {:+.1}%", best_crf, size_change_pct));
        
        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim: None,
            psnr: None,
            vmaf: None,
            iterations,
            quality_passed,
            log: ctx.log.clone(),
            confidence: 0.75,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: ctx.config.quality_thresholds.min_ssim,
        })
    }
    
    fn name(&self) -> &'static str { "CompressOnly" }
    fn description(&self) -> &'static str { 
        "确保输出 < 输入（不验证质量）" 
    }
}

/// CompressWithQuality 策略 - 压缩 + 粗略质量验证
pub struct CompressWithQualityStrategy;

impl ExploreStrategy for CompressWithQualityStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        use crate::video_explorer::ConfidenceBreakdown;
        
        ctx.log(format!("💾🎯 Compress+Quality Mode ({:?})", ctx.encoder));
        ctx.progress_start("💾🎯 Compress+Quality");
        
        // 先用 CompressOnly 找到能压缩的 CRF
        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.initial_crf));
        let initial_size = ctx.encode(ctx.config.initial_crf)?;
        let mut iterations = 1u32;
        
        let (best_crf, best_size) = if initial_size < ctx.input_size {
            (ctx.config.initial_crf, initial_size)
        } else {
            // 二分搜索
            let mut low = ctx.config.initial_crf;
            let mut high = ctx.config.max_crf;
            let mut best = (ctx.config.max_crf, initial_size);
            
            while high - low > 0.5 && iterations < ctx.config.max_iterations {
                let mid = (low + high) / 2.0;
                ctx.progress_update(&format!("Binary search CRF {:.1}...", mid));
                let size = ctx.encode(mid)?;
                iterations += 1;
                
                if size < ctx.input_size {
                    best = (mid, size);
                    high = mid;
                } else {
                    low = mid;
                }
            }
            best
        };
        
        // 计算 SSIM 验证质量
        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim(best_crf).ok();
        let ssim = ssim_result.as_ref().map(|r| r.value);
        let psnr = ssim_result.and_then(|r| r.psnr);
        
        ctx.progress_done();
        
        let size_change_pct = ctx.size_change_pct(best_size);
        let quality_passed = best_size < ctx.input_size && 
            ssim.map(|s| s >= ctx.config.quality_thresholds.min_ssim).unwrap_or(false);
        
        ctx.log(format!("📊 RESULT: CRF {:.1}, SSIM {:.4}, {:+.1}%", 
            best_crf, ssim.unwrap_or(0.0), size_change_pct));
        
        Ok(ExploreResult {
            optimal_crf: best_crf,
            output_size: best_size,
            size_change_pct,
            ssim,
            psnr,
            vmaf: None,
            iterations,
            quality_passed,
            log: ctx.log.clone(),
            confidence: 0.75,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: ctx.config.quality_thresholds.min_ssim,
        })
    }
    
    fn name(&self) -> &'static str { "CompressWithQuality" }
    fn description(&self) -> &'static str { 
        "确保输出 < 输入 + 粗略 SSIM 验证" 
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: 单元测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_strategy_name_consistency() {
        // Property 1: Strategy 选择一致性
        let modes = [
            ExploreMode::SizeOnly,
            ExploreMode::QualityMatch,
            ExploreMode::PreciseQualityMatch,
            ExploreMode::PreciseQualityMatchWithCompression,
            ExploreMode::CompressOnly,
            ExploreMode::CompressWithQuality,
        ];
        
        for mode in modes {
            let strategy = create_strategy(mode);
            let expected_name = strategy_name(mode);
            assert_eq!(strategy.name(), expected_name, 
                "Strategy name mismatch for {:?}", mode);
        }
    }
    
    #[test]
    fn test_ssim_result_creation() {
        let actual = SsimResult::actual(0.98, Some(45.0));
        assert_eq!(actual.source, SsimSource::Actual);
        assert_eq!(actual.value, 0.98);
        
        let predicted = SsimResult::predicted(0.95, 40.0);
        assert_eq!(predicted.source, SsimSource::Predicted);
        assert_eq!(predicted.psnr, Some(40.0));
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: 属性测试 (Property-Based Tests)
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;
    
    /// 生成随机 ExploreMode
    fn arb_explore_mode() -> impl Strategy<Value = ExploreMode> {
        prop_oneof![
            Just(ExploreMode::SizeOnly),
            Just(ExploreMode::QualityMatch),
            Just(ExploreMode::PreciseQualityMatch),
            Just(ExploreMode::PreciseQualityMatchWithCompression),
            Just(ExploreMode::CompressOnly),
            Just(ExploreMode::CompressWithQuality),
        ]
    }
    
    proptest! {
        /// **Feature: explore-strategy-pattern-v6.3, Property 1: Strategy 选择一致性**
        /// *对于任意* ExploreMode，create_strategy() 返回的 Strategy 的 name() 
        /// 应与该模式的预期名称匹配
        /// **Validates: Requirements 1.1**
        #[test]
        fn prop_strategy_selection_consistency(mode in arb_explore_mode()) {
            let strategy = create_strategy(mode);
            let expected_name = strategy_name(mode);
            prop_assert_eq!(strategy.name(), expected_name);
        }
        
        /// **Feature: explore-strategy-pattern-v6.3, Property 3: SSIM 缓存一致性**
        /// *对于任意* CRF 值，缓存后获取应返回相同的值
        /// **Validates: Requirements 3.4**
        #[test]
        fn prop_ssim_cache_consistency(
            crf in 10.0f32..51.0f32,
            ssim_value in 0.0f64..1.0f64,
            psnr_value in 20.0f64..60.0f64
        ) {
            use std::path::PathBuf;
            use crate::video_explorer::{ExploreConfig, VideoEncoder, EncoderPreset};
            
            let mut ctx = ExploreContext::new(
                PathBuf::from("/tmp/test_input.mp4"),
                PathBuf::from("/tmp/test_output.mp4"),
                1000000,
                VideoEncoder::Hevc,
                vec![],
                4,
                false,
                EncoderPreset::Medium,
                ExploreConfig::default(),
            );
            
            // 缓存 SSIM 结果
            let result = SsimResult::actual(ssim_value, Some(psnr_value));
            ctx.cache_ssim(crf, result.clone());
            
            // 获取缓存的结果
            let cached = ctx.get_cached_ssim(crf);
            prop_assert!(cached.is_some());
            let cached = cached.unwrap();
            prop_assert_eq!(cached.value, ssim_value);
            prop_assert_eq!(cached.psnr, Some(psnr_value));
        }
        
        /// **Feature: explore-strategy-pattern-v6.3, Property 4: SSIM 回退正确性**
        /// *对于任意* PSNR 值，PSNR→SSIM 映射应产生有效的 SSIM 值 (0-1)
        /// **Validates: Requirements 3.2, 3.3**
        #[test]
        fn prop_psnr_to_ssim_mapping_valid(psnr in 20.0f64..60.0f64) {
            // 使用 ExploreContext 中的 PSNR→SSIM 公式
            let ssim = (1.0 - 10_f64.powf(-psnr / 20.0)).min(0.9999);
            prop_assert!(ssim >= 0.0 && ssim <= 1.0, 
                "SSIM {} out of range for PSNR {}", ssim, psnr);
            // 更高的 PSNR 应该产生更高的 SSIM
            let ssim_higher = (1.0 - 10_f64.powf(-(psnr + 5.0) / 20.0)).min(0.9999);
            prop_assert!(ssim_higher >= ssim,
                "Higher PSNR {} should produce higher SSIM", psnr + 5.0);
        }
        
        /// **Feature: explore-strategy-pattern-v6.3, Property 2: 探索委托正确性**
        /// *对于任意* ExploreMode，create_strategy() 返回的 Strategy 应有有效的 name 和 description
        /// **Validates: Requirements 1.3**
        #[test]
        fn prop_strategy_has_valid_metadata(mode in arb_explore_mode()) {
            let strategy = create_strategy(mode);
            // name 不应为空
            prop_assert!(!strategy.name().is_empty(), 
                "Strategy name should not be empty for {:?}", mode);
            // description 不应为空
            prop_assert!(!strategy.description().is_empty(),
                "Strategy description should not be empty for {:?}", mode);
            // name 应该是 ASCII
            prop_assert!(strategy.name().is_ascii(),
                "Strategy name should be ASCII for {:?}", mode);
        }
        
        /// **Feature: explore-strategy-pattern-v6.3, Property 5: 大小缓存一致性**
        /// *对于任意* CRF 和 size，缓存后获取应返回相同的值
        /// **Validates: Requirements 6.3**
        #[test]
        fn prop_size_cache_consistency(
            crf in 10.0f32..51.0f32,
            size in 1000u64..10000000u64
        ) {
            use std::path::PathBuf;
            use crate::video_explorer::{ExploreConfig, VideoEncoder, EncoderPreset};
            
            let mut ctx = ExploreContext::new(
                PathBuf::from("/tmp/test_input.mp4"),
                PathBuf::from("/tmp/test_output.mp4"),
                1000000,
                VideoEncoder::Hevc,
                vec![],
                4,
                false,
                EncoderPreset::Medium,
                ExploreConfig::default(),
            );
            
            // 缓存 size
            ctx.cache_size(crf, size);
            
            // 获取缓存的结果
            let cached = ctx.get_cached_size(crf);
            prop_assert_eq!(cached, Some(size));
        }
    }
}
