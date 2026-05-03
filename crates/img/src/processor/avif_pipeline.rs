use crate::{ImgQualityError, Result};
use shared_utils::conversion::{ConversionResult, ConvertOptions};
use std::fs;
use std::path::{Path, PathBuf};

pub struct AvifConversionPipeline<'a> {
    pub input: &'a Path,
    pub options: &'a ConvertOptions,
    pub quality: Option<u8>,
    pub is_lossless: bool,

    // Runtime Context
    input_size: u64,
    output_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
}

impl<'a> AvifConversionPipeline<'a> {
    pub fn new(
        input: &'a Path,
        options: &'a ConvertOptions,
        quality: Option<u8>,
        is_lossless: bool,
    ) -> Self {
        Self {
            input,
            options,
            quality,
            is_lossless,
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
        None
    }

    fn setup_workspace(&mut self) -> Result<()> {
        let output = crate::lossless_converter::get_output_path(self.input, "avif", self.options)?;

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

        if self.is_lossless {
            crate::lossless_converter::execute_avif_lossless_encoding(
                self.input,
                temp_path,
                self.options,
            )?;
        } else {
            crate::lossless_converter::execute_avif_encoding(
                self.input,
                temp_path,
                self.quality,
                self.options,
            )?;
        }

        let size = fs::metadata(temp_path)?.len();
        Ok(ExecutionMetrics { output_size: size })
    }

    fn finalize(&self, metrics: ExecutionMetrics) -> Result<ConversionResult> {
        let output_path = self.output_path.as_ref().unwrap();
        let temp_path = self.temp_path.as_ref().unwrap();
        let format_label = if self.is_lossless {
            "AVIF Lossless"
        } else {
            "AVIF"
        };

        crate::lossless_converter::finalize_with_size_check(
            self.input,
            temp_path,
            output_path,
            self.input_size,
            metrics.output_size,
            self.options,
            format_label,
            None,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionMetrics {
    pub output_size: u64,
}
