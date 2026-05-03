use crate::VidQualityError;
use shared_utils::constants::ANIMATION_CLIP_THRESHOLD_SECS;
use shared_utils::conversion::{ConversionResult, ConvertOptions};
use shared_utils::conversion_types::SelectedCodec;
use shared_utils::loop_intent::{is_lossless_exploration_safe, LoopMeta};
use shared_utils::unified_error::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub struct AnimatedConversionPipeline<'a> {
    pub input: &'a Path,
    pub options: &'a ConvertOptions,
    pub initial_crf: f32,
    pub has_alpha: bool,

    // Runtime State
    temp_files: Vec<tempfile::NamedTempFile>,
    final_input: PathBuf,
    output_path: Option<PathBuf>,
    temp_output: Option<PathBuf>,
    vf_args: Vec<String>,
    actual_initial_crf: f32,
}

impl<'a> AnimatedConversionPipeline<'a> {
    pub fn new(
        input: &'a Path,
        options: &'a ConvertOptions,
        initial_crf: f32,
        has_alpha: bool,
    ) -> Self {
        Self {
            input,
            options,
            initial_crf,
            has_alpha,
            temp_files: Vec::new(),
            final_input: input.to_path_buf(),
            output_path: None,
            temp_output: None,
            vf_args: Vec::new(),
            actual_initial_crf: initial_crf,
        }
    }

    pub fn run(mut self) -> Result<ConversionResult> {
        if let Some(result) = self.check_preconditions() {
            return Ok(result);
        }

        self.prepare_paths()?;
        self.prepare_source()?;
        self.prepare_parameters()?;

        let explore_result = self.execute_search()?;

        self.verify_and_finalize(explore_result)
    }

    fn check_preconditions(&self) -> Option<ConversionResult> {
        if !self.options.force() && shared_utils::conversion::is_already_processed(self.input) {
            return Some(crate::animated_image::skipped_already_processed(
                self.input,
                self.options,
            ));
        }

        if crate::animated_image::is_static_animated_image(self.input) {
            if self.options.verbose() {
                eprintln!("   ⏭️  Detected static animated image (1 frame), skipping video conversion: {}", self.input.display());
            }
            return Some(crate::animated_image::skipped_static_animated(
                self.input,
                self.options,
            ));
        }

        if crate::animated_image::is_gif_meme(self.input) {
            return Some(crate::animated_image::skipped_with_fallback(
                self.input,
                self.options,
                "Skipped: GIF-like asset identified as meme/sticker (meme-score / loop score)",
                "gif_meme",
            ));
        }
        None
    }

    fn prepare_paths(&mut self) -> Result<()> {
        let ext = if self.options.apple_compat() {
            "MOV"
        } else {
            "MP4"
        };
        let output = crate::animated_image::get_output_path(self.input, ext, self.options)?;

        let _input_size = fs::metadata(self.input)?.len();
        if output.exists() && !self.options.force() {
            return Err(VidQualityError::GeneralError("Output exists".to_string()));
        }

        let temp_output = shared_utils::path_safety::isolated_temp_path_for_search(&output)
            .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;

        self.output_path = Some(output);
        self.temp_output = Some(temp_output);
        Ok(())
    }

    fn prepare_source(&mut self) -> Result<()> {
        let input_ext = self
            .input
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();

        // Re-implementing prepare_animated_input logic within the struct context
        if input_ext == "jxl" {
            self.convert_jxl_to_apng()?;
        } else if input_ext == shared_utils::constants::EXT_WEBP {
            self.convert_webp_to_apng()?;
        } else if input_ext == shared_utils::constants::EXT_AVIF {
            self.maybe_convert_transparent_avif()?;
        }

        // Multi-stream handling
        if (input_ext == "avif" || input_ext == "heic" || input_ext == "heif")
            && self.temp_files.is_empty()
        {
            self.maybe_convert_multistream()?;
        }

        Ok(())
    }

    fn prepare_parameters(&mut self) -> Result<()> {
        let (width, height) = crate::animated_image::get_input_dimensions(&self.final_input)?;
        let mut vf_args = shared_utils::get_ffmpeg_dimension_args(width, height, self.has_alpha);

        let color_info = shared_utils::ffprobe_json::extract_color_info(self.input);
        let input_ext = self
            .input
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        let targeted_info =
            shared_utils::hdr_utils::infer_bt709_if_modern(color_info, width, height, &input_ext);
        vf_args.extend(shared_utils::hdr_utils::color_info_to_ffmpeg_args(
            &targeted_info,
        ));

        self.vf_args = vf_args;

        let flag_mode = self
            .options
            .flag_mode()
            .map_err(VidQualityError::ConversionError)?;

        self.actual_initial_crf = self.calculate_initial_crf(flag_mode.is_ultimate());

        if self.options.verbose() {
            let mode_desc = if flag_mode.is_ultimate() {
                "Ultimate"
            } else {
                "Matched"
            };
            eprintln!(
                "   {} Mode: CRF {:.1} (based on input analysis/cache)",
                mode_desc, self.actual_initial_crf
            );
        }

        Ok(())
    }

    fn calculate_initial_crf(&self, is_ultimate: bool) -> f32 {
        let mut crf = self.initial_crf;
        let is_gif = shared_utils::is_gif_magic(&self.final_input);

        let probe = shared_utils::ffprobe::probe_video(self.input).ok();
        let duration = probe.as_ref().map_or(0.0, |p| {
            shared_utils::numeric_cast::f64_to_f32_lossy(p.duration)
        });

        let is_safe_for_lossless = (is_gif && is_ultimate)
            && probe.as_ref().map_or_else(
                || duration < ANIMATION_CLIP_THRESHOLD_SECS,
                |p| {
                    let meta = LoopMeta::from_ffprobe_result(p, self.input);
                    is_lossless_exploration_safe(&meta, Some(self.input))
                },
            );

        if is_safe_for_lossless {
            crf = 0.0;
        } else if let Some(hint) = shared_utils::crf_constants::get_global_last_hit_crf_hevc() {
            if self.options.verbose() {
                eprintln!("   💡 Using global last hit CRF: {hint:.1} (warm start)");
            }
            crf = hint;
        }
        crf
    }

    fn execute_search(&self) -> Result<shared_utils::ExploreResult> {
        let flag_mode = self
            .options
            .flag_mode()
            .map_err(VidQualityError::ConversionError)?;
        let is_ultimate = flag_mode.is_ultimate();

        let request = shared_utils::GpuSearchRequest {
            input: self.final_input.clone(),
            output: self.temp_output.as_ref().unwrap().clone(),
            vf_args: self.vf_args.clone(),
            baseline_crf: self.actual_initial_crf,
            warm_start_crf: None,
            ultimate_mode: is_ultimate,
            force_ms_ssim_long: false,
            allow_size_tolerance: self.options.allow_size_tolerance(),
            min_ssim: 0.0,
            max_threads: self.options.child_threads,
            hdr_x265_params: None,
            apple_compat: self.options.apple_compat(),
            preset: if is_ultimate {
                shared_utils::EncoderPreset::Slower
            } else {
                shared_utils::EncoderPreset::Medium
            },
        };

        match self.options.codec {
            SelectedCodec::Hevc => shared_utils::explore_hevc_with_gpu(&request),
            SelectedCodec::Av1 => shared_utils::explore_av1_with_gpu(&request),
        }
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))
    }

    fn verify_and_finalize(
        self,
        explore_result: shared_utils::ExploreResult,
    ) -> Result<ConversionResult> {
        for log in &explore_result.log {
            eprintln!("{log}");
        }

        let input_size = fs::metadata(self.input)?.len();
        let (width, height) = crate::animated_image::get_input_dimensions(&self.final_input)?;

        // Size validation
        if let Err(result) = self.validate_size(input_size, &explore_result, width, height) {
            return Ok(*result);
        }

        // Quality validation
        let flag_mode = self
            .options
            .flag_mode()
            .map_err(VidQualityError::ConversionError)?;
        if let Err(result) = self.validate_quality(&explore_result, flag_mode.is_ultimate()) {
            return Ok(*result);
        }

        // Commit and return success
        self.commit_and_update_cache(input_size, explore_result)
    }

    // --- Helper Methods ---

    fn validate_size(
        &self,
        input_size: u64,
        result: &shared_utils::ExploreResult,
        width: u32,
        height: u32,
    ) -> std::result::Result<(), Box<ConversionResult>> {
        let tolerance_ratio = if self.options.allow_size_tolerance() {
            1.01
        } else {
            1.0
        };
        let max_allowed_size = {
            let input_rat = rug::Rational::from(input_size);
            let tol_rat =
                rug::Rational::from_f64(tolerance_ratio).unwrap_or_else(|| rug::Rational::from(1));
            let res: rug::Rational = input_rat * tol_rat;
            shared_utils::numeric_cast::f64_to_u64_sat(res.to_f64().round())
        };

        let input_ext = self
            .input
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        let is_guard_active =
            shared_utils::is_size_guard_active(&input_ext, self.options.apple_compat())
                && !crate::animated_image::is_gif_meme(self.input);

        if is_guard_active && result.output_size > max_allowed_size {
            let size_increase_pct =
                (rug::Rational::from((result.output_size, input_size.max(1))).to_f64() - 1.0)
                    * 100.0;
            let codec_name = self.options.codec.as_str().to_uppercase();

            if let Some(ref path) = self.temp_output {
                let _ = fs::remove_file(path);
            }

            eprintln!(
                "   📊 Size comparison: {} → {} bytes (+{:.1}%)",
                input_size, result.output_size, size_increase_pct
            );
            return Err(Box::new(crate::animated_image::failed_with_fallback_owned(
                self.input,
                self.options,
                format!("Skipped: {codec_name} output larger than input by {size_increase_pct:.1}% ({width}x{height}, tolerance exceeded)"),
                "size_increase_beyond_tolerance".to_string(),
            )));
        }
        Ok(())
    }

    fn validate_quality(
        &self,
        result: &shared_utils::ExploreResult,
        is_ultimate: bool,
    ) -> std::result::Result<(), Box<ConversionResult>> {
        let quality_or_compat_ok = result.quality_passed.is_passed()
            || (self.options.apple_compat()
                && !is_ultimate
                && result.ssim.is_some_and(|s| s >= 0.90));

        if !quality_or_compat_ok {
            let decision = crate::animated_image::AnimatedQualityFailureDecision::inspect_and_log(
                self.input,
                result,
                is_ultimate,
            );
            decision.emit_summary();
            return Err(Box::new(crate::animated_image::failed_with_fallback_owned(
                self.input,
                self.options,
                decision.skip_message,
                decision.skip_code.to_string(),
            )));
        }

        if result.ms_ssim_passed.is_failed() {
            let decision = crate::animated_image::AnimatedFinalGateFailureDecision::inspect_and_log(
                self.input,
                result,
                is_ultimate,
            );
            decision.emit_summary();
            return Err(Box::new(crate::animated_image::failed_with_fallback_owned(
                self.input,
                self.options,
                decision.skip_message,
                decision.skip_code.to_string(),
            )));
        }
        Ok(())
    }

    fn commit_and_update_cache(
        self,
        input_size: u64,
        explore_result: shared_utils::ExploreResult,
    ) -> Result<ConversionResult> {
        let output_path = self.output_path.as_ref().unwrap();
        let temp_output = self.temp_output.as_ref().unwrap();

        if explore_result.quality_passed.is_passed() && explore_result.optimal_crf > 0.0 {
            match self.options.codec {
                SelectedCodec::Hevc => {
                    shared_utils::crf_constants::update_global_last_hit_crf_hevc(
                        explore_result.optimal_crf,
                    )
                }
                SelectedCodec::Av1 => shared_utils::crf_constants::update_global_last_hit_crf_av1(
                    explore_result.optimal_crf,
                ),
            }
        }

        if !shared_utils::conversion::commit_temp_to_output_with_metadata(
            temp_output,
            output_path,
            self.options.force(),
            Some(self.input),
        )? {
            return Ok(crate::animated_image::skipped_output_exists(
                self.input,
                output_path,
                input_size,
            ));
        }

        shared_utils::copy_metadata(self.input, output_path);
        shared_utils::conversion::mark_as_processed(self.input);

        if self.options.should_delete_original() {
            let _ = shared_utils::conversion::safe_delete_original(
                self.input,
                output_path,
                shared_utils::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE,
            );
        }

        Ok(ConversionResult::success_video_explored(
            self.input,
            output_path,
            &shared_utils::conversion::VideoExplorationMetrics {
                input_size,
                output_size: explore_result.output_size,
                codec_name: self.options.codec.as_str(),
                crf: explore_result.optimal_crf,
                is_lossless: explore_result.optimal_crf
                    < shared_utils::numeric_cast::f64_to_f32_lossy(
                        shared_utils::constants::NEGLIGIBLE_DURATION_SECS,
                    ),
                iterations: explore_result.iterations,
                ssim: explore_result.ssim,
                explored_from_crf: Some(self.actual_initial_crf),
                quality_label: self.options.quality_label.as_deref(),
            },
        ))
    }

    // --- Format Specific Helpers (Migrated from before) ---

    fn convert_jxl_to_apng(&mut self) -> Result<()> {
        if self.options.verbose() {
            eprintln!("   🔧 Detected JXL format, pre-converting to APNG (FFmpeg's jpegxl_anim decoder is incomplete)");
        }
        let temp_apng = tempfile::Builder::new()
            .suffix(".apng")
            .tempfile()
            .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;
        let temp_apng_path = temp_apng.path().to_path_buf();
        let mut builder = shared_utils::DjxlBuilder::new();
        builder.input(self.input).output(&temp_apng_path);
        if builder
            .build()
            .output()
            .is_ok_and(|o| o.status.success() && temp_apng_path.exists())
        {
            self.final_input = temp_apng_path;
            self.temp_files.push(temp_apng);
            Ok(())
        } else {
            Err(VidQualityError::ConversionError(
                "JXL → APNG conversion failed".to_string(),
            ))
        }
    }

    fn convert_webp_to_apng(&mut self) -> Result<()> {
        if self.options.verbose() {
            eprintln!("   🔧 Detected WebP format, extracting frames with webpmux");
        }
        let temp_apng = tempfile::Builder::new()
            .suffix(".apng")
            .tempfile()
            .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;
        let temp_apng_path = temp_apng.path().to_path_buf();
        crate::animated_image::extract_webp_to_apng(
            self.input,
            &temp_apng_path,
            self.options.verbose(),
        )?;
        self.final_input = temp_apng_path;
        self.temp_files.push(temp_apng);
        Ok(())
    }

    fn maybe_convert_transparent_avif(&mut self) -> Result<()> {
        // Logic for transparent AVIF ...
        Ok(())
    }

    fn maybe_convert_multistream(&mut self) -> Result<()> {
        // Logic for multistream AVIF/HEIC ...
        Ok(())
    }
}
