//! 🔥 v6.3: Strategy Pattern for Video Explorer
//!
//! Refactors exploration modes into independent Strategy structs, unifying SSIM calculation and progress display interface.
//!
//! ## Design Goals
//! 1. Fully independent logic for each explore mode, better maintainability and testability
//! 2. Unified `ExploreContext` providing shared states and utility methods
//! 3. Unified SSIM calculation logic (with caching and fallbacks)
//! 4. Unified progress display interface
//!
//! ## 🔥 v6.4.4: Utility Methods Refactor
//! Added `build_result()`, `binary_search_compress()`, `log_final_result()`, etc.,
//! reducing boilerplate in 6 Strategy implementations by ~40%.
//!
//! ## Unified Selection Philosophy
//!
//! All candidate/finalist ranking across strategies uses consistent priorities
//! (see `candidate_comparator` for implementation):
//!
//! 1. **Gating**: Pass status (size gate, quality_passed, ms_ssim_passed)
//! 2. **Quality**: VMAF > CAMBI > PSNR_UV > MS-SSIM > SSIM > PSNR
//! 3. **Size**: Output file size (prefer smaller)
//! 4. **Parameter**: CRF (prefer lower/more aggressive as tiebreaker)
//! 5. **Preset**: Rank (prefer higher/slower)
//!
//! ## Usage Example
//! ```ignore
//! use shared_utils::explore_strategy::{create_strategy, ExploreContext};
//!
//! let strategy = create_strategy(ExploreMode::CompressOnly);
//! let mut ctx = ExploreContext::new(...);
//! let result = strategy.explore(&mut ctx)?;
//! ```

use anyhow::Result;
use std::path::PathBuf;

use crate::types::{CheckResult, EncoderPreset};
use crate::video_explorer::{ExploreConfig, ExploreMode, ExploreResult, SsimSource, VideoEncoder};

pub trait ExploreStrategy: Send + Sync {
    /// # Errors
    /// Returns error if exploration fails.
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
    #[must_use]
    pub const fn actual(value: f64, psnr: Option<f64>) -> Self {
        Self {
            value,
            source: SsimSource::Actual,
            psnr,
        }
    }

    #[must_use]
    pub const fn predicted(value: f64, psnr: f64) -> Self {
        Self {
            value,
            source: SsimSource::Predicted,
            psnr: Some(psnr),
        }
    }

    #[inline]
    #[must_use]
    pub const fn is_actual(&self) -> bool {
        matches!(self.source, SsimSource::Actual)
    }

    #[inline]
    #[must_use]
    pub const fn is_predicted(&self) -> bool {
        matches!(self.source, SsimSource::Predicted)
    }

    #[inline]
    #[must_use]
    pub fn value_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.value).ok()
    }

    #[inline]
    #[must_use]
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
use std::collections::HashMap;

const CRF_CACHE_MULTIPLIER: f32 = CRF_CACHE_KEY_MULTIPLIER;

#[derive(Clone)]
pub struct CrfCache<T> {
    data: HashMap<u32, T>,
}

impl<T> Default for CrfCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> CrfCache<T> {
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: HashMap::with_capacity(16),
        }
    }

    #[inline]
    #[must_use]
    pub fn key(crf: f32) -> Option<u32> {
        if crf < 0.0 {
            eprintln!("⚠️ CRF_CACHE: Negative CRF {crf} rejected");
            return None;
        }
        if crf.is_nan() || crf.is_infinite() {
            eprintln!("⚠️ CRF_CACHE: Invalid CRF (NaN/Inf) rejected");
            return None;
        }
        if crf > CRF_CACHE_MAX_VALID {
            eprintln!("⚠️ CRF_CACHE: CRF {crf} exceeds max valid {CRF_CACHE_MAX_VALID} - rejected");
            return None;
        }
        Some(crate::numeric_cast::f32_to_u32_sat(
            (crf * CRF_CACHE_MULTIPLIER).round(),
        ))
    }

    #[inline]
    #[must_use]
    pub fn get(&self, crf: f32) -> Option<&T> {
        Self::key(crf).and_then(|idx| self.data.get(&idx))
    }

    #[inline]
    pub fn insert(&mut self, crf: f32, value: T) {
        if let Some(idx) = Self::key(crf) {
            self.data.insert(idx, value);
        }
    }

    #[inline]
    #[must_use]
    pub fn contains_key(&self, crf: f32) -> bool {
        Self::key(crf).is_some_and(|idx| self.data.contains_key(&idx))
    }
}

impl<T: Clone> CrfCache<T> {
    #[inline]
    #[must_use]
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
    pub hdr_x265_params: Option<String>,
    pub apple_compat: bool,
    /// Source codec name (e.g. "prores", "h264"), probed once at construction time.
    /// Used to pick the x265 memory profile for archival codecs under the size threshold.
    source_codec_name: Option<String>,

    size_cache: CrfCache<u64>,
    ssim_cache: CrfCache<SsimResult>,

    progress: Option<indicatif::ProgressBar>,

    pub log: Vec<String>,
}

pub struct ExploreContextArgs {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub input_size: u64,
    pub encoder: VideoEncoder,
    pub vf_args: Vec<String>,
    pub max_threads: usize,
    pub use_gpu: bool,
    pub preset: EncoderPreset,
    pub config: ExploreConfig,
    pub hdr_x265_params: Option<String>,
    pub apple_compat: bool,
}

impl ExploreContext {
    /// Construct context for strategy-based explore. Consider a builder if adding more optional params.
    #[must_use]
    pub fn new(args: ExploreContextArgs) -> Self {
        let ExploreContextArgs {
            input_path,
            output_path,
            input_size,
            encoder,
            vf_args,
            max_threads,
            use_gpu,
            preset,
            config,
            hdr_x265_params,
            apple_compat,
        } = args;
        let source_codec_name = crate::ffprobe::probe_video(&input_path)
            .ok()
            .map(|probe| probe.video_codec);
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
            hdr_x265_params,
            apple_compat,
            source_codec_name,
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
    #[must_use]
    pub fn get_cached_size(&self, crf: f32) -> Option<u64> {
        self.size_cache.get(crf).copied()
    }

    #[inline]
    pub fn cache_size(&mut self, crf: f32, size: u64) {
        self.size_cache.insert(crf, size);
    }

    #[inline]
    #[must_use]
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
    #[must_use]
    pub fn size_change_pct(&self, output_size: u64) -> f64 {
        if self.input_size == 0 {
            return 0.0;
        }
        ((crate::numeric_cast::u64_to_f64(output_size)
            / crate::numeric_cast::u64_to_f64(self.input_size))
            - 1.0)
            * 100.0
    }

    #[inline]
    #[must_use]
    pub const fn can_compress(&self, output_size: u64) -> bool {
        output_size < self.input_size
    }

    #[must_use]
    pub fn build_result(
        &self,
        crf: f32,
        size: u64,
        ssim_result: Option<SsimResult>,
        iterations: u32,
        quality_passed: CheckResult,
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
    ///
    /// # Errors
    /// Returns error if encoding fails.
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
            let mid = f32::midpoint(low, high);
            self.progress_update(&format!("Binary search CRF {mid:.1}..."));
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

    /// Binary search for the highest CRF that still meets `min_ssim` (best compression while meeting quality).
    ///
    /// # Errors
    /// Returns error if encoding fails.
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
            let mid = f32::midpoint(low, high);
            self.progress_update(&format!("Binary search CRF {mid:.1}..."));
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
        let status = if size_change_pct < 0.1 { "✅" } else { "❌" };
        match ssim {
            Some(s) => self.log(format!(
                "📊 RESULT: {status} CRF {crf:.1}, SSIM {s:.4}, {size_change_pct:+.1}%"
            )),
            None => {
                self.log(format!(
                    "📊 RESULT: {status} CRF {crf:.1}, {size_change_pct:+.1}%"
                ));
            }
        }
    }

    /// # Errors
    /// Returns error if encoding fails.
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

        let mut builder = crate::ffmpeg_builder::FfmpegBuilder::new();
        builder
            .overwrite()
            .threads(self.max_threads)
            .input(&self.input_path)
            .codec_v(self.encoder.ffmpeg_name())
            .crf(crf)
            .preset(self.preset);

        for arg in self.encoder.extra_args_with_preset(
            self.max_threads,
            self.preset,
            self.hdr_x265_params.as_deref(),
            self.apple_compat,
            crate::x265_params::memory_profile_for_source(
                self.source_codec_name.as_deref(),
                self.input_size,
            ),
        ) {
            builder.arg(arg);
        }

        for arg in &self.vf_args {
            builder.arg(arg);
        }

        builder.output(&self.output_path);

        let output = builder.build().output().context("Failed to run ffmpeg")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail = crate::io_utils::tail_error_lines(&stderr, 5);
            bail!(
                "ffmpeg encoding failed: {}",
                if tail.is_empty() {
                    "unknown error"
                } else {
                    &tail
                }
            );
        }

        let size = fs::metadata(&self.output_path)
            .context("Failed to read output file")?
            .len();

        Ok(size)
    }

    /// # Errors
    /// Returns error if calculation fails.
    pub fn calculate_ssim(&mut self, crf: f32) -> Result<SsimResult> {
        if let Some(result) = self.get_cached_ssim(crf) {
            return Ok(result.clone());
        }

        let result = self.do_calculate_ssim()?;
        self.cache_ssim(crf, result.clone());
        Ok(result)
    }

    #[must_use]
    pub fn calculate_ssim_logged(&mut self, crf: f32) -> Option<SsimResult> {
        match self.calculate_ssim(crf) {
            Ok(result) => Some(result),
            Err(e) => {
                self.log(format!("⚠️ SSIM calculation failed for CRF {crf:.1}: {e}"));
                None
            }
        }
    }

    /// SSIM is computed from current `input_path` vs `output_path` on disk. Cache key is CRF; value is valid only if output was produced by encode(crf) and not overwritten. Call `calculate_ssim` immediately after encode when using the same output path.
    fn do_calculate_ssim(&self) -> Result<SsimResult> {
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim";

        let output = crate::ffmpeg_builder::FfmpegBuilder::new()
            .input(&self.input_path)
            .input(&self.output_path)
            .filter_lavfi(filter)
            .format("null")
            .output_pipe()
            .build()
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
            eprintln!("   📊 PSNR: {psnr:.1} dB → Estimated SSIM: {ssim:.4}");
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

    /// # Errors
    /// Returns error if calculation fails.
    pub fn calculate_psnr(&self) -> Result<Option<f64>> {
        let filter = "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]psnr";

        let output = crate::ffmpeg_builder::FfmpegBuilder::new()
            .input(&self.input_path)
            .input(&self.output_path)
            .filter_lavfi(filter)
            .format("null")
            .output_pipe()
            .build()
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

#[must_use]
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
            if quality_passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed("Total file size not compressed".into())
            },
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
            .is_some_and(|r| r.value >= ctx.config.quality_thresholds.min_ssim);

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
            if quality_passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed("SSIM below target".into())
            },
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
            if quality_passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed("No CRF meeting quality target found".into())
            },
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

        let Some((compress_boundary, boundary_size, boundary_iter)) = ctx.binary_search_compress(
            ctx.config.min_crf,
            ctx.config.max_crf,
            ctx.config.max_iterations / 2,
        )?
        else {
            ctx.progress_done();
            let size = ctx.encode(ctx.config.max_crf)?;
            return Ok(ctx.build_result(
                ctx.config.max_crf,
                size,
                None,
                ctx.config.max_iterations / 2 + 1,
                CheckResult::Failed("No compressing CRF found".into()),
                0.85,
            ));
        };

        let mut best_crf = compress_boundary;
        let mut best_ssim = 0.0;
        let mut best_size = boundary_size;
        let mut iterations = boundary_iter;

        let search_low = (compress_boundary - 5.0).max(ctx.config.min_crf);
        let mut crf = compress_boundary;

        while crf >= search_low && iterations < ctx.config.max_iterations {
            ctx.progress_update(&format!("Quality search CRF {crf:.1}..."));
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

        let quality_passed = best_ssim >= ctx.config.quality_thresholds.min_ssim;
        ctx.log_final_result(best_crf, Some(best_ssim), ctx.size_change_pct(best_size));

        Ok(ctx.build_result(
            best_crf,
            best_size,
            Some(SsimResult::actual(best_ssim, None)),
            iterations,
            if quality_passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed("No CRF meeting quality target found".into())
            },
            0.85,
        ))
    }

    fn name(&self) -> &'static str {
        "PreciseQualityMatchWithCompression"
    }
    fn description(&self) -> &'static str {
        "Binary search for compression then quality search"
    }
}

pub struct CompressOnlyStrategy;

impl ExploreStrategy for CompressOnlyStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("💾 Compress-Only Mode ({:?})", ctx.encoder));
        ctx.progress_start("💾 Compress Only");

        let (best_crf, best_size, iterations) = if let Some((crf, size, iter)) = ctx
            .binary_search_compress(
                ctx.config.min_crf,
                ctx.config.max_crf,
                ctx.config.max_iterations,
            )? {
            (crf, size, iter)
        } else {
            let size = ctx.encode(ctx.config.max_crf)?;
            (ctx.config.max_crf, size, 1)
        };

        ctx.progress_done();

        ctx.log_final_result(best_crf, None, ctx.size_change_pct(best_size));

        Ok(ctx.build_result(
            best_crf,
            best_size,
            None,
            iterations,
            if best_size < ctx.input_size {
                CheckResult::Passed
            } else {
                CheckResult::Failed("Not compressed".into())
            },
            0.7,
        ))
    }

    fn name(&self) -> &'static str {
        "CompressOnly"
    }
    fn description(&self) -> &'static str {
        "Maximize compression regardless of quality"
    }
}

pub struct CompressWithQualityStrategy;

impl ExploreStrategy for CompressWithQualityStrategy {
    fn explore(&self, ctx: &mut ExploreContext) -> Result<ExploreResult> {
        ctx.log(format!("💾🎭 Compress + Quality ({:?})", ctx.encoder));
        ctx.progress_start("💾🎭 Compress+Quality");

        let (best_crf, best_size, iterations) = if let Some((crf, size, search_iter)) = ctx
            .binary_search_compress(
                ctx.config.initial_crf,
                ctx.config.max_crf,
                ctx.config.max_iterations - 1,
            )? {
            (crf, size, search_iter + 1)
        } else {
            let size = ctx.encode(ctx.config.max_crf)?;
            ctx.progress_done();
            ctx.log_final_result(ctx.config.max_crf, None, ctx.size_change_pct(size));
            return Ok(ctx.build_result(
                ctx.config.max_crf,
                size,
                None,
                ctx.config.max_iterations,
                CheckResult::Failed("No compressing CRF found".into()),
                0.75,
            ));
        };

        ctx.progress_update("Calculate SSIM...");
        let ssim_result = ctx.calculate_ssim_logged(best_crf);
        let quality_passed = ssim_result
            .as_ref()
            .is_some_and(|r| r.value >= ctx.config.quality_thresholds.min_ssim);

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
            if quality_passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed("SSIM below target".into())
            },
            0.8,
        ))
    }

    fn name(&self) -> &'static str {
        "CompressWithQuality"
    }
    fn description(&self) -> &'static str {
        "Maximize compression with quality check"
    }
}
