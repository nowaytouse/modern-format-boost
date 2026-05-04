use crate::{ImgQualityError, Result};
use shared_utils::conversion::{ConversionResult, ConvertOptions};
use shared_utils::image_jpeg_analysis::is_jpeg_complete;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::trace;

pub struct JpegToJxlPipeline<'a> {
    pub input: &'a Path,
    pub options: &'a ConvertOptions,
    pub hdr_info: Option<&'a shared_utils::ColorInfo>,

    // Runtime Context
    input_size: u64,
    output_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
    max_threads: usize,
}

impl<'a> JpegToJxlPipeline<'a> {
    pub fn new(
        input: &'a Path,
        options: &'a ConvertOptions,
        hdr_info: Option<&'a shared_utils::ColorInfo>,
    ) -> Self {
        Self {
            input,
            options,
            hdr_info,
            input_size: 0,
            output_path: None,
            temp_path: None,
            max_threads: shared_utils::thread_manager::get_optimal_threads(),
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

        if !is_jpeg_complete(&std::fs::read(self.input)?) {
            return Err(ImgQualityError::ConversionError(
                "JPEG is truncated or missing EOI".to_string(),
            ));
        }

        self.input_size = fs::metadata(self.input)?.len();
        Ok(())
    }

    fn handle_early_skips(&self) -> Option<ConversionResult> {
        if !self.options.force() && shared_utils::conversion::is_already_processed(self.input) {
            return Some(ConversionResult::skipped_duplicate(self.input));
        }

        if shared_utils::image_jpeg_analysis::is_ultra_hdr_jpeg_file(self.input) {
            shared_utils::progress_mode::emit_stderr(&format!(
                "   🌈 UltraHDR detected: {} - skipping JXL encoding (tool limitation) and copying original",
                self.input.file_name().unwrap_or_default().to_string_lossy()
            ));
            crate::lossless_converter::copy_original_on_skip(self.input, self.options);
            crate::lossless_converter::mark_as_processed(self.input);
            return Some(ConversionResult::skipped_custom(
                self.input,
                self.input_size,
                "UltraHDR JPEG",
                "Skipped due to cjxl gainmap incompatibility",
            ));
        }

        trace!(
            "UltraHDR not detected for {}: performing standard JPEG transcoding",
            self.input.file_name().unwrap_or_default().to_string_lossy()
        );

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

        // Phase 1: Try cjxl directly
        let res = crate::lossless_converter::run_cjxl_jpeg_transcode(
            self.input,
            temp_path,
            self.options,
            self.max_threads,
            None,
            self.hdr_info,
        );
        let output_cmd =
            res.map_err(|e| ImgQualityError::ToolNotFound(format!("cjxl not found: {e}")))?;

        if output_cmd.status.success() {
            return Ok(ExecutionMetrics {
                method: "JPEG lossless",
            });
        }

        let stderr = String::from_utf8_lossy(&output_cmd.stderr);
        if crate::lossless_converter::is_jpeg_reconstruction_cjxl_error(&stderr) {
            self.handle_reconstruction_error(temp_path, &stderr)
        } else if stderr.contains("Error while decoding")
            || stderr.contains("Corrupt JPEG")
            || stderr.contains("Premature end")
        {
            self.handle_corruption_fallback(temp_path, &stderr)
        } else {
            self.handle_generic_fallback(temp_path, &stderr)
        }
    }

    fn handle_reconstruction_error(
        &self,
        temp_path: &Path,
        original_stderr: &str,
    ) -> Result<ExecutionMetrics> {
        // Strip tail and retry
        let (source_to_use, _guard) =
            match shared_utils::jxl_utils::strip_jpeg_tail_to_temp(self.input) {
                Ok(Some((cleaned, guard))) => {
                    if self.options.verbose() {
                        eprintln!("   🔧 Stripped JPEG tail; retrying with original cjxl flags");
                    }
                    (cleaned, Some(guard))
                }
                _ => (self.input.to_path_buf(), None),
            };

        let retry = crate::lossless_converter::run_cjxl_jpeg_transcode(
            &source_to_use,
            temp_path,
            self.options,
            self.max_threads,
            None,
            self.hdr_info,
        );
        if let Ok(out) = retry {
            if out.status.success() {
                let method = if source_to_use == self.input {
                    "JPEG lossless"
                } else {
                    "JPEG lossless (sanitized tail)"
                };
                return Ok(ExecutionMetrics { method });
            }
        }

        // Final retry with --allow_jpeg_reconstruction 0
        let retry_no_recon = crate::lossless_converter::run_cjxl_jpeg_transcode(
            &source_to_use,
            temp_path,
            self.options,
            self.max_threads,
            Some(0),
            self.hdr_info,
        );
        if let Ok(out) = retry_no_recon {
            if out.status.success() {
                return Ok(ExecutionMetrics {
                    method: "JPEG lossless (--allow_jpeg_reconstruction 0)",
                });
            }
        }

        Err(ImgQualityError::ConversionError(format!(
            "cjxl JPEG transcode failed: {original_stderr}"
        )))
    }

    fn handle_corruption_fallback(
        &self,
        temp_path: &Path,
        original_stderr: &str,
    ) -> Result<ExecutionMetrics> {
        match shared_utils::jxl_utils::try_imagemagick_fallback(
            self.input,
            temp_path,
            0.0,
            self.max_threads,
            self.options.apple_compat(),
            self.options.ultimate(),
        ) {
            Ok(()) => Ok(ExecutionMetrics {
                method: "JPEG (Sanitized) -> JXL",
            }),
            Err(e) => Err(ImgQualityError::ConversionError(format!(
                "Fallback failed after JPEG corruption: {e} (cjxl error: {original_stderr})"
            ))),
        }
    }

    fn handle_generic_fallback(
        &self,
        temp_path: &Path,
        original_stderr: &str,
    ) -> Result<ExecutionMetrics> {
        if self.options.verbose() {
            eprintln!("   🔄 JPEG transcode failed, trying ImageMagick pipeline...");
        }
        match shared_utils::jxl_utils::try_imagemagick_fallback(
            self.input,
            temp_path,
            0.0,
            self.max_threads,
            self.options.apple_compat(),
            self.options.ultimate(),
        ) {
            Ok(()) => Ok(ExecutionMetrics {
                method: "JPEG -> JXL (ImageMagick fallback)",
            }),
            Err(_) => Err(ImgQualityError::ConversionError(format!(
                "cjxl JPEG transcode failed: {original_stderr}"
            ))),
        }
    }

    fn finalize(&self, metrics: ExecutionMetrics) -> Result<ConversionResult> {
        let output_path = self.output_path.as_ref().unwrap();
        let temp_path = self.temp_path.as_ref().unwrap();

        crate::lossless_converter::commit_jpeg_to_jxl_success(
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
    pub method: &'static str,
}
