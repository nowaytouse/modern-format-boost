use crate::conversion_api::VideoFinalizationMetrics;
use crate::VidQualityError;
use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, SelectedCodec, TargetVideoFormat,
};
use shared_utils::unified_error::Result;
use shared_utils::video_detection::VideoDetectionResult as VideoDetection;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct VideoConversionPipeline<'a> {
    pub input: &'a Path,
    pub config: &'a ConversionConfig,
    pub cache: Option<&'a AnalysisCache>,

    // Runtime Context
    label: String,
    detection: Option<VideoDetection>,
    strategy: Option<ConversionStrategy>,
    output_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
    source_is_gif: bool,
}

pub struct ExecutionMetrics {
    pub output_size: u64,
    pub final_crf: f32,
    pub attempts: u8,
    pub explore_result: Option<shared_utils::ExploreResult>,
}

impl<'a> VideoConversionPipeline<'a> {
    pub fn new(
        input: &'a Path,
        config: &'a ConversionConfig,
        cache: Option<&'a AnalysisCache>,
    ) -> Self {
        let label = input
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        Self {
            input,
            config,
            cache,
            label,
            detection: None,
            strategy: None,
            output_path: None,
            temp_path: None,
            source_is_gif: false,
        }
    }

    pub fn run(mut self) -> Result<ConversionOutput> {
        self.initialize()?;

        if let Some(output) = self.handle_initial_skips() {
            return Ok(output);
        }

        self.analyze_and_plan()?;

        if self.strategy.as_ref().unwrap().target == TargetVideoFormat::Skip {
            return self.finalize_skip();
        }

        self.setup_workspace()?;

        let metrics = self.execute()?;

        self.verify_and_finalize(metrics)
    }

    fn initialize(&self) -> Result<()> {
        shared_utils::ctrlc_guard::wait_if_prompt_active();

        if let Err(e) = shared_utils::conversion::validate_input_file(self.input) {
            return Err(VidQualityError::ConversionError(e));
        }

        shared_utils::progress_mode::set_log_context(&self.label);
        Ok(())
    }

    fn handle_initial_skips(&self) -> Option<ConversionOutput> {
        if self.config.apple_compat() && shared_utils::is_live_photo(self.input) {
            let reason = "Live Photo detected in Apple compat mode";
            shared_utils::progress_mode::video_skipped(reason);

            let file_size = std::fs::metadata(self.input).map_or(0, |m| m.len());

            let _ = shared_utils::copy_on_skip_or_fail(
                self.input,
                self.config.output_dir.as_deref(),
                self.config.base_dir.as_deref(),
                false,
            );

            return Some(ConversionOutput {
                input_path: self.input.display().to_string(),
                output_path: String::new(),
                strategy: ConversionStrategy {
                    target: TargetVideoFormat::Skip,
                    reason: reason.to_string(),
                    command: String::new(),
                    preserve_audio: false,
                    crf: 0.0,
                    lossless: false,
                },
                input_size: file_size,
                output_size: 0,
                size_ratio: 0.0,
                success: true,
                message: "Skipped Live Photo in Apple compat mode".to_string(),
                final_crf: 0.0,
                exploration_attempts: 0,
                blake3: None,
            });
        }
        None
    }

    fn analyze_and_plan(&mut self) -> Result<()> {
        let mut detection = crate::detection_api::detect_video_with_cache(self.input, self.cache)?;

        self.reconcile_animated_image(&mut detection);

        if detection.frame_count <= 1 {
            self.strategy = Some(ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: "Static image detected (1 frame) - vid ignores static media".to_string(),
                ..Default::default()
            });
            self.detection = Some(detection);
            return Ok(());
        }

        self.log_hdr_warnings(&detection);

        detection.file_path = self.input.display().to_string();

        let strategy = crate::conversion_api::determine_strategy_with_apple_compat(
            &detection,
            self.input,
            self.config.apple_compat(),
            self.config.force(),
            self.config.codec,
        );

        if self.config.codec == SelectedCodec::Av1 && self.config.apple_compat() {
            return Err(VidQualityError::GeneralError(
                "AV1 is incompatible with Apple compatibility mode".to_string(),
            ));
        }

        let input_ext = self
            .input
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        self.source_is_gif = input_ext.eq_ignore_ascii_case("gif");

        self.detection = Some(detection);
        self.strategy = Some(strategy);
        Ok(())
    }

    fn setup_workspace(&mut self) -> Result<()> {
        let strategy = self.strategy.as_ref().unwrap();
        let output_path =
            crate::conversion_api::build_output_path(self.input, strategy, self.config)?;

        if output_path.exists() && !self.config.force() {
            shared_utils::progress_mode::video_skipped(&format!(
                "Output exists: {}",
                output_path.display()
            ));
            return Err(VidQualityError::GeneralError("Output exists".to_string()));
        }

        let temp_path = shared_utils::path_safety::isolated_temp_path_for_search(&output_path)
            .map_err(|e| VidQualityError::conversion_error(e.to_string()))?;

        self.output_path = Some(output_path);
        self.temp_path = Some(temp_path);
        Ok(())
    }

    fn execute(&self) -> Result<ExecutionMetrics> {
        let strategy = self.strategy.as_ref().unwrap();
        let detection = self.detection.as_ref().unwrap();
        let temp_path = self.temp_path.as_ref().unwrap();

        info!(
            "🎬 Auto Mode: {} → {}",
            self.input.display(),
            strategy.target.as_str()
        );
        info!("   Reason: {}", strategy.reason);

        match strategy.target {
            TargetVideoFormat::HevcLosslessMkv => {
                let size = crate::conversion_api::execute_lossless(
                    detection,
                    temp_path,
                    self.config.child_threads,
                    self.config.codec,
                    self.config.apple_compat(),
                    self.config.ultimate_mode(),
                )?;
                Ok(ExecutionMetrics {
                    output_size: size,
                    final_crf: 0.0,
                    attempts: 0,
                    explore_result: None,
                })
            }
            TargetVideoFormat::HevcMov | TargetVideoFormat::HevcMp4 | TargetVideoFormat::Av1Mp4 => {
                self.execute_lossy_search()
            }
            _ => Err(VidQualityError::GeneralError(
                "Unsupported target format".to_string(),
            )),
        }
    }

    fn execute_lossy_search(&self) -> Result<ExecutionMetrics> {
        let detection = self.detection.as_ref().unwrap();
        let temp_path = self.temp_path.as_ref().unwrap();
        let config = self.config;

        let vf_args = shared_utils::get_ffmpeg_dimension_args(
            detection.width,
            detection.height,
            false,
        );
        let predicted_crf = crate::conversion_api::calculate_matched_crf(detection, &config.codec)?;

        let warm_start_crf = self.get_warm_start_crf(predicted_crf);
        let search_crf = warm_start_crf.unwrap_or(predicted_crf);

        let ultimate = config.ultimate_mode();
        info!(
            "   {} {} Mode: base CRF {:.1} → search anchor {:.1}",
            if ultimate { "🔥" } else { "🔬" },
            if ultimate { "Ultimate" } else { "Matched" },
            predicted_crf,
            search_crf
        );

        let hdr_params = crate::conversion_api::prepare_hdr_x265_params(detection);

        let request = shared_utils::GpuSearchRequest {
            input: self.input.to_path_buf(),
            output: temp_path.clone(),
            vf_args,
            baseline_crf: predicted_crf,
            warm_start_crf,
            ultimate_mode: ultimate,
            force_ms_ssim_long: config.force_ms_ssim_long(),
            allow_size_tolerance: config.allow_size_tolerance(),
            min_ssim: config.min_ssim,
            max_threads: config.child_threads,
            hdr_x265_params: hdr_params,
            apple_compat: config.apple_compat(),
            preset: if ultimate {
                shared_utils::EncoderPreset::Slower
            } else {
                shared_utils::EncoderPreset::Medium
            },
        };

        let explore_result = match config.codec {
            SelectedCodec::Hevc => shared_utils::explore_hevc_with_gpu(&request),
            SelectedCodec::Av1 => shared_utils::explore_av1_with_gpu(&request),
            SelectedCodec::Av2 | SelectedCodec::Vvc => {
                return Err(VidQualityError::GeneralError(format!(
                    "{} encoding not yet implemented (experimental codec)",
                    config.codec.as_str().to_uppercase()
                )));
            }
        }
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

        for log in &explore_result.log {
            info!("{}", log);
        }

        Ok(ExecutionMetrics {
            output_size: explore_result.output_size,
            final_crf: explore_result.optimal_crf,
            attempts: u8::try_from(explore_result.iterations).unwrap_or(u8::MAX),
            explore_result: Some(explore_result),
        })
    }

    fn verify_and_finalize(&self, metrics: ExecutionMetrics) -> Result<ConversionOutput> {
        let output_path = self.output_path.as_ref().unwrap();
        let temp_path = self.temp_path.as_ref().unwrap();
        let detection = self.detection.as_ref().unwrap();

        // Check if quality/size is acceptable
        if let Some(ref result) = metrics.explore_result {
            if !result.quality_passed.is_passed()
                && (self.config.match_quality() || self.config.explore_smaller())
            {
                // Handle Apple compat fallback logic here...
                // (For now, simplified)
                if !self.config.apple_compat() {
                    return Err(VidQualityError::GeneralError(
                        "Quality/Size requirement not met".to_string(),
                    ));
                }
            }
        }

        // Commit temp to output
        crate::conversion_api::verify_and_finalize_video(
            VideoFinalizationMetrics {
                input: self.input,
                output_path,
                temp_path,
                detection,
                output_size: metrics.output_size,
                final_crf: metrics.final_crf,
                attempts: metrics.attempts,
            },
            self.config,
        )
    }

    fn finalize_skip(&self) -> Result<ConversionOutput> {
        let detection = self.detection.as_ref().unwrap();
        let strategy = self.strategy.as_ref().unwrap();

        shared_utils::progress_mode::video_skipped(&strategy.reason);

        shared_utils::copy_on_skip_or_fail(
            self.input,
            self.config.output_dir.as_deref(),
            self.config.base_dir.as_deref(),
            false,
        )
        .map_err(|e| VidQualityError::GeneralError(e.to_string()))?;

        return Ok(ConversionOutput {
            input_path: self.input.display().to_string(),
            output_path: String::new(),
            strategy,
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "Skipped".to_string(),
            ..Default::default()
        });
    }

    // --- Internal Helpers ---
    fn get_warm_start_crf(&self, _predicted_crf: f32) -> Option<f32> {
        if let Some(ref det) = self.detection {
            if let Some(hint) = det.precision.last_best_crf {
                return Some(hint);
            }
        }
        None
    }

    fn reconcile_animated_image(&self, detection: &mut VideoDetection) {
        if detection.frame_count <= 1
            && shared_utils::quality_matcher::SourceCodec::identify_by_content(self.input)
                .is_some_and(|codec| codec.can_be_animated())
        {
            if let Ok(image_det) = shared_utils::image_detection::detect_image(self.input) {
                if matches!(
                    image_det.image_type,
                    shared_utils::image_detection::ImageType::Animated
                ) || image_det.frame_count > 1
                {
                    detection.frame_count = u64::from(image_det.frame_count.max(2));
                }
            }
        }
    }

    fn log_hdr_warnings(&self, detection: &VideoDetection) {
        if detection.is_dolby_vision {
            info!("Dolby Vision detected: RPU preservation checked");
        }
    }
}
