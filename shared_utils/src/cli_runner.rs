use crate::batch::BatchResult;
use crate::file_copier::{SUPPORTED_VIDEO_EXTENSIONS, copy_unsupported_files, verify_output_completeness};
use crate::report::print_summary_report;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;
use log::{info, warn, error};
use walkdir::WalkDir;

/// Trait to unify result reporting from different tools
pub trait CliProcessingResult {
    fn is_skipped(&self) -> bool;
    fn skip_reason(&self) -> Option<&str>;
    fn input_path(&self) -> &str;
    fn output_path(&self) -> Option<&str>;
    fn input_size(&self) -> u64;
    fn output_size(&self) -> Option<u64>;
    fn message(&self) -> &str;
}

// Default impl for shared_utils::conversion::ConversionResult
impl CliProcessingResult for crate::conversion::ConversionResult {
    fn is_skipped(&self) -> bool { self.skipped }
    fn skip_reason(&self) -> Option<&str> { self.skip_reason.as_deref() }
    fn input_path(&self) -> &str { &self.input_path }
    fn output_path(&self) -> Option<&str> { self.output_path.as_deref() }
    fn input_size(&self) -> u64 { self.input_size }
    fn output_size(&self) -> Option<u64> { self.output_size }
    fn message(&self) -> &str { &self.message }
}

/// Configuration for the CLI runner
pub struct CliRunnerConfig {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub recursive: bool,
    pub label: String, // e.g. "AV1 Video" or "HEVC Video"
}

/// Run the "Auto" command logic for batch processing
pub fn run_auto_command<F, R>(
    config: CliRunnerConfig,
    converter: F,
) -> Result<()>
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

    // 1. Find files
    let walker = if recursive {
        WalkDir::new(input).follow_links(true)
    } else {
        WalkDir::new(input).max_depth(1)
    };

    let files: Vec<_> = walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            if let Some(ext) = e.path().extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_str.as_str())
            } else {
                false
            }
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    if files.is_empty() {
        anyhow::bail!(
            "❌ 目录中没有找到视频文件: {}\n\
             💡 支持的视频格式: {}\n\
             💡 如果要处理图像，请使用 imgquality 工具",
            input.display(),
            SUPPORTED_VIDEO_EXTENSIONS.join(", ")
        );
    }

    info!("📂 Found {} video files to process", files.len());

    // 2. Process Batch
    let start_time = Instant::now();
    let mut batch_result = BatchResult::new();
    let mut total_input_bytes: u64 = 0;
    let mut total_output_bytes: u64 = 0;

    for file in &files {
        match converter(file) {
            Ok(result) => {
                if result.is_skipped() {
                    info!("⏭️ {} → SKIP ({})", 
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        result.skip_reason().unwrap_or("unknown")
                    );
                    batch_result.skip();
                } else if result.output_size().unwrap_or(0) == 0 && result.output_path().is_none() {
                    // Legacy "skip" signals (empty result)
                    info!("⏭️ {} → SKIP (legacy signal)", 
                        file.file_name().unwrap_or_default().to_string_lossy()
                    );
                    batch_result.skip();
                } else {
                    info!("✅ {} → {} ({})", 
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        result.output_path().unwrap_or("?"),
                        result.message() // Message already contains size reduction info if formatted correctly
                    );
                    batch_result.success();
                    total_input_bytes += result.input_size();
                    total_output_bytes += result.output_size().unwrap_or(result.input_size());
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                if error_msg.contains("Output exists:") {
                     info!("⏭️ {} → SKIP (output exists)", 
                        file.file_name().unwrap_or_default().to_string_lossy()
                    );
                    batch_result.skip();
                } else {
                    info!("❌ {} failed: {}", file.display(), e);
                    batch_result.fail(file.clone(), e.to_string());

                    // 🔥 Fallback: Copy original if conversion failed (No data loss)
                    // 注意：cli_runner 是通用工具，不保证目录结构
                    if let Some(ref out_dir) = config.output {
                        let file_name = file.file_name().unwrap_or_default();
                        let dest = out_dir.join(file_name);
                        if !dest.exists() {
                            if let Err(copy_err) = std::fs::copy(file, &dest) {
                                error!("❌ Failed to copy original: {}", copy_err);
                            } else {
                                info!("📋 Copied original (conversion failed): {}", file.display());
                                let _ = crate::xmp_merger::merge_xmp_for_copied_file(file, &dest);
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Summary Report
    print_summary_report(
        &batch_result,
        start_time.elapsed(),
        total_input_bytes,
        total_output_bytes,
        &config.label,
    );

    // 4. Post-processing (Copy unsupported, verify)
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
    }

    Ok(())
}

fn process_single_file<F, R>(config: &CliRunnerConfig, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R>,
    R: CliProcessingResult,
{
    let input = &config.input;
    
    // Check extension
    if let Some(ext) = input.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        if !SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_str.as_str()) {
             anyhow::bail!(
                "❌ 不是视频文件: {}\n\
                 💡 文件扩展名: .{}\n\
                 💡 支持的视频格式: {}\n\
                 💡 如果要处理图像，请使用 imgquality 工具",
                input.display(),
                ext_str,
                SUPPORTED_VIDEO_EXTENSIONS.join(", ")
            );
        }
    }

    let result = converter(input)?;

    info!("");
    info!("📊 Conversion Summary:");
    info!("   Input:  {} ({} bytes)", result.input_path(), result.input_size());
    if let Some(out_path) = result.output_path() {
        info!("   Output: {} ({} bytes)", out_path, result.output_size().unwrap_or(0));
    }
    info!("   Result: {}", result.message());
    
    Ok(())
}
