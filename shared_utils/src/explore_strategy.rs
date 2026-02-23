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
    EncoderPreset, ExploreConfig, ExploreMode, ExploreResult, SsimSource, VideoEncoder,
};


pub trait ExploreStrategy: Send + Sync {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult>;

    fn name(&self) -> &'static str;

    fn description(&self) -> &'static str;
}


#[derive(Debug, Clone)]
pub struct SsimResult {
    pub value: f64,
    pub source: SsimSource,
    pub psnr: Option<f64>,
}

impl SsimResult {
    pub fn actual(value: f64, psnr: Option<f64>) -> Self {
        Self {
            value,
            source: SsimSource::Actual,
            psnr,
        }
    }

    pub fn predicted(value: f64, psnr: f64) -> Self {
        Self {
            value,
            source: SsimSource::Predicted,
            psnr: Some(psnr),
        }
    }

    #[inline]
    pub fn is_actual(&self) -> bool {
        matches!(self.source, SsimSource::Actual)
    }

    #[inline]
    pub fn is_predicted(&self) -> bool {
        matches!(self.source, SsimSource::Predicted)
    }


    #[inline]
    pub fn value_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.value).ok()
    }

    #[inline]
    pub fn meets_threshold(&self, threshold: f64) -> bool {
        crate::float_compare::ssim_meets_threshold(self.value, threshold)
    }
}


#[deprecated(since = "8.5.0", note = "Use SsimResult directly")]
pub type SsimCalculationResult = SsimResult;

#[deprecated(since = "8.5.0", note = "Use SsimSource directly")]
pub type SsimDataSource = SsimSource;


#[derive(Debug, Clone)]
pub struct ProgressConfig {
    pub show_spinner: bool,
    pub show_percentage: bool,
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


use crate::crf_constants::{CRF_CACHE_KEY_MULTIPLIER, CRF_CACHE_MAX_VALID};

const CRF_CACHE_SIZE: usize = 6400;

const CRF_CACHE_MULTIPLIER: f32 = CRF_CACHE_KEY_MULTIPLIER;

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
    #[inline]
    pub fn new() -> Self {
        Self {
            data: Box::new(std::array::from_fn(|_| None)),
        }
    }

    #[inline]
    pub fn key(crf: f32) -> Option<usize> {
        if crf < 0.0 {
            eprintln!("⚠️ CRF_CACHE: Negative CRF {} rejected", crf);
            return None;
        }
        if crf.is_nan() || crf.is_infinite() {
            eprintln!("⚠️ CRF_CACHE: Invalid CRF (NaN/Inf) rejected");
            return None;
        }
        if crf > CRF_CACHE_MAX_VALID {
            eprintln!(
                "⚠️ CRF_CACHE: CRF {} exceeds max valid {} - rejected",
                crf, CRF_CACHE_MAX_VALID
            );
            return None;
        }
        let idx = (crf * CRF_CACHE_MULTIPLIER).round() as usize;
        if idx < CRF_CACHE_SIZE {
            Some(idx)
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self, crf: f32) -> Option<&T> {
        Self::key(crf).and_then(|idx| self.data[idx].as_ref())
    }

    #[inline]
    pub fn insert(&mut self, crf: f32, value: T) {
        if let Some(idx) = Self::key(crf) {
            self.data[idx] = Some(value);
        }
    }

    #[inline]
    pub fn contains_key(&self, crf: f32) -> bool {
        Self::key(crf)
            .map(|idx| self.data[idx].is_some())
            .unwrap_or(false)
    }
}

impl<T: Clone> CrfCache<T> {
    #[inline]
    pub fn get_cloned(&self, crf: f32) -> Option<T> {
        self.get(crf).cloned()
    }
}


pub struct ExploreContext {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub input_size: u64,
    pub encoder: VideoEncoder,
    pub vf_args: Vec<String>,
    pub max_threads: usize,
    pub use_gpu: bool,
    pub preset: EncoderPreset,
    pub config: ExploreConfig,

    size_cache: CrfCache<u64>,
    ssim_cache: CrfCache<SsimResult>,

    progress: Option<indicatif::ProgressBar>,

    pub log: Vec<String>,
}

impl ExploreContext {
    /// Construct context for strategy-based explore. Consider a builder if adding more optional params.
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

    pub fn log(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
    }

    #[inline]
    pub fn get_cached_size(&self, crf: f32) -> Option<u64> {
        self.size_cache.get(crf).copied()
    }

    #[inline]
    pub fn cache_size(&mut self, crf: f32, size: u64) {
        self.size_cache.insert(crf, size);
    }

    #[inline]
    pub fn get_cached_ssim(&self, crf: f32) -> Option<&SsimResult> {
        self.ssim_cache.get(crf)
    }

    #[inline]
    pub fn cache_ssim(&mut self, crf: f32, result: SsimResult) {
        self.ssim_cache.insert(crf, result);
    }


    pub fn progress_start(&mut self, name: &str) {
        let pb = crate::progress::create_professional_spinner(name);
        self.progress = Some(pb);
    }

    pub fn progress_update(&self, msg: &str) {
        if let Some(ref pb) = self.progress {
            pb.set_message(msg.to_string());
        }
    }

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

    pub fn progress_done(&mut self) {
        if let Some(pb) = self.progress.take() {
            pb.finish_and_clear();
        }
    }

    #[inline]
    pub fn size_change_pct(&self, output_size: u64) -> f64 {
        if self.input_size == 0 {
            return 0.0;
        }
        ((output_size as f64 / self.input_size as f64) - 1.0) * 100.0
    }

    #[inline]
    pub fn can_compress(&self, output_size: u64) -> bool {
        output_size < self.input_size
    }


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
            ms_ssim: None,
            iterations,
            quality_passed,
            log: self.log.clone(),
            confidence,
            confidence_detail: ConfidenceBreakdown::default(), // not filled; confidence is the fixed value above
            actual_min_ssim: self.config.quality_thresholds.min_ssim,
            ..Default::default()
        }
    }

    /// Returns `Some(crf, size, iterations)` when at least one CRF compresses; `None` when none do (caller must handle).
    pub fn binary_search_compress(
        &mut self,
        low: f32,
        high: f32,
        max_iter: u32,
    ) -> Result<Option<(f32, u64, u32)>> {
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

        if best_size == u64::MAX {
            Ok(None)
        } else {
            Ok(Some((best_crf, best_size, iterations)))
        }
    }

    /// Binary search for the highest CRF that still meets min_ssim (best compression while meeting quality).
    pub fn binary_search_quality(
        &mut self,
        low: f32,
        high: f32,
        max_iter: u32,
    ) -> Result<(f32, u64, f64, u32)> {
        let min_ssim = self.config.quality_thresholds.min_ssim;
        let mut low = low;
        let mut high = high;
        let mut best_crf = self.config.initial_crf;
        let mut best_ssim = 0.0f64;
        let mut best_size = self.encode(self.config.initial_crf)?;
        let mut iterations = 0u32;

        self.progress_update(&format!("Test CRF {:.1}...", self.config.initial_crf));
        if let Ok(result) = self.calculate_ssim(self.config.initial_crf) {
            if result.value >= min_ssim {
                best_ssim = result.value;
            }
        }
        iterations += 1;

        while high - low > 1.0 && iterations < max_iter {
            let mid = (low + high) / 2.0;
            self.progress_update(&format!("Binary search CRF {:.1}...", mid));
            let size = self.encode(mid)?;
            iterations += 1;

            if let Ok(result) = self.calculate_ssim(mid) {
                if result.value >= min_ssim {
                    low = mid;
                    if mid > best_crf {
                        best_crf = mid;
                        best_ssim = result.value;
                        best_size = size;
                    }
                } else {
                    high = mid;
                }
            } else {
                high = mid;
            }
        }

        Ok((best_crf, best_size, best_ssim, iterations))
    }

    pub fn log_final_result(&mut self, crf: f32, ssim: Option<f64>, size_change_pct: f64) {
        match ssim {
            Some(s) => self.log(format!(
                "📊 RESULT: CRF {:.1}, SSIM {:.4}, {:+.1}%",
                crf, s, size_change_pct
            )),
            None => self.log(format!(
                "📊 RESULT: CRF {:.1}, {:+.1}%",
                crf, size_change_pct
            )),
        }
    }


    pub fn encode(&mut self, crf: f32) -> Result<u64> {
        if let Some(size) = self.get_cached_size(crf) {
            return Ok(size);
        }

        let size = self.do_encode(crf)?;
        self.cache_size(crf, size);
        Ok(size)
    }

    fn do_encode(&self, crf: f32) -> Result<u64> {
        use anyhow::{bail, Context};
        use std::fs;
        use std::process::Command;

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-threads")
            .arg(self.max_threads.to_string())
            .arg("-i")
            .arg(crate::safe_path_arg(&self.input_path).as_ref())
            .arg("-c:v")
            .arg(self.encoder.ffmpeg_name())
            .arg("-crf")
            .arg(format!("{:.1}", crf))
            .arg("-preset")
            .arg(self.preset.x26x_name());

        for arg in self.encoder.extra_args(self.max_threads) {
            cmd.arg(arg);
        }

        for arg in &self.vf_args {
            cmd.arg(arg);
        }

        cmd.arg(crate::safe_path_arg(&self.output_path).as_ref());

        let output = cmd.output().context("Failed to run ffmpeg")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "ffmpeg encoding failed: {}",
                stderr.lines().last().unwrap_or("unknown error")
            );
        }

        let size = fs::metadata(&self.output_path)
            .context("Failed to read output file")?
            .len();

        Ok(size)
    }

    pub fn calculate_ssim(&mut self, crf: f32) -> Result<SsimResult> {
        if let Some(result) = self.get_cached_ssim(crf) {
            return Ok(result.clone());
        }

        let result = self.do_calculate_ssim()?;
        self.cache_ssim(crf, result.clone());
        Ok(result)
    }

    pub fn calculate_ssim_logged(&mut self, crf: f32) -> Option<SsimResult> {
        match self.calculate_ssim(crf) {
            Ok(result) => Some(result),
            Err(e) => {
                self.log(format!(
                    "⚠️ SSIM calculation failed for CRF {:.1}: {}",
                    crf, e
                ));
                None
            }
        }
    }

    /// SSIM is computed from current input_path vs output_path on disk. Cache key is CRF; value is valid only if output was produced by encode(crf) and not overwritten. Call calculate_ssim immediately after encode when using the same output path.
    fn do_calculate_ssim(&self) -> Result<SsimResult> {
        use std::process::Command;

        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim";

        let output = Command::new("ffmpeg")
            .arg("-i")
            .arg(crate::safe_path_arg(&self.input_path).as_ref())
            .arg("-i")
            .arg(crate::safe_path_arg(&self.output_path).as_ref())
            .arg("-lavfi")
            .arg(filter)
            .arg("-f")
            .arg("null")
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

        eprintln!("   ⚠️ SSIM calculation failed, trying PSNR fallback...");

        if let Some(psnr) = self.calculate_psnr()? {
            let ssim = crate::ssim_mapping::psnr_to_ssim_estimate(psnr);
            eprintln!("   📊 PSNR: {:.1} dB → Estimated SSIM: {:.4}", psnr, ssim);
            return Ok(SsimResult::predicted(ssim, psnr));
        }

        eprintln!("   ⚠️ Both SSIM and PSNR measurement failed");
        Err(anyhow::anyhow!(
            "Both SSIM and PSNR calculation failed for {}",
            self.output_path.display()
        ))
    }

    fn parse_ssim(stderr: &str) -> Option<f64> {
        for line in stderr.lines() {
            if let Some(pos) = line.find("All:") {
                let value_str = &line[pos + 4..];
                let value_str = value_str.trim_start();
                let end = value_str
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(value_str.len());
                if end > 0 {
                    if let Ok(ssim) = value_str[..end].parse::<f64>() {
                        if (0.0..=1.0).contains(&ssim) {
                            return Some(ssim);
                        }
                    }
                }
            }
        }
        None
    }

    fn calculate_psnr(&self) -> Result<Option<f64>> {
        use std::process::Command;

        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]psnr";

        let output = Command::new("ffmpeg")
            .arg("-i")
            .arg(crate::safe_path_arg(&self.input_path).as_ref())
            .arg("-i")
            .arg(crate::safe_path_arg(&self.output_path).as_ref())
            .arg("-lavfi")
            .arg(filter)
            .arg("-f")
            .arg("null")
            .arg("-")
            .output();

        if let Ok(out) = output {
            let stderr = String::from_utf8_lossy(&out.stderr);
            for line in stderr.lines() {
                if let Some(pos) = line.find("average:") {
                    let value_str = &line[pos + 8..];
                    let value_str = value_str.trim_start();
                    let end = value_str
                        .find(|c: char| !c.is_numeric() && c != '.' && c != '-')
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


pub fn create_strategy(mode: ExploreMode) -> Box<dyn ExploreStrategy> {
    match mode {
        ExploreMode::SizeOnly => Box::new(SizeOnlyStrategy),
        ExploreMode::QualityMatch => Box::new(QualityMatchStrategy),
        ExploreMode::PreciseQualityMatch => Box::new(PreciseQualityMatchStrategy),
        ExploreMode::PreciseQualityMatchWithCompression => {
            Box::new(PreciseQualityMatchWithCompressionStrategy)
        }
        ExploreMode::CompressOnly => Box::new(CompressOnlyStrategy),
        ExploreMode::CompressWithQuality => Box::new(CompressWithQualityStrategy),
    }
}

pub struct SizeOnlyStrategy;

impl ExploreStrategy for SizeOnlyStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("🔍 Size-Only Explore ({:?})", ctx.encoder));
        ctx.progress_start("🔍 Size Explore");

        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.max_crf));
        let max_size = ctx.encode(ctx.config.max_crf)?;
        let quality_passed = max_size < ctx.input_size;

        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim_logged(ctx.config.max_crf);

        ctx.progress_done();
        ctx.log_final_result(
            ctx.config.max_crf,
            ssim_result.as_ref().map(|r| r.value),
            ctx.size_change_pct(max_size),
        );

        Ok(ctx.build_result(
            ctx.config.max_crf,
            max_size,
            ssim_result,
            1,
            quality_passed,
            0.7,
        ))
    }

    fn name(&self) -> &'static str {
        "SizeOnly"
    }
    fn description(&self) -> &'static str {
        "Minimize file size (no quality check)"
    }
}

pub struct QualityMatchStrategy;

impl ExploreStrategy for QualityMatchStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("🎯 Quality-Match Mode ({:?})", ctx.encoder));
        ctx.log(format!("   Predicted CRF: {}", ctx.config.initial_crf));
        ctx.progress_start("🎯 Quality Match");

        ctx.progress_update(&format!("Encoding CRF {:.1}...", ctx.config.initial_crf));
        let output_size = ctx.encode(ctx.config.initial_crf)?;

        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim_logged(ctx.config.initial_crf);
        let quality_passed = ssim_result
            .as_ref()
            .map(|r| r.value >= ctx.config.quality_thresholds.min_ssim)
            .unwrap_or(false);

        ctx.progress_done();
        ctx.log_final_result(
            ctx.config.initial_crf,
            ssim_result.as_ref().map(|r| r.value),
            ctx.size_change_pct(output_size),
        );

        Ok(ctx.build_result(
            ctx.config.initial_crf,
            output_size,
            ssim_result,
            1,
            quality_passed,
            0.6,
        ))
    }

    fn name(&self) -> &'static str {
        "QualityMatch"
    }
    fn description(&self) -> &'static str {
        "Single encode at predicted CRF + SSIM check"
    }
}

pub struct PreciseQualityMatchStrategy;

impl ExploreStrategy for PreciseQualityMatchStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("🎯 Precise Quality Match ({:?})", ctx.encoder));
        ctx.progress_start("🎯 Precise Quality");

        let (best_crf, best_size, best_ssim, iterations) = ctx.binary_search_quality(
            ctx.config.min_crf,
            ctx.config.max_crf,
            ctx.config.max_iterations,
        )?;

        ctx.progress_done();

        let quality_passed = best_ssim >= ctx.config.quality_thresholds.min_ssim;
        ctx.log_final_result(best_crf, Some(best_ssim), ctx.size_change_pct(best_size));

        Ok(ctx.build_result(
            best_crf,
            best_size,
            Some(SsimResult::actual(best_ssim, None)),
            iterations,
            quality_passed,
            0.85,
        ))
    }

    fn name(&self) -> &'static str {
        "PreciseQualityMatch"
    }
    fn description(&self) -> &'static str {
        "Binary search for max CRF meeting min SSIM"
    }
}

pub struct PreciseQualityMatchWithCompressionStrategy;

impl ExploreStrategy for PreciseQualityMatchWithCompressionStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!(
            "🎯💾 Precise Quality + Compress ({:?})",
            ctx.encoder
        ));
        ctx.progress_start("🎯💾 Quality+Compress");

        let (compress_boundary, boundary_size, boundary_iter) = match ctx.binary_search_compress(
            ctx.config.min_crf,
            ctx.config.max_crf,
            ctx.config.max_iterations / 2,
        )? {
            Some((crf, size, iter)) => (crf, size, iter),
            None => {
                ctx.progress_done();
                let size = ctx.encode(ctx.config.max_crf)?;
                return Ok(ctx.build_result(
                    ctx.config.max_crf,
                    size,
                    None,
                    ctx.config.max_iterations / 2 + 1,
                    false,
                    0.85,
                ));
            }
        };

        let mut best_crf = compress_boundary;
        let mut best_ssim = 0.0;
        let mut best_size = boundary_size;
        let mut iterations = boundary_iter;

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
                break;
            }
            crf -= 1.0;
        }

        ctx.progress_done();

        let quality_passed =
            best_size < ctx.input_size && best_ssim >= ctx.config.quality_thresholds.min_ssim;
        ctx.log_final_result(best_crf, Some(best_ssim), ctx.size_change_pct(best_size));

        Ok(ctx.build_result(
            best_crf,
            best_size,
            Some(SsimResult::actual(best_ssim, None)),
            iterations,
            quality_passed,
            0.85,
        ))
    }

    fn name(&self) -> &'static str {
        "PreciseQualityMatchWithCompression"
    }
    fn description(&self) -> &'static str {
        "Max SSIM with output smaller than input"
    }
}

pub struct CompressOnlyStrategy;

impl ExploreStrategy for CompressOnlyStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("💾 Compress-Only Mode ({:?})", ctx.encoder));
        ctx.progress_start("💾 Compress Only");

        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.initial_crf));
        let initial_size = ctx.encode(ctx.config.initial_crf)?;

        if initial_size < ctx.input_size {
            ctx.progress_done();
            ctx.log_final_result(
                ctx.config.initial_crf,
                None,
                ctx.size_change_pct(initial_size),
            );
            return Ok(ctx.build_result(ctx.config.initial_crf, initial_size, None, 1, true, 0.8));
        }

        let (best_crf, best_size, iterations) = match ctx.binary_search_compress(
            ctx.config.initial_crf,
            ctx.config.max_crf,
            ctx.config.max_iterations - 1,
        )? {
            Some((crf, size, search_iter)) => (crf, size, search_iter + 1),
            None => {
                let size = ctx.encode(ctx.config.max_crf)?;
                ctx.progress_done();
                ctx.log_final_result(ctx.config.max_crf, None, ctx.size_change_pct(size));
                return Ok(ctx.build_result(
                    ctx.config.max_crf,
                    size,
                    None,
                    ctx.config.max_iterations,
                    false,
                    0.75,
                ));
            }
        };

        ctx.progress_done();
        let quality_passed = best_size < ctx.input_size;
        ctx.log_final_result(best_crf, None, ctx.size_change_pct(best_size));

        Ok(ctx.build_result(best_crf, best_size, None, iterations, quality_passed, 0.75))
    }

    fn name(&self) -> &'static str {
        "CompressOnly"
    }
    fn description(&self) -> &'static str {
        "Ensure output < input (no quality check)"
    }
}

pub struct CompressWithQualityStrategy;

impl ExploreStrategy for CompressWithQualityStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("💾🎯 Compress+Quality Mode ({:?})", ctx.encoder));
        ctx.progress_start("💾🎯 Compress+Quality");

        ctx.progress_update(&format!("Test CRF {:.1}...", ctx.config.initial_crf));
        let initial_size = ctx.encode(ctx.config.initial_crf)?;

        let (best_crf, best_size, iterations) = if initial_size < ctx.input_size {
            (ctx.config.initial_crf, initial_size, 1u32)
        } else {
            match ctx.binary_search_compress(
                ctx.config.initial_crf,
                ctx.config.max_crf,
                ctx.config.max_iterations - 1,
            )? {
                Some((crf, size, iter)) => (crf, size, iter + 1),
                None => {
                    let size = ctx.encode(ctx.config.max_crf)?;
                    (ctx.config.max_crf, size, ctx.config.max_iterations)
                }
            }
        };

        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim_logged(best_crf);
        let quality_passed = best_size < ctx.input_size
            && ssim_result
                .as_ref()
                .map(|r| r.value >= ctx.config.quality_thresholds.min_ssim)
                .unwrap_or(false);

        ctx.progress_done();
        ctx.log_final_result(
            best_crf,
            ssim_result.as_ref().map(|r| r.value),
            ctx.size_change_pct(best_size),
        );

        Ok(ctx.build_result(
            best_crf,
            best_size,
            ssim_result,
            iterations,
            quality_passed,
            0.75,
        ))
    }

    fn name(&self) -> &'static str {
        "CompressWithQuality"
    }
    fn description(&self) -> &'static str {
        "Output < input + coarse SSIM check"
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_name_consistency() {
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
            assert!(!strategy.name().is_empty(), "strategy.name() should not be empty for {:?}", mode);
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


    #[test]
    fn test_crf_cache_basic_operations() {
        let mut cache: CrfCache<u64> = CrfCache::new();

        cache.insert(23.5, 1000000);
        assert_eq!(cache.get(23.5), Some(&1000000));
        assert!(cache.contains_key(23.5));

        assert_eq!(cache.get(24.0), None);
        assert!(!cache.contains_key(24.0));
    }

    #[test]
    fn test_crf_cache_boundary_values() {
        let mut cache: CrfCache<u64> = CrfCache::new();

        cache.insert(0.0, 100);
        assert_eq!(cache.get(0.0), Some(&100));

        cache.insert(63.9, 200);
        assert_eq!(cache.get(63.9), Some(&200));

        cache.insert(64.0, 300);
        assert_eq!(cache.get(64.0), None);

        cache.insert(-1.0, 400);
        assert_eq!(cache.get(-1.0), None);
    }

    #[test]
    fn test_crf_cache_precision() {
        let mut cache: CrfCache<u64> = CrfCache::new();

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

        cache.insert(23.5, 200);
        assert_eq!(cache.get(23.5), Some(&200));
    }
}


#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

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
        #[test]
        fn prop_strategy_selection_consistency(mode in arb_explore_mode()) {
            let strategy = create_strategy(mode);
            prop_assert!(!strategy.name().is_empty(), "strategy.name() should not be empty for {:?}", mode);
        }

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

            let result = SsimResult::actual(ssim_value, Some(psnr_value));
            ctx.cache_ssim(crf, result.clone());

            let cached = ctx.get_cached_ssim(crf);
            prop_assert!(cached.is_some());
            let cached = cached.unwrap();
            prop_assert_eq!(cached.value, ssim_value);
            prop_assert_eq!(cached.psnr, Some(psnr_value));
        }

        #[test]
        fn prop_psnr_to_ssim_mapping_valid(psnr in 20.0f64..60.0f64) {
            let ssim = crate::ssim_mapping::psnr_to_ssim_estimate(psnr);
            prop_assert!((0.0..=1.0).contains(&ssim),
                "SSIM {} out of range for PSNR {}", ssim, psnr);
            let ssim_higher = crate::ssim_mapping::psnr_to_ssim_estimate(psnr + 5.0);
            prop_assert!(ssim_higher >= ssim,
                "Higher PSNR {} should produce higher SSIM", psnr + 5.0);
        }

        #[test]
        fn prop_strategy_has_valid_metadata(mode in arb_explore_mode()) {
            let strategy = create_strategy(mode);
            prop_assert!(!strategy.name().is_empty(),
                "Strategy name should not be empty for {:?}", mode);
            prop_assert!(!strategy.description().is_empty(),
                "Strategy description should not be empty for {:?}", mode);
            prop_assert!(strategy.name().is_ascii(),
                "Strategy name should be ASCII for {:?}", mode);
        }

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

            ctx.cache_size(crf, size);

            let cached = ctx.get_cached_size(crf);
            prop_assert_eq!(cached, Some(size));
        }

        #[test]
        fn prop_crf_cache_key_uniqueness(
            crf1 in 0.0f32..63.0f32,
            crf2 in 0.0f32..63.0f32
        ) {
            const CACHE_CRF_RESOLUTION: f32 = 1.0 / CRF_CACHE_MULTIPLIER;
            if (crf1 - crf2).abs() >= CACHE_CRF_RESOLUTION {
                let key1 = CrfCache::<u64>::key(crf1);
                let key2 = CrfCache::<u64>::key(crf2);
                prop_assert_ne!(key1, key2,
                    "CRF {} and {} (diff {:.4}) should map to different keys, but both got {:?}",
                    crf1, crf2, (crf1 - crf2).abs(), key1);
            }
        }

        #[test]
        fn prop_crf_cache_025_step_uniqueness(
            base in 10.0f32..50.0f32
        ) {
            let crf_values = [base, base + 0.25, base + 0.5, base + 0.75];
            let keys: Vec<_> = crf_values.iter()
                .map(|&crf| CrfCache::<u64>::key(crf))
                .collect();

            for i in 0..keys.len() {
                for j in (i+1)..keys.len() {
                    prop_assert_ne!(keys[i], keys[j],
                        "CRF {} and {} should have different keys, but both got {:?}",
                        crf_values[i], crf_values[j], keys[i]);
                }
            }
        }

        #[test]
        fn prop_crf_cache_equivalence(
            crf in 0.0f32..63.9f32,
            value in 0u64..u64::MAX
        ) {
            use std::collections::HashMap;

            let mut cache: CrfCache<u64> = CrfCache::new();
            cache.insert(crf, value);
            let cache_result = cache.get(crf).copied();
            let cache_contains = cache.contains_key(crf);

            let mut hashmap: HashMap<i32, u64> = HashMap::new();
            let key = (crf * CRF_CACHE_MULTIPLIER).round() as i32;
            hashmap.insert(key, value);
            let hashmap_result = hashmap.get(&key).copied();
            let hashmap_contains = hashmap.contains_key(&key);

            prop_assert_eq!(cache_result, hashmap_result,
                "CrfCache and HashMap should return same value for CRF {}", crf);
            prop_assert_eq!(cache_contains, hashmap_contains,
                "CrfCache and HashMap should have same contains_key for CRF {}", crf);
        }

        #[test]
        fn prop_crf_cache_backward_compatible(
            base in 10u32..50u32,
            value in 0u64..1000000u64
        ) {
            let crf_05_step = base as f32 + 0.5;
            let crf_whole = base as f32;

            let mut cache: CrfCache<u64> = CrfCache::new();

            cache.insert(crf_05_step, value);
            cache.insert(crf_whole, value + 1);

            prop_assert_eq!(cache.get(crf_05_step), Some(&value),
                "Should retrieve value for CRF {}", crf_05_step);
            prop_assert_eq!(cache.get(crf_whole), Some(&(value + 1)),
                "Should retrieve value for CRF {}", crf_whole);

            prop_assert_ne!(
                CrfCache::<u64>::key(crf_05_step),
                CrfCache::<u64>::key(crf_whole),
                "CRF {} and {} should have different keys", crf_05_step, crf_whole
            );
        }

        #[test]
        fn prop_crf_cache_boundary_safe(
            crf in -100.0f32..200.0f32,
            value in 0u64..1000000u64
        ) {
            let mut cache: CrfCache<u64> = CrfCache::new();

            cache.insert(crf, value);

            let _ = cache.get(crf);
            let _ = cache.contains_key(crf);

            if (0.0..64.0).contains(&crf) {
                prop_assert_eq!(cache.get(crf), Some(&value));
            } else {
                prop_assert_eq!(cache.get(crf), None);
            }
        }
    }
}
