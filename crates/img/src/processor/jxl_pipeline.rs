use crate::{ImgQualityError, Result};
use shared_utils::conversion::{ConversionResult, ConvertOptions};
use std::fs;
use std::path::{Path, PathBuf};

pub struct JxlConversionPipeline<'a> {
    pub input: &'a Path,
    pub options: &'a ConvertOptions,
    pub distance: f32,
    pub hdr_info: Option<&'a shared_utils::ColorInfo>,

    // Runtime Context
    label: String,
    input_size: u64,
    output_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
}

impl<'a> JxlConversionPipeline<'a> {
    pub fn new(
        input: &'a Path,
        options: &'a ConvertOptions,
        distance: f32,
        hdr_info: Option<&'a shared_utils::ColorInfo>,
    ) -> Self {
        let label = input
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        Self {
            input,
            options,
            distance,
            hdr_info,
            label,
            input_size: 0,
            output_path: None,
            temp_path: None,
        }
    }

    pub fn run(mut self) -> Result<ConversionResult> {
        self.initialize()?;

        if let Some(result) = self.handle_early_skips() {
            return Ok(result);
        }

        self.setup_workspace()?;

        let execute_result = self.execute()?;

        self.finalize(execute_result)
    }

    fn initialize(&mut self) -> Result<()> {
        if let Err(e) = shared_utils::conversion::validate_input_file(self.input) {
            return Err(ImgQualityError::ConversionError(e));
        }
        self.input_size = fs::metadata(self.input)?.len();
        Ok(())
    }

    fn handle_early_skips(&self) -> Option<ConversionResult> {
        if !self.options.force() && shared_utils::conversion::is_already_processed(self.input) {
            return Some(ConversionResult::skipped_duplicate(self.input));
        }

        if let Some(ext) = self.input.extension() {
            if ext.to_string_lossy().to_lowercase() == "png"
                && self.input_size < crate::constants::SMALL_PNG_THRESHOLD_BYTES
            {
                if self.options.verbose() {
                    eprintln!(
                        "   ⏭️  Skipped small PNG (< 500KB): {}",
                        self.input.display()
                    );
                }
                crate::lossless_converter::mark_as_processed(self.input);
                return Some(ConversionResult::skipped_custom(
                    self.input,
                    self.input_size,
                    "Skipped: Small PNG (< 500KB)",
                    "small_file",
                ));
            }
        }
        None
    }

    fn setup_workspace(&mut self) -> Result<()> {
        let output = crate::lossless_converter::get_output_path(self.input, "jxl", self.options)?;

        if output.exists() && !self.options.force() {
            return Err(ImgQualityError::ConversionError(
                "Output exists".to_string(),
            ));
        }

        let temp_path = shared_utils::path_safety::isolated_temp_path_for_search(&output)
            .map_err(|e| ImgQualityError::ConversionError(e.to_string()))?;

        self.output_path = Some(output);
        self.temp_path = Some(temp_path);
        Ok(())
    }

    fn execute(&self) -> Result<ExecutionMetrics> {
        let temp_path = self.temp_path.as_ref().unwrap();
        let (actual_input, _temp_file_guard) = crate::lossless_converter::prepare_input_for_cjxl(
            self.input,
            self.options,
            self.hdr_info,
        )?;

        // Extract ICC Profile
        let icc_temp = shared_utils::jxl_utils::extract_icc_profile(self.input);
        let icc_path = icc_temp.as_ref().map(|t| t.path());

        let max_threads = if self.options.child_threads > 0 {
            self.options.child_threads
        } else {
            shared_utils::thread_manager::get_optimal_threads()
        };

        let actual_dist =
            shared_utils::constants::jxl_distance_for_mode(self.distance, self.options.ultimate());
        let actual_eff = crate::lossless_converter::jxl_screening_effort(
            self.options.ultimate(),
            self.options.explore(),
        );

        match crate::lossless_converter::execute_jxl_encoding(
            self.input,
            &actual_input,
            temp_path,
            actual_dist,
            actual_eff,
            max_threads,
            self.options,
            self.hdr_info,
            icc_path,
        ) {
            Ok(_) => {
                let mut size = fs::metadata(temp_path)?.len();
                let mut method = "cjxl";

                if self.options.ultimate() && self.options.explore() {
                    if let Ok(Some(explore_result)) =
                        crate::lossless_converter::try_explore_ultimate_jxl_distance(
                            self.input,
                            &actual_input,
                            temp_path,
                            self.input_size,
                            size,
                            max_threads,
                            self.options,
                            icc_path,
                            self.hdr_info,
                        )
                    {
                        size = explore_result.output_size;
                        method = "cjxl-explored";
                    }
                }

                Ok(ExecutionMetrics {
                    output_size: size,
                    method,
                })
            }
            Err(e) => {
                if self.options.verbose() {
                    eprintln!("   ⚠️  cjxl failed for {}: {}", self.label, e);
                }
                Err(e)
            }
        }
    }

    fn finalize(&self, metrics: ExecutionMetrics) -> Result<ConversionResult> {
        let output_path = self.output_path.as_ref().unwrap();
        let temp_path = self.temp_path.as_ref().unwrap();

        crate::lossless_converter::finalize_fallback_jxl(
            self.input,
            temp_path,
            output_path,
            self.input_size,
            self.options,
            metrics.method,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionMetrics {
    pub output_size: u64,
    pub method: &'static str,
}
