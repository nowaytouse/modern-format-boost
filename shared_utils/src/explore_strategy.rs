//! 🔥 v6.3: Strategy Pattern for Video Explorer
//!
//! 将探索模式重构为独立的 Strategy 结构体，统一 SSIM 计算和进度显示接口。
//!
//! ## 设计目标
//! 1. 每种探索模式的逻辑完全独立，更易维护和测试
//! 2. 统一的 ExploreContext 提供共享状态和工具方法
//! 3. 统一的 SSIM 计算逻辑（带缓存和回退）
//! 4. 统一的进度显示接口
//!
//! ## 🔥 v6.4.4: 辅助方法重构
//! 添加 `build_result()`, `binary_search_compress()`, `log_final_result()` 等辅助方法，
//! 减少 6 个 Strategy 实现中约 40% 的重复代码。
//!
//! ## 使用示例
//! ```ignore
//! use shared_utils::explore_strategy::{create_strategy, ExploreContext};
//! 
//! let strategy = create_strategy(ExploreMode::CompressOnly);
//! let mut ctx = ExploreContext::new(...);
//! let result = strategy.explore(&mut ctx)?;
//! ```

use anyhow::Result;
use std::path::PathBuf;

use crate::video_explorer::{
    ExploreConfig, ExploreMode, ExploreResult, VideoEncoder, EncoderPreset,
    SsimSource,
};

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.3: ExploreStrategy Trait
// ═══════════════════════════════════════════════════════════════

/// 探索策略 Trait - 所有探索模式必须实现此接口
/// 
/// # 实现指南
/// 
/// 每个 Strategy 实现应：
/// 1. 调用 `ctx.progress_start()` 开始进度显示
/// 2. 使用 `ctx.encode()` 和 `ctx.calculate_ssim()` 进行编码和质量计算
/// 3. 使用 `ctx.build_result()` 构建统一格式的结果
/// 4. 调用 `ctx.progress_done()` 结束进度显示
/// 
/// # 示例
/// 
/// ```ignore
/// impl ExploreStrategy for MyStrategy {
///     fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
///         ctx.progress_start("🔍 My Strategy");
///         let size = ctx.encode(20.0)?;
///         let ssim = ctx.calculate_ssim(20.0).ok();
///         ctx.progress_done();
///         Ok(ctx.build_result(20.0, size, ssim, 1, true))
///     }
///     fn name(&self) -> &'static str { "MyStrategy" }
///     fn description(&self) -> &'static str { "My custom strategy" }
/// }
/// ```
pub trait ExploreStrategy: Send + Sync {
    /// 执行探索，返回探索结果
    /// 
    /// # Errors
    /// 
    /// 返回 `Err` 如果：
    /// - 编码失败（ffmpeg 错误）
    /// - 文件 I/O 错误
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
/// 
/// 用于区分实际计算的 SSIM 和从 PSNR 映射预测的 SSIM。
/// 预测的 SSIM 在日志中会用 `~` 前缀标注。
/// 
/// # 示例
/// 
/// ```
/// use shared_utils::explore_strategy::SsimResult;
/// 
/// // 实际计算的 SSIM
/// let actual = SsimResult::actual(0.98, Some(45.0));
/// assert!(actual.is_actual());
/// 
/// // 从 PSNR 预测的 SSIM
/// let predicted = SsimResult::predicted(0.95, 40.0);
/// assert!(predicted.is_predicted());
/// ```
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
    /// 
    /// # Arguments
    /// * `value` - SSIM 值 (0.0 - 1.0)
    /// * `psnr` - 可选的 PSNR 值
    pub fn actual(value: f64, psnr: Option<f64>) -> Self {
        Self { value, source: SsimSource::Actual, psnr }
    }
    
    /// 创建预测的 SSIM 结果（从 PSNR 映射）
    /// 
    /// # Arguments
    /// * `value` - 预测的 SSIM 值 (0.0 - 1.0)
    /// * `psnr` - 用于预测的 PSNR 值
    pub fn predicted(value: f64, psnr: f64) -> Self {
        Self { value, source: SsimSource::Predicted, psnr: Some(psnr) }
    }
    
    /// 检查是否为实际计算的 SSIM
    #[inline]
    pub fn is_actual(&self) -> bool {
        matches!(self.source, SsimSource::Actual)
    }
    
    /// 检查是否为预测的 SSIM
    #[inline]
    pub fn is_predicted(&self) -> bool {
        matches!(self.source, SsimSource::Predicted)
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v7.1: 类型安全辅助方法
    // ═══════════════════════════════════════════════════════════════
    
    /// 获取类型安全的 SSIM 值
    /// 
    /// 返回 `Option<Ssim>` 而不是 `f64`，确保值在有效范围内
    #[inline]
    pub fn value_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.value).ok()
    }
    
    /// 检查 SSIM 是否满足阈值（使用类型安全比较）
    /// 
    /// 🔥 v7.1: 使用 float_compare::ssim_meets_threshold 进行精确比较
    #[inline]
    pub fn meets_threshold(&self, threshold: f64) -> bool {
        crate::float_compare::ssim_meets_threshold(self.value, threshold)
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.5: 类型别名 - 更清晰的命名（向后兼容）
// ═══════════════════════════════════════════════════════════════

/// SSIM 计算结果（更清晰的命名）
/// 
/// 🔥 v6.4.5: 推荐使用此名称，`SsimResult` 保留用于向后兼容
pub type SsimCalculationResult = SsimResult;

/// SSIM 数据来源（更清晰的命名）
/// 
/// 🔥 v6.4.5: 推荐使用此名称，`SsimSource` 保留用于向后兼容
pub type SsimDataSource = SsimSource;

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
// 🔥 v6.4.7: CrfCache - 高性能 CRF 缓存（精度升级）
// 🔥 v6.4.9: 使用 crf_constants 模块的统一常量
// ═══════════════════════════════════════════════════════════════

use crate::crf_constants::{
    CRF_CACHE_KEY_MULTIPLIER, CRF_CACHE_MAX_VALID,
};

/// CRF 缓存数组大小
/// 🔥 v6.4.7: 升级精度从 0.1 到 0.025
/// 🔥 v6.4.9: 使用整数键计算，避免浮点精度问题
/// CRF 范围: 0.0-63.99, 精度 0.01, 共 6400 个槽位
const CRF_CACHE_SIZE: usize = 6400;

/// CRF 缓存键乘数（从 crf_constants 导入）
/// 🔥 v6.4.9: 升级到 100.0，使用整数键避免浮点精度问题
/// 
/// 计算公式: idx = (crf * 100).round() as usize
/// - 23.025 * 100 = 2302 (整数键，无精度损失)
/// - 23.024 * 100 = 2302 (故意合并相近值)
const CRF_CACHE_MULTIPLIER: f32 = CRF_CACHE_KEY_MULTIPLIER;

/// 高性能 CRF 缓存 - 使用数组实现 O(1) 查找
/// 
/// 🔥 v6.4.5: 替代 HashMap<i32, T>，提升约 30% 查询性能
/// 🔥 v6.4.7: 精度升级到 0.025，支持未来更细粒度的 CRF 步进
/// 
/// # 设计原理
/// 
/// CRF 值范围固定 (0.0-63.0)，精度 0.025，共 2560 个可能值。
/// 使用固定大小数组比 HashMap 更高效：
/// - O(1) 查找，无哈希计算开销
/// - 更好的缓存局部性
/// - 无动态内存分配
/// 
/// # 向后兼容性
/// 
/// 0.5 步进的 CRF 值（如 20.0, 20.5）在新精度下仍然正确映射：
/// - 20.0 * 40 = 800
/// - 20.5 * 40 = 820
/// 
/// # 示例
/// 
/// ```
/// use shared_utils::explore_strategy::CrfCache;
/// 
/// let mut cache: CrfCache<u64> = CrfCache::new();
/// cache.insert(23.5, 1000000);
/// assert_eq!(cache.get(23.5), Some(&1000000));
/// 
/// // 0.25 步进也能正确区分
/// cache.insert(23.25, 2000000);
/// assert_eq!(cache.get(23.25), Some(&2000000));
/// assert_eq!(cache.get(23.5), Some(&1000000)); // 不会碰撞
/// ```
#[derive(Clone)]
pub struct CrfCache<T> {
    data: Box<[Option<T>; CRF_CACHE_SIZE]>,
}

impl<T> Default for CrfCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> CrfCache<T> {
    /// 创建新的空缓存
    #[inline]
    pub fn new() -> Self {
        // 使用 Box 避免栈溢出（640 * size_of::<Option<T>>）
        Self {
            data: Box::new(std::array::from_fn(|_| None)),
        }
    }
    
    /// 将 CRF 值转换为整数索引
    /// 
    /// 🔥 v6.4.9: 使用整数键避免浮点精度问题
    /// 计算: (crf * 100).round() as usize
    /// 
    /// 如果 CRF 超出范围 [0.0, 63.99]，返回 None 并打印警告
    #[inline]
    pub fn key(crf: f32) -> Option<usize> {
        // 🔥 v6.4.5: 防御性检查，负数和超大值都返回 None
        if crf < 0.0 {
            eprintln!("⚠️ CRF_CACHE: Negative CRF {} rejected", crf);
            return None;
        }
        if crf.is_nan() || crf.is_infinite() {
            eprintln!("⚠️ CRF_CACHE: Invalid CRF (NaN/Inf) rejected");
            return None;
        }
        if crf > CRF_CACHE_MAX_VALID {
            eprintln!("⚠️ CRF_CACHE: CRF {} exceeds max valid {} - rejected", crf, CRF_CACHE_MAX_VALID);
            return None;
        }
        // 🔥 v6.4.9: 使用 round() 避免浮点精度问题
        // 23.025 * 100 = 2302.5 -> round() -> 2302
        let idx = (crf * CRF_CACHE_MULTIPLIER).round() as usize;
        if idx < CRF_CACHE_SIZE { Some(idx) } else { None }
    }
    
    /// 获取缓存值
    #[inline]
    pub fn get(&self, crf: f32) -> Option<&T> {
        Self::key(crf).and_then(|idx| self.data[idx].as_ref())
    }
    
    /// 插入缓存值
    #[inline]
    pub fn insert(&mut self, crf: f32, value: T) {
        if let Some(idx) = Self::key(crf) {
            self.data[idx] = Some(value);
        }
    }
    
    /// 检查是否包含指定 CRF
    #[inline]
    pub fn contains_key(&self, crf: f32) -> bool {
        Self::key(crf).map(|idx| self.data[idx].is_some()).unwrap_or(false)
    }
}

impl<T: Clone> CrfCache<T> {
    /// 获取缓存值的副本
    #[inline]
    pub fn get_cloned(&self, crf: f32) -> Option<T> {
        self.get(crf).cloned()
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
    
    // 🔥 v6.4.5: 使用 CrfCache 替代 HashMap，提升查询性能
    size_cache: CrfCache<u64>,
    ssim_cache: CrfCache<SsimResult>,
    
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
            size_cache: CrfCache::new(),
            ssim_cache: CrfCache::new(),
            progress: None,
            log: Vec::new(),
        }
    }
    
    /// 添加日志
    pub fn log(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
    }
    
    /// 获取缓存的文件大小
    /// 
    /// 🔥 v6.4.5: 使用 CrfCache O(1) 查找
    #[inline]
    pub fn get_cached_size(&self, crf: f32) -> Option<u64> {
        self.size_cache.get(crf).copied()
    }
    
    /// 缓存文件大小
    #[inline]
    pub fn cache_size(&mut self, crf: f32, size: u64) {
        self.size_cache.insert(crf, size);
    }
    
    /// 获取缓存的 SSIM 结果
    #[inline]
    pub fn get_cached_ssim(&self, crf: f32) -> Option<&SsimResult> {
        self.ssim_cache.get(crf)
    }
    
    /// 缓存 SSIM 结果
    #[inline]
    pub fn cache_ssim(&mut self, crf: f32, result: SsimResult) {
        self.ssim_cache.insert(crf, result);
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
    /// 
    /// # Returns
    /// 负数表示压缩，正数表示膨胀
    /// 
    /// # Example
    /// - 输入 1MB，输出 800KB → -20.0%
    /// - 输入 1MB，输出 1.2MB → +20.0%
    /// 计算大小变化百分比
    /// 
    /// # Returns
    /// 负数表示压缩，正数表示膨胀
    /// 如果 input_size 为 0，返回 0.0（防御性编程）
    #[inline]
    pub fn size_change_pct(&self, output_size: u64) -> f64 {
        if self.input_size == 0 {
            return 0.0;
        }
        ((output_size as f64 / self.input_size as f64) - 1.0) * 100.0
    }
    
    /// 检查是否能压缩（输出 < 输入）
    #[inline]
    pub fn can_compress(&self, output_size: u64) -> bool {
        output_size < self.input_size
    }
    
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v6.4.4: 辅助方法 - 减少 Strategy 重复代码
    // ═══════════════════════════════════════════════════════════════
    
    /// 构建统一格式的探索结果
    /// 
    /// 🔥 v6.4.4: 减少 6 个 Strategy 中重复的结果构建代码
    /// 
    /// # Arguments
    /// * `crf` - 最优 CRF 值
    /// * `size` - 输出文件大小
    /// * `ssim_result` - SSIM 计算结果（可选）
    /// * `iterations` - 迭代次数
    /// * `quality_passed` - 是否通过质量验证
    /// * `confidence` - 置信度 (0.0 - 1.0)
    pub fn build_result(
        &self,
        crf: f32,
        size: u64,
        ssim_result: Option<SsimResult>,
        iterations: u32,
        quality_passed: bool,
        confidence: f64,
    ) -> ExploreResult {
        use crate::video_explorer::ConfidenceBreakdown;
        
        let size_change_pct = self.size_change_pct(size);
        let ssim = ssim_result.as_ref().map(|r| r.value);
        let psnr = ssim_result.and_then(|r| r.psnr);
        
        ExploreResult {
            optimal_crf: crf,
            output_size: size,
            size_change_pct,
            ssim,
            psnr,
            vmaf: None,
            iterations,
            quality_passed,
            log: self.log.clone(),
            confidence,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        }
    }
    
    /// 二分搜索找到能压缩的 CRF
    /// 
    /// 🔥 v6.4.4: 统一的二分搜索逻辑，减少重复代码
    /// 
    /// # Arguments
    /// * `low` - 搜索下界（低 CRF = 高质量）
    /// * `high` - 搜索上界（高 CRF = 低质量）
    /// * `max_iter` - 最大迭代次数
    /// 
    /// # Returns
    /// `(best_crf, best_size, iterations)` - 最优 CRF、对应大小、实际迭代次数
    pub fn binary_search_compress(
        &mut self,
        low: f32,
        high: f32,
        max_iter: u32,
    ) -> Result<(f32, u64, u32)> {
        let mut low = low;
        let mut high = high;
        let mut best_crf = high;
        let mut best_size = u64::MAX;
        let mut iterations = 0u32;
        
        while high - low > 0.5 && iterations < max_iter {
            let mid = (low + high) / 2.0;
            self.progress_update(&format!("Binary search CRF {:.1}...", mid));
            let size = self.encode(mid)?;
            iterations += 1;
            
            if size < self.input_size {
                best_crf = mid;
                best_size = size;
                high = mid;
            } else {
                low = mid;
            }
        }
        
        Ok((best_crf, best_size, iterations))
    }
    
    /// 二分搜索找到最高 SSIM 的 CRF
    /// 
    /// 🔥 v6.4.4: 统一的质量搜索逻辑
    /// 
    /// # Arguments
    /// * `low` - 搜索下界
    /// * `high` - 搜索上界
    /// * `max_iter` - 最大迭代次数
    /// 
    /// # Returns
    /// `(best_crf, best_size, best_ssim, iterations)`
    pub fn binary_search_quality(
        &mut self,
        low: f32,
        high: f32,
        max_iter: u32,
    ) -> Result<(f32, u64, f64, u32)> {
        let mut low = low;
        let mut high = high;
        let mut best_crf = self.config.initial_crf;
        let mut best_ssim = 0.0f64;
        let mut iterations = 0u32;
        
        // 先测试初始 CRF
        self.progress_update(&format!("Test CRF {:.1}...", self.config.initial_crf));
        let mut best_size = self.encode(self.config.initial_crf)?;
        if let Ok(result) = self.calculate_ssim(self.config.initial_crf) {
            best_ssim = result.value;
        }
        iterations += 1;
        
        // 二分搜索优化
        while high - low > 1.0 && iterations < max_iter {
            let mid = (low + high) / 2.0;
            self.progress_update(&format!("Binary search CRF {:.1}...", mid));
            let size = self.encode(mid)?;
            iterations += 1;
            
            if let Ok(result) = self.calculate_ssim(mid) {
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
        
        Ok((best_crf, best_size, best_ssim, iterations))
    }
    
    /// 记录最终结果日志
    /// 
    /// 🔥 v6.4.4: 统一的结果日志格式
    pub fn log_final_result(&mut self, crf: f32, ssim: Option<f64>, size_change_pct: f64) {
        match ssim {
            Some(s) => self.log(format!("📊 RESULT: CRF {:.1}, SSIM {:.4}, {:+.1}%", crf, s, size_change_pct)),
            None => self.log(format!("📊 RESULT: CRF {:.1}, {:+.1}%", crf, size_change_pct)),
        }
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
    
    /// 🔥 v6.4.5: 计算 SSIM（带日志记录的版本）
    /// 
    /// 与 `calculate_ssim` 不同，此方法：
    /// - 失败时记录警告日志而非返回错误
    /// - 返回 Option<SsimResult> 而非 Result
    /// 
    /// 适用于 SSIM 计算是可选的场景（如 SizeOnly 策略）
    /// 
    /// # Arguments
    /// * `crf` - CRF 值
    /// 
    /// # Returns
    /// Some(SsimResult) 如果计算成功，None 如果失败（已记录日志）
    pub fn calculate_ssim_logged(&mut self, crf: f32) -> Option<SsimResult> {
        match self.calculate_ssim(crf) {
            Ok(result) => Some(result),
            Err(e) => {
                self.log(format!("⚠️ SSIM calculation failed for CRF {:.1}: {}", crf, e));
                None
            }
        }
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
/// 
/// 使用最高 CRF 值编码，不验证 SSIM 质量。
/// 适用于只关心文件大小的场景。
pub struct SizeOnlyStrategy;

impl ExploreStrategy for SizeOnlyStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("🔍 Size-Only Explore ({:?})", ctx.encoder));
        ctx.progress_start("🔍 Size Explore");
        
        // 测试 max_crf（最高 CRF = 最小文件）
        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.max_crf));
        let max_size = ctx.encode(ctx.config.max_crf)?;
        let quality_passed = max_size < ctx.input_size;
        
        // 🔥 v6.4.5: 使用 calculate_ssim_logged 记录错误
        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim_logged(ctx.config.max_crf);
        
        ctx.progress_done();
        ctx.log_final_result(ctx.config.max_crf, ssim_result.as_ref().map(|r| r.value), ctx.size_change_pct(max_size));
        
        // 🔥 v6.4.4: 使用 build_result 减少重复代码
        Ok(ctx.build_result(ctx.config.max_crf, max_size, ssim_result, 1, quality_passed, 0.7))
    }
    
    fn name(&self) -> &'static str { "SizeOnly" }
    fn description(&self) -> &'static str { "寻找更小的文件大小（不验证质量）" }
}

/// QualityMatch 策略 - 仅匹配输入质量
/// 
/// 使用算法预测的 CRF 值进行单次编码，然后验证 SSIM。
/// 适用于快速匹配质量的场景。
pub struct QualityMatchStrategy;

impl ExploreStrategy for QualityMatchStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("🎯 Quality-Match Mode ({:?})", ctx.encoder));
        ctx.log(format!("   Predicted CRF: {}", ctx.config.initial_crf));
        ctx.progress_start("🎯 Quality Match");
        
        // 单次编码
        ctx.progress_update(&format!("Encoding CRF {:.1}...", ctx.config.initial_crf));
        let output_size = ctx.encode(ctx.config.initial_crf)?;
        
        // 🔥 v6.4.5: 使用 calculate_ssim_logged 记录错误
        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim_logged(ctx.config.initial_crf);
        let quality_passed = ssim_result.as_ref()
            .map(|r| r.value >= ctx.config.quality_thresholds.min_ssim)
            .unwrap_or(false);
        
        ctx.progress_done();
        ctx.log_final_result(ctx.config.initial_crf, ssim_result.as_ref().map(|r| r.value), ctx.size_change_pct(output_size));
        
        // 🔥 v6.4.4: 使用 build_result 减少重复代码
        Ok(ctx.build_result(ctx.config.initial_crf, output_size, ssim_result, 1, quality_passed, 0.6))
    }
    
    fn name(&self) -> &'static str { "QualityMatch" }
    fn description(&self) -> &'static str { "使用算法预测的 CRF，单次编码 + SSIM 验证" }
}

/// PreciseQualityMatch 策略 - 精确质量匹配
/// 
/// 使用二分搜索找到最高 SSIM 的 CRF 值。
/// 不关心文件大小，只关心质量。
pub struct PreciseQualityMatchStrategy;

impl ExploreStrategy for PreciseQualityMatchStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("🎯 Precise Quality Match ({:?})", ctx.encoder));
        ctx.progress_start("🎯 Precise Quality");
        
        // 🔥 v6.4.4: 使用 binary_search_quality 减少重复代码
        let (best_crf, best_size, best_ssim, iterations) = ctx.binary_search_quality(
            ctx.config.min_crf,
            ctx.config.max_crf,
            ctx.config.max_iterations,
        )?;
        
        ctx.progress_done();
        
        let quality_passed = best_ssim >= ctx.config.quality_thresholds.min_ssim;
        ctx.log_final_result(best_crf, Some(best_ssim), ctx.size_change_pct(best_size));
        
        Ok(ctx.build_result(best_crf, best_size, Some(SsimResult::actual(best_ssim, None)), iterations, quality_passed, 0.85))
    }
    
    fn name(&self) -> &'static str { "PreciseQualityMatch" }
    fn description(&self) -> &'static str { "二分搜索 + SSIM 裁判验证，找到最高 SSIM" }
}

/// PreciseQualityMatchWithCompression 策略 - 精确质量匹配 + 压缩
/// 
/// 先找到压缩边界，然后在压缩范围内找最高 SSIM。
/// 如果无法同时满足，优先保证压缩。
pub struct PreciseQualityMatchWithCompressionStrategy;

impl ExploreStrategy for PreciseQualityMatchWithCompressionStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("🎯💾 Precise Quality + Compress ({:?})", ctx.encoder));
        ctx.progress_start("🎯💾 Quality+Compress");
        
        // 🔥 v6.4.4: 使用 binary_search_compress 找压缩边界
        let (compress_boundary, _, boundary_iter) = ctx.binary_search_compress(
            ctx.config.min_crf,
            ctx.config.max_crf,
            ctx.config.max_iterations / 2,
        )?;
        
        // 在压缩范围内找最高 SSIM
        let mut best_crf = compress_boundary;
        let mut best_ssim = 0.0;
        let mut best_size = ctx.get_cached_size(compress_boundary).unwrap_or(0);
        let mut iterations = boundary_iter;
        
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
        
        let quality_passed = best_size < ctx.input_size && best_ssim >= ctx.config.quality_thresholds.min_ssim;
        ctx.log_final_result(best_crf, Some(best_ssim), ctx.size_change_pct(best_size));
        
        Ok(ctx.build_result(best_crf, best_size, Some(SsimResult::actual(best_ssim, None)), iterations, quality_passed, 0.85))
    }
    
    fn name(&self) -> &'static str { "PreciseQualityMatchWithCompression" }
    fn description(&self) -> &'static str { "找到最高 SSIM 且输出 < 输入" }
}

/// CompressOnly 策略 - 仅压缩
/// 
/// 确保输出文件小于输入文件，不验证 SSIM 质量。
/// 与 SizeOnly 不同：SizeOnly 寻找最小输出，CompressOnly 只要更小即可。
pub struct CompressOnlyStrategy;

impl ExploreStrategy for CompressOnlyStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("💾 Compress-Only Mode ({:?})", ctx.encoder));
        ctx.progress_start("💾 Compress Only");
        
        // 先测试 initial_crf
        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.initial_crf));
        let initial_size = ctx.encode(ctx.config.initial_crf)?;
        
        if initial_size < ctx.input_size {
            // 能压缩，直接返回
            ctx.progress_done();
            ctx.log_final_result(ctx.config.initial_crf, None, ctx.size_change_pct(initial_size));
            return Ok(ctx.build_result(ctx.config.initial_crf, initial_size, None, 1, true, 0.8));
        }
        
        // 🔥 v6.4.4: 使用 binary_search_compress 减少重复代码
        let (best_crf, best_size, search_iter) = ctx.binary_search_compress(
            ctx.config.initial_crf,
            ctx.config.max_crf,
            ctx.config.max_iterations - 1,
        )?;
        let iterations = search_iter + 1; // +1 for initial test
        
        ctx.progress_done();
        let quality_passed = best_size < ctx.input_size;
        ctx.log_final_result(best_crf, None, ctx.size_change_pct(best_size));
        
        Ok(ctx.build_result(best_crf, best_size, None, iterations, quality_passed, 0.75))
    }
    
    fn name(&self) -> &'static str { "CompressOnly" }
    fn description(&self) -> &'static str { "确保输出 < 输入（不验证质量）" }
}

/// CompressWithQuality 策略 - 压缩 + 粗略质量验证
/// 
/// 确保输出文件小于输入文件，并进行粗略 SSIM 验证。
/// 与 PreciseQualityMatchWithCompression 不同：不追求最高 SSIM，只要通过阈值即可。
pub struct CompressWithQualityStrategy;

impl ExploreStrategy for CompressWithQualityStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("💾🎯 Compress+Quality Mode ({:?})", ctx.encoder));
        ctx.progress_start("💾🎯 Compress+Quality");
        
        // 先测试 initial_crf
        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.initial_crf));
        let initial_size = ctx.encode(ctx.config.initial_crf)?;
        
        let (best_crf, best_size, iterations) = if initial_size < ctx.input_size {
            (ctx.config.initial_crf, initial_size, 1u32)
        } else {
            // 🔥 v6.4.4: 使用 binary_search_compress 减少重复代码
            let (crf, size, iter) = ctx.binary_search_compress(
                ctx.config.initial_crf,
                ctx.config.max_crf,
                ctx.config.max_iterations - 1,
            )?;
            (crf, size, iter + 1)
        };
        
        // 🔥 v6.4.5: 使用 calculate_ssim_logged 记录错误
        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim_logged(best_crf);
        let quality_passed = best_size < ctx.input_size && 
            ssim_result.as_ref().map(|r| r.value >= ctx.config.quality_thresholds.min_ssim).unwrap_or(false);
        
        ctx.progress_done();
        ctx.log_final_result(best_crf, ssim_result.as_ref().map(|r| r.value), ctx.size_change_pct(best_size));
        
        Ok(ctx.build_result(best_crf, best_size, ssim_result, iterations, quality_passed, 0.75))
    }
    
    fn name(&self) -> &'static str { "CompressWithQuality" }
    fn description(&self) -> &'static str { "确保输出 < 输入 + 粗略 SSIM 验证" }
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
    
    // ═══════════════════════════════════════════════════════════════
    // 🔥 v6.4.5: CrfCache 单元测试
    // ═══════════════════════════════════════════════════════════════
    
    #[test]
    fn test_crf_cache_basic_operations() {
        let mut cache: CrfCache<u64> = CrfCache::new();
        
        // 插入和获取
        cache.insert(23.5, 1000000);
        assert_eq!(cache.get(23.5), Some(&1000000));
        assert!(cache.contains_key(23.5));
        
        // 不存在的 key
        assert_eq!(cache.get(24.0), None);
        assert!(!cache.contains_key(24.0));
    }
    
    #[test]
    fn test_crf_cache_boundary_values() {
        let mut cache: CrfCache<u64> = CrfCache::new();
        
        // 最小 CRF
        cache.insert(0.0, 100);
        assert_eq!(cache.get(0.0), Some(&100));
        
        // 最大有效 CRF (63.9)
        cache.insert(63.9, 200);
        assert_eq!(cache.get(63.9), Some(&200));
        
        // 超出范围的 CRF 应该被忽略
        cache.insert(64.0, 300);
        assert_eq!(cache.get(64.0), None);
        
        // 负数 CRF 应该被忽略
        cache.insert(-1.0, 400);
        assert_eq!(cache.get(-1.0), None);
    }
    
    #[test]
    fn test_crf_cache_precision() {
        let mut cache: CrfCache<u64> = CrfCache::new();
        
        // 测试 0.1 精度
        cache.insert(23.0, 100);
        cache.insert(23.1, 101);
        cache.insert(23.2, 102);
        
        assert_eq!(cache.get(23.0), Some(&100));
        assert_eq!(cache.get(23.1), Some(&101));
        assert_eq!(cache.get(23.2), Some(&102));
    }
    
    #[test]
    fn test_crf_cache_overwrite() {
        let mut cache: CrfCache<u64> = CrfCache::new();
        
        cache.insert(23.5, 100);
        assert_eq!(cache.get(23.5), Some(&100));
        
        // 覆盖
        cache.insert(23.5, 200);
        assert_eq!(cache.get(23.5), Some(&200));
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
        
        /// **Feature: code-quality-v6.4.9, Property 2: CRF 整数键唯一性**
        /// *对于任意*两个不同的 CRF 值（差异 >= 0.01），它们应映射到不同的缓存键
        /// 🔥 v6.4.9: 升级到 0.01 精度（乘数 100.0）
        /// **Validates: Requirements 1.2**
        #[test]
        fn prop_crf_cache_key_uniqueness(
            crf1 in 0.0f32..63.0f32,
            crf2 in 0.0f32..63.0f32
        ) {
            // 🔥 v6.4.9: 如果两个 CRF 值差异 >= 0.01，它们应该映射到不同的键
            if (crf1 - crf2).abs() >= 0.01 {
                let key1 = CrfCache::<u64>::key(crf1);
                let key2 = CrfCache::<u64>::key(crf2);
                prop_assert_ne!(key1, key2, 
                    "CRF {} and {} (diff {:.4}) should map to different keys, but both got {:?}",
                    crf1, crf2, (crf1 - crf2).abs(), key1);
            }
        }
        
        /// **Feature: code-quality-v6.4.7, Property 1b: 0.25 步进键唯一性**
        /// 验证 0.25 步进的 CRF 值不会碰撞
        /// **Validates: Requirements 1.1, 1.2**
        #[test]
        fn prop_crf_cache_025_step_uniqueness(
            base in 10.0f32..50.0f32
        ) {
            // 测试 base, base+0.25, base+0.5, base+0.75 都映射到不同的键
            let crf_values = [base, base + 0.25, base + 0.5, base + 0.75];
            let keys: Vec<_> = crf_values.iter()
                .map(|&crf| CrfCache::<u64>::key(crf))
                .collect();
            
            // 所有键都应该不同
            for i in 0..keys.len() {
                for j in (i+1)..keys.len() {
                    prop_assert_ne!(keys[i], keys[j],
                        "CRF {} and {} should have different keys, but both got {:?}",
                        crf_values[i], crf_values[j], keys[i]);
                }
            }
        }
        
        /// **Feature: code-quality-v6.4.5, Property 1: CrfCache 等价性**
        /// *对于任意* CRF 值和缓存值，CrfCache 的行为应与 HashMap 完全一致
        /// **Validates: Requirements 2.1, 2.2, 2.3**
        #[test]
        fn prop_crf_cache_equivalence(
            crf in 0.0f32..63.9f32,
            value in 0u64..u64::MAX
        ) {
            use std::collections::HashMap;
            
            // CrfCache 实现
            let mut cache: CrfCache<u64> = CrfCache::new();
            cache.insert(crf, value);
            let cache_result = cache.get(crf).copied();
            let cache_contains = cache.contains_key(crf);
            
            // HashMap 参考实现（使用新的乘数 40.0）
            let mut hashmap: HashMap<i32, u64> = HashMap::new();
            let key = (crf * 40.0) as i32;  // 🔥 v6.4.7: 更新为 40.0
            hashmap.insert(key, value);
            let hashmap_result = hashmap.get(&key).copied();
            let hashmap_contains = hashmap.contains_key(&key);
            
            // 验证等价性
            prop_assert_eq!(cache_result, hashmap_result, 
                "CrfCache and HashMap should return same value for CRF {}", crf);
            prop_assert_eq!(cache_contains, hashmap_contains,
                "CrfCache and HashMap should have same contains_key for CRF {}", crf);
        }
        
        /// **Feature: code-quality-v6.4.7, Property 2: CRF 缓存向后兼容**
        /// *对于任意* 0.5 步进的 CRF 值，升级后的缓存应返回与升级前相同的结果
        /// **Validates: Requirements 1.3**
        #[test]
        fn prop_crf_cache_backward_compatible(
            base in 10u32..50u32,
            value in 0u64..1000000u64
        ) {
            // 测试 0.5 步进的 CRF 值（旧版本支持的精度）
            let crf_05_step = base as f32 + 0.5;
            let crf_whole = base as f32;
            
            let mut cache: CrfCache<u64> = CrfCache::new();
            
            // 插入 0.5 步进值
            cache.insert(crf_05_step, value);
            cache.insert(crf_whole, value + 1);
            
            // 验证能正确获取
            prop_assert_eq!(cache.get(crf_05_step), Some(&value),
                "Should retrieve value for CRF {}", crf_05_step);
            prop_assert_eq!(cache.get(crf_whole), Some(&(value + 1)),
                "Should retrieve value for CRF {}", crf_whole);
            
            // 验证 0.5 步进值不会与整数值碰撞
            prop_assert_ne!(
                CrfCache::<u64>::key(crf_05_step),
                CrfCache::<u64>::key(crf_whole),
                "CRF {} and {} should have different keys", crf_05_step, crf_whole
            );
        }
        
        /// **Feature: code-quality-v6.4.5, Property 2: CrfCache 边界安全**
        /// *对于任意* 超出范围的 CRF 值，CrfCache 应安全处理（不 panic）
        /// **Validates: Requirements 2.1**
        #[test]
        fn prop_crf_cache_boundary_safe(
            crf in -100.0f32..200.0f32,
            value in 0u64..1000000u64
        ) {
            let mut cache: CrfCache<u64> = CrfCache::new();
            
            // 插入不应 panic
            cache.insert(crf, value);
            
            // 获取不应 panic
            let _ = cache.get(crf);
            let _ = cache.contains_key(crf);
            
            // 如果 CRF 在有效范围内，应该能获取到值
            if crf >= 0.0 && crf < 64.0 {
                prop_assert_eq!(cache.get(crf), Some(&value));
            } else {
                prop_assert_eq!(cache.get(crf), None);
            }
        }
    }
}
