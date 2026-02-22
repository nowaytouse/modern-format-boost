use crate::batch::BatchResult;
use crate::file_copier::{
    copy_unsupported_files, verify_output_completeness, SUPPORTED_VIDEO_EXTENSIONS,
};
use crate::report::print_summary_report;
use anyhow::Result;
use log::{error, info, warn};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub trait CliProcessingResult {
    fn is_skipped(&self) -> bool;
    fn is_success(&self) -> bool;
    fn skip_reason(&self) -> Option<&str>;
    fn input_path(&self) -> &str;
    fn output_path(&self) -> Option<&str>;
    fn input_size(&self) -> u64;
    fn output_size(&self) -> Option<u64>;
    fn message(&self) -> &str;
}

impl CliProcessingResult for crate::conversion::ConversionResult {
    fn is_skipped(&self) -> bool {
        self.skipped
    }
    fn is_success(&self) -> bool {
        self.success && !self.skipped
    }
    fn skip_reason(&self) -> Option<&str> {
        self.skip_reason.as_deref()
    }
    fn input_path(&self) -> &str {
        &self.input_path
    }
    fn output_path(&self) -> Option<&str> {
        self.output_path.as_deref()
    }
    fn input_size(&self) -> u64 {
        self.input_size
    }
    fn output_size(&self) -> Option<u64> {
        self.output_size
    }
    fn message(&self) -> &str {
        &self.message
    }
}

pub struct CliRunnerConfig {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub recursive: bool,
    pub label: String,
    pub base_dir: Option<PathBuf>,
}

pub fn run_auto_command<F, R>(config: CliRunnerConfig, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R>,
    R: CliProcessingResult,
{
    if config.input.is_dir() {
        process_directory(&config, converter)
    } else {
        process_single_file(&config, converter)
    }
}

fn process_directory<F, R>(config: &CliRunnerConfig, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R>,
    R: CliProcessingResult,
{
    let input = &config.input;
    let recursive = config.recursive;

    let files = crate::collect_files_small_first(input, SUPPORTED_VIDEO_EXTENSIONS, recursive);

    if files.is_empty() {
        anyhow::bail!(
            "❌ No video files found in directory: {}\n\
             💡 Supported video formats: {}\n\
             💡 Use imgquality tool for images",
            input.display(),
            SUPPORTED_VIDEO_EXTENSIONS.join(", ")
        );
    }

    info!("📂 Found {} video files to process", files.len());

    let start_time = Instant::now();
    let mut batch_result = BatchResult::new();
    let mut total_input_bytes: u64 = 0;
    let mut total_output_bytes: u64 = 0;

    for file in &files {
        match converter(file) {
            Ok(result) => {
                if result.is_skipped() {
                    info!(
                        "⏭️ {} → SKIP ({})",
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        result.skip_reason().unwrap_or("unknown")
                    );
                    batch_result.skip();
                } else if result.is_success() {
                    info!(
                        "✅ {} → {} ({})",
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        result.output_path().unwrap_or("?"),
                        result.message()
                    );
                    batch_result.success();
                    total_input_bytes += result.input_size();
                    total_output_bytes += result.output_size().unwrap_or(result.input_size());
                } else {
                    info!(
                        "❌ {} → FAILED ({})",
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        result.message()
                    );
                    batch_result.fail(file.clone(), result.message().to_string());
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                if error_msg.contains("Output exists:") {
                    info!(
                        "⏭️ {} → SKIP (output exists)",
                        file.file_name().unwrap_or_default().to_string_lossy()
                    );
                    batch_result.skip();
                } else {
                    info!("❌ {} failed: {}", file.display(), e);
                    batch_result.fail(file.clone(), e.to_string());

                    if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                        file,
                        config.output.as_deref(),
                        config.base_dir.as_deref(),
                        true,
                    ) {
                        error!("❌ Failed to copy original: {}", copy_err);
                    } else {
                        info!("📋 Copied original (conversion failed): {}", file.display());
                    }
                }
            }
        }
    }

    print_summary_report(
        &batch_result,
        start_time.elapsed(),
        total_input_bytes,
        total_output_bytes,
        &config.label,
    );

    if let Some(ref output_dir) = config.output {
        info!("\n📦 Copying unsupported files...");
        let copy_result = copy_unsupported_files(input, output_dir, recursive);
        if copy_result.copied > 0 {
            info!("📦 Copied {} unsupported files", copy_result.copied);
        }
        if copy_result.failed > 0 {
            error!("❌ Failed to copy {} files", copy_result.failed);
        }

        info!("\n🔍 Verifying output completeness...");
        let verify = verify_output_completeness(input, output_dir, recursive);
        info!("{}", verify.message);
        if !verify.passed {
            warn!("⚠️  Some files may be missing from output!");
        }

        if let Some(ref base_dir) = config.base_dir {
            info!("\n📁 Preserving directory metadata...");
            if let Err(e) = crate::metadata::preserve_directory_metadata(base_dir, output_dir) {
                error!("⚠️ Failed to preserve directory metadata: {}", e);
            } else {
                info!("✅ Directory metadata preserved");
            }
        }
    }

    Ok(())
}

fn process_single_file<F, R>(config: &CliRunnerConfig, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R>,
    R: CliProcessingResult,
{
    let input = &config.input;

    if let Some(ext) = input.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        if !SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_str.as_str()) {
            anyhow::bail!(
                "❌ Not a video file: {}\n\
                 💡 Extension: .{}\n\
                 💡 Supported video formats: {}\n\
                 💡 Use imgquality tool for images",
                input.display(),
                ext_str,
                SUPPORTED_VIDEO_EXTENSIONS.join(", ")
            );
        }
    }

    let result = match converter(input) {
        Ok(r) => r,
        Err(e) => {
            if let Some(ref output_dir) = config.output {
                if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                    input,
                    Some(output_dir),
                    config.base_dir.as_deref(),
                    true,
                ) {
                    error!("❌ Failed to copy original to output dir: {}", copy_err);
                } else {
                    info!(
                        "📋 Copied original to output (conversion failed): {}",
                        input.display()
                    );
                }
            }
            return Err(e);
        }
    };

    info!("");
    info!("📊 Conversion Summary:");
    info!(
        "   Input:  {} ({} bytes)",
        result.input_path(),
        result.input_size()
    );
    if let Some(out_path) = result.output_path() {
        info!(
            "   Output: {} ({} bytes)",
            out_path,
            result.output_size().unwrap_or(0)
        );
    }
    info!("   Result: {}", result.message());

    Ok(())
}
