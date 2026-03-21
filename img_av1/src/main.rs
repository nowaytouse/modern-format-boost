use clap::{Parser, Subcommand};
use img_av1::{calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description};
use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::modern_ui::{colors, symbols};
use shared_utils::{
    check_dangerous_directory, disk_full_pause_reason, print_summary_report, BatchPauseController,
    BatchResult,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use img_av1::conversion_api::ConversionOutput;

#[derive(Clone)]
struct AutoConvertConfig {
    output_dir: Option<PathBuf>,
    force: bool,
    recursive: bool,
    delete_original: bool,
    in_place: bool,
    explore: bool,
    match_quality: bool,
    compress: bool,
    apple_compat: bool,
    use_gpu: bool,
    ultimate: bool,
    verbose: bool,
    base_dir: Option<PathBuf>,
    child_threads: usize,
    allow_size_tolerance: bool,
    cache: Option<Arc<AnalysisCache>>,
}

#[derive(Parser)]
#[command(name = "img-av1")]
#[command(version, about = "Image quality analyzer and format upgrade tool - AV1/AVIF", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(name = "run")]
    Run {
        #[arg(short, long)]
        output: Option<PathBuf>,

        #[arg(long)]
        base_dir: Option<PathBuf>,

        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(short, long)]
        force: bool,

        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        #[arg(long)]
        delete_original: bool,

        #[arg(long)]
        in_place: bool,

        #[arg(long, default_value_t = true)]
        explore: bool,

        #[arg(long, default_value_t = true)]
        match_quality: bool,

        #[arg(long, default_value_t = true)]
        compress: bool,

        #[arg(long, default_value_t = true)]
        apple_compat: bool,

        #[arg(long)]
        no_apple_compat: bool,

        #[arg(long, default_value_t = false)]
        ultimate: bool,

        #[arg(long, default_value_t = false)]
        cpu: bool,

        #[arg(short, long)]
        verbose: bool,

        #[arg(long, default_value_t = 0)]
        child_threads: usize,

        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,

        #[arg(long)]
        no_allow_size_tolerance: bool,

        /// Force video conversion: skip meme-score check, always convert animated images to video (MOV/MP4)
        #[arg(long)]
        force_video: bool,

        /// Resume from last run: skip files already in progress file (default).
        #[arg(long, default_value_t = true)]
        resume: bool,

        /// Start fresh: ignore previous progress file, process all files.
        #[arg(long)]
        no_resume: bool,
    },

    Verify {
        original: PathBuf,

        converted: PathBuf,
    },

    RestoreTimestamps {
        #[arg(value_name = "SOURCE_DIR")]
        source: PathBuf,

        #[arg(value_name = "OUTPUT_DIR")]
        output: PathBuf,
    },

    /// Display cache statistics
    CacheStats,
}

fn main() -> anyhow::Result<()> {
    if let Err(e) =
        shared_utils::logging::init_logging("img_av1", shared_utils::logging::LogConfig::default())
    {
        eprintln!("⚠️ Failed to initialize logging: {}", e);
    }

    let cache = AnalysisCache::default_local()
        .map(Arc::new)
        .map_err(|e| {
            shared_utils::log_eprintln!("⚠️ [Cache] Failed to initialize SQLite cache: {}", e);
            e
        })
        .ok();

    if let Some(ref cache) = cache {
        if let Err(e) = cache.cleanup_old_records(30 * 24 * 3600) {
            shared_utils::log_eprintln!("⚠️ [Cache] Failed to cleanup old records: {}", e);
        }
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            input,
            output,
            force,
            recursive,
            delete_original,
            in_place,
            explore,
            match_quality,
            compress,
            apple_compat,
            no_apple_compat,
            ultimate,
            cpu,
            base_dir,
            verbose,
            child_threads,
            allow_size_tolerance,
            no_allow_size_tolerance,
            force_video,
            resume: resume_flag,
            no_resume,
        } => {
            let resume = resume_flag && !no_resume;
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let should_delete = delete_original || in_place;

            let flag_mode = match shared_utils::validate_flags_result_with_ultimate(
                explore,
                match_quality,
                compress,
                ultimate,
            ) {
                Ok(mode) => mode,
                Err(e) => {
                    shared_utils::log_eprintln!("{}", e);
                    std::process::exit(1);
                }
            };

            if verbose {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} {} (for animated→video)",
                    symbols::VIDEO,
                    flag_mode.description_en()
                ));
                shared_utils::progress_mode::emit_stderr(&format!("{} Static: JPEG→JXL (reconstruct) │ Modern Lossless→JXL (d=0.0) │ PNG/Legacy→JXL (d=0.0/0.1)", symbols::IMAGE));
            }
            shared_utils::progress_mode::set_verbose_mode(verbose);
            // Create run log automatically; quality and progress are always recorded
            if let Err(e) = shared_utils::progress_mode::set_default_run_log_file("img_av1") {
                shared_utils::log_eprintln!(
                    "⚠️  {}: {}",
                    "\x1b[33mCould not open default log file\x1b[0m",
                    e
                );
            }
            if apple_compat {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} Apple Compatibility: {}ENABLED{} (animated WebP → AV1)",
                    symbols::SHIELD,
                    colors::BOLD,
                    colors::RESET
                ));
                std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1");
            }
            if force_video {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} Force Video: {}ENABLED{} (skip meme-score)",
                    symbols::ROCKET,
                    colors::BOLD,
                    colors::RESET
                ));
                std::env::set_var("MODERN_FORMAT_BOOST_FORCE_VIDEO", "1");
            }
            if in_place {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} In-place mode: {}ENABLED{} (auto-delete original)",
                    symbols::SAVE,
                    colors::BOLD,
                    colors::RESET
                ));
            }
            if ultimate {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} Ultimate Explore: {}ENABLED{} (max SSIM mode)",
                    symbols::SEARCH,
                    colors::BOLD,
                    colors::RESET
                ));
            }
            if !allow_size_tolerance {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} Size Tolerance: {}DISABLED{} (strict < original)",
                    symbols::CHART,
                    colors::BOLD,
                    colors::RESET
                ));
            }
            if cpu {
                shared_utils::log_eprintln!("🖥️  CPU Encoding: ENABLED (libaom for maximum SSIM)");
            }

            let workload = if input.is_dir() {
                shared_utils::thread_manager::WorkloadType::Image
            } else {
                shared_utils::thread_manager::WorkloadType::Video
            };
            let thread_config = shared_utils::thread_manager::get_balanced_thread_config(workload);

            let config = AutoConvertConfig {
                output_dir: output.clone(),
                force,
                recursive,
                delete_original: should_delete,
                in_place,
                explore,
                match_quality,
                compress,
                apple_compat,
                use_gpu: !cpu,
                ultimate,
                verbose,
                base_dir: base_dir.clone(),
                child_threads: if child_threads > 0 {
                    child_threads
                } else {
                    thread_config.child_threads
                },
                allow_size_tolerance,
                cache: cache.clone(),
            };

            if input.is_file() {
                auto_convert_single_file(&input, &config)?;
            } else if input.is_dir() {
                auto_convert_directory(&input, &config, resume)?;
            } else {
                shared_utils::log_eprintln!(
                    "❌ {}: {}",
                    "\x1b[1;31mError: Input path does not exist\x1b[0m",
                    input.display()
                );
                std::process::exit(1);
            }
        }

        Commands::Verify {
            original,
            converted,
        } => {
            verify_conversion(&original, &converted, cache.as_deref())?;
        }

        Commands::RestoreTimestamps { source, output } => {
            if let Err(e) = shared_utils::restore_timestamps_from_source_to_output(&source, &output)
            {
                shared_utils::log_eprintln!(
                    "⚠️ {}: {}",
                    "\x1b[33mrestore-timestamps failed\x1b[0m",
                    e
                );
                std::process::exit(1);
            }
        }

        Commands::CacheStats => {
            if let Some(cache) = cache {
                match cache.get_statistics() {
                    Ok(stats) => {
                        println!("\n📊 Cache Statistics");
                        println!("═══════════════════════════════════════");
                        println!(
                            "📁 Database Size: {:.2} MB ({:.3} GB)",
                            stats.db_size_mb(),
                            stats.db_size_gb()
                        );
                        println!("📦 Total Records: {}", stats.total_records());
                        println!("   ├─ Analysis: {}", stats.analysis_records);
                        println!("   ├─ Quality: {}", stats.quality_records);
                        println!("   └─ Video: {}", stats.video_records);
                        println!("🔗 Path Index Entries: {}", stats.path_index_entries);
                        println!("\n🔢 Version Information:");
                        println!("   ├─ Schema Version: v{}", stats.schema_version);
                        println!(
                            "   └─ Current Algorithm: v{}",
                            stats.current_algorithm_version
                        );

                        if !stats.algorithm_version_distribution.is_empty() {
                            println!("\n📈 Algorithm Version Distribution:");
                            let mut versions: Vec<_> =
                                stats.algorithm_version_distribution.iter().collect();
                            versions.sort_by_key(|(v, _)| *v);
                            for (version, count) in versions {
                                let marker = if *version < stats.current_algorithm_version {
                                    "⚠️  (stale)"
                                } else if *version == stats.current_algorithm_version {
                                    "✅ (current)"
                                } else {
                                    "❓ (future)"
                                };
                                println!("   v{}: {} records {}", version, count, marker);
                            }

                            let stale = stats.stale_records();
                            if stale > 0 {
                                println!("\n⚠️  {} stale records detected (will be auto-invalidated on next run)", stale);
                            }
                        }

                        let usage_percent = (stats.db_size_bytes as f64
                            / shared_utils::analysis_cache::CACHE_SIZE_LIMIT_BYTES as f64)
                            * 100.0;
                        println!("\n💾 Storage Usage: {:.1}% of 85 GB limit", usage_percent);

                        if usage_percent > 80.0 {
                            println!("⚠️  Cache is approaching size limit!");
                        }

                        println!("═══════════════════════════════════════\n");
                    }
                    Err(e) => {
                        shared_utils::log_eprintln!("❌ Failed to get cache statistics: {}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                shared_utils::log_eprintln!("❌ Cache is not initialized");
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn verify_conversion(
    original: &Path,
    converted: &Path,
    cache: Option<&AnalysisCache>,
) -> anyhow::Result<()> {
    println!("🔍 Verifying conversion quality...");
    println!("   Original:  {}", original.display());
    println!("   Converted: {}", converted.display());

    let original_analysis =
        shared_utils::image_analyzer::analyze_image_with_cache(original, cache)?;
    let converted_analysis =
        shared_utils::image_analyzer::analyze_image_with_cache(converted, cache)?;

    println!("\n📊 Size Comparison:");
    println!(
        "   Original size:  {} bytes ({:.2} KB)",
        original_analysis.file_size,
        original_analysis.file_size as f64 / 1024.0
    );
    println!(
        "   Converted size: {} bytes ({:.2} KB)",
        converted_analysis.file_size,
        converted_analysis.file_size as f64 / 1024.0
    );

    let reduction =
        100.0 * (1.0 - converted_analysis.file_size as f64 / original_analysis.file_size as f64);
    println!("   Size reduction: {:.2}%", reduction);

    let orig_img = load_image_safe(original)?;
    let conv_img = load_image_safe(converted)?;

    println!("\n📏 Quality Metrics:");
    if let Some(psnr) = calculate_psnr(&orig_img, &conv_img) {
        if psnr.is_infinite() {
            println!("   PSNR: ∞ dB (Identical - mathematically lossless)");
        } else {
            println!(
                "   PSNR: {:.2} dB ({})",
                psnr,
                psnr_quality_description(psnr)
            );
        }
    }

    if let Some(ssim) = calculate_ssim(&orig_img, &conv_img) {
        println!("   SSIM: {:.6} ({})", ssim, ssim_quality_description(ssim));
    }

    println!("\n✅ Verification complete");

    Ok(())
}

fn load_image_safe(path: &Path) -> anyhow::Result<image::DynamicImage> {
    let is_jxl = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase() == "jxl")
        .unwrap_or(false);

    if is_jxl {
        use std::process::Command;

        let temp_png_file = tempfile::Builder::new()
            .suffix(".png")
            .tempfile()
            .map_err(|e| anyhow::anyhow!("Failed to create temp file: {}", e))?;

        let temp_path = temp_png_file.path();

        let status = Command::new("djxl")
            .arg(shared_utils::safe_path_arg(path).as_ref())
            .arg(shared_utils::safe_path_arg(temp_path).as_ref())
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to execute djxl: {}", e))?;

        if !status.success() {
            return Err(anyhow::anyhow!("djxl failed to decode JXL file"));
        }

        let img = shared_utils::image_detection::open_image_with_limits(temp_path)
            .map_err(|e| anyhow::anyhow!("Failed to open decoded PNG: {}", e))?;

        Ok(img)
    } else {
        Ok(shared_utils::image_detection::open_image_with_limits(path)?)
    }
}

fn copy_original_if_adjacent_mode(input: &Path, config: &AutoConvertConfig) -> anyhow::Result<()> {
    shared_utils::copy_on_skip_or_fail(
        input,
        config.output_dir.as_deref(),
        config.base_dir.as_deref(),
        config.verbose,
    )?;
    Ok(())
}

fn convert_result_to_output(result: shared_utils::ConversionResult) -> ConversionOutput {
    let input_path = result.input_path.clone();
    ConversionOutput {
        original_path: result.input_path,
        output_path: result.output_path.unwrap_or(input_path),
        skipped: result.skipped,
        message: result.message,
        original_size: result.input_size,
        output_size: result.output_size,
        size_reduction: result.size_reduction.map(|r| r as f32),
    }
}

fn auto_convert_single_file(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<ConversionOutput> {
    use img_av1::lossless_converter::{
        convert_jpeg_to_jxl, convert_to_av1_mp4, convert_to_av1_mp4_matched, convert_to_jxl,
        convert_to_jxl_matched, ConvertOptions,
    };

    // Pause if the user is being prompted to exit via Ctrl+C
    shared_utils::ctrlc_guard::wait_if_prompt_active();

    // Check for Apple Photos library before processing
    if let Err(e) = shared_utils::check_apple_photos_library(input) {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    let fixed_input = shared_utils::fix_extension_if_mismatch(input)?;
    let input = fixed_input.as_path();

    // Always skip HEIC/HEIF: Lossless is extremely rare, and re-encoding lossy HEIC causes generational loss.
    // Apple ecosystem also heavily relies on original HEIC/HEIF files.
    if shared_utils::image_heic_analysis::is_heic_file(input) {
        let file_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        copy_original_if_adjacent_mode(input, config)?;
        return Ok(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            message: "HEIC/HEIF detected; skipping to avoid generational loss and preserve original fidelity".to_string(),
            original_size: file_size,
            output_size: None,
            size_reduction: None,
        });
    }

    let analysis =
        shared_utils::image_analyzer::analyze_image_with_cache(input, config.cache.as_deref())?;

    // Single source of truth for static skip: JXL + modern lossy (avoid generational loss).
    if !analysis.is_animated {
        let skip =
            shared_utils::should_skip_image_format(analysis.format.as_str(), analysis.is_lossless);
        if skip.should_skip {
            if config.verbose {
                println!("⏭️ {}: {}", skip.reason, input.display());
            }
            copy_original_if_adjacent_mode(input, config)?;
            return Ok(ConversionOutput {
                original_path: input.display().to_string(),
                output_path: input.display().to_string(),
                skipped: true,
                message: skip.reason,
                original_size: analysis.file_size,
                output_size: None,
                size_reduction: None,
            });
        }
    }

    // 完整接入图像质量分析：静态图始终做像素级分析，用于路由 + 质量输出（自动写入 run log）
    // 针对性：JPEG 这种已经明确走 lossless transcode 到 JXL 的不需要开启昂贵的像素级分析
    let pixel_analysis = if !analysis.is_animated && analysis.format != "JPEG" {
        shared_utils::image_quality_detector::analyze_image_quality_with_cache(
            input,
            config.cache.as_deref(),
        )
    } else {
        None
    };
    if let Some(ref q) = pixel_analysis {
        shared_utils::log_media_info_for_image_quality(q, input);
    }

    let mut quality_label = analysis.quality_summary();
    if let Some(ref pa) = pixel_analysis {
        let ct_str = pa.content_type.name.to_uppercase();
        quality_label = if quality_label.is_empty() {
            ct_str
        } else {
            format!("{}: {}", ct_str, quality_label)
        };
    }

    let options = ConvertOptions {
        force: config.force,
        output_dir: config.output_dir.clone(),
        base_dir: config.base_dir.clone(),
        delete_original: config.delete_original,
        in_place: config.in_place,
        explore: config.explore,
        match_quality: config.match_quality,
        compress: config.compress,
        apple_compat: config.apple_compat,
        use_gpu: config.use_gpu,
        ultimate: config.ultimate,
        allow_size_tolerance: config.allow_size_tolerance,
        verbose: config.verbose,
        child_threads: config.child_threads,
        input_format: Some(analysis.format.clone()),
        quality_label: Some(quality_label),
    };

    macro_rules! verbose_log {
        ($($arg:tt)*) => {
            if config.verbose {
                println!($($arg)*);
            }
        };
    }

    let make_skipped = |msg: &str| -> ConversionOutput {
        ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            message: msg.to_string(),
            original_size: analysis.file_size,
            output_size: None,
            size_reduction: None,
        }
    };

    let result = match (
        analysis.format.as_str(),
        analysis.is_lossless,
        analysis.is_animated,
    ) {
        ("WebP", true, false)
        | ("AVIF", true, false)
        | ("TIFF", true, false)
        | ("HEIC", true, false)
        | ("HEIF", true, false) => {
            verbose_log!("🔄 Modern Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0, analysis.hdr_info.as_ref())?
        }
        // Static modern lossy / JXL already handled by should_skip_image_format above.
        ("JPEG", _, false) => {
            if config.match_quality {
                verbose_log!("🔄 JPEG→JXL (MATCH QUALITY): {}", input.display());
                convert_to_jxl_matched(input, &options, &analysis)?
            } else {
                verbose_log!("🔄 JPEG→JXL lossless transcode: {}", input.display());
                convert_jpeg_to_jxl(input, &options, analysis.hdr_info.as_ref())?
            }
        }
        (_, true, false) => {
            verbose_log!("🔄 Legacy Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0, analysis.hdr_info.as_ref())?
        }
        (format, is_lossless, true) => {
            let is_modern_animated = matches!(format, "WebP" | "AVIF" | "HEIC" | "HEIF" | "JXL");
            let is_apple_native = matches!(format, "HEIC" | "HEIF");

            let should_skip_modern = if is_modern_animated && !is_lossless {
                if config.apple_compat {
                    is_apple_native
                } else {
                    true
                }
            } else {
                false
            };

            if should_skip_modern {
                verbose_log!(
                    "⏭️ Skipping modern lossy animated format (avoid generational loss): {}",
                    input.display()
                );
                if is_apple_native && config.apple_compat {
                    verbose_log!("   💡 Reason: {} is already a native Apple format", format);
                } else {
                    verbose_log!(
                        "   💡 Use --apple-compat to convert to AV1 for Apple device compatibility"
                    );
                }
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(make_skipped("Skipping modern lossy animated format"));
            }

            let duration = match analysis.duration_secs {
                Some(d) if d > 0.0 => d,
                Some(0.0) => {
                    let is_modern = matches!(format, "WebP" | "AVIF" | "JXL" | "HEIC" | "HEIF");
                    let use_lossless = analysis.is_lossless;

                    if is_modern && !use_lossless {
                        verbose_log!(
                            "⏭️ Skipping static-disguised modern format (lossy): {}",
                            input.display()
                        );
                        copy_original_if_adjacent_mode(input, config)?;
                        return Ok(make_skipped(
                            "Skipping static modern format to avoid generational loss",
                        ));
                    }

                    let distance = if use_lossless { 0.0 } else { 0.1 };
                    verbose_log!(
                        "🔄 Static GIF/Modern→JXL ({}): {}",
                        if distance == 0.0 {
                            "Lossless"
                        } else {
                            "Quality 100"
                        },
                        input.display()
                    );
                    let conv_result = if use_lossless {
                        convert_to_jxl(input, &options, 0.0, analysis.hdr_info.as_ref())?
                    } else if config.match_quality {
                        convert_to_jxl_matched(input, &options, &analysis)?
                    } else {
                        convert_to_jxl(input, &options, 0.1, analysis.hdr_info.as_ref())?
                    };
                    return Ok(convert_result_to_output(conv_result));
                }
                _ => {
                    let retry =
                        shared_utils::image_analyzer::get_animation_duration_for_path(input);
                    if let Some(d) = retry {
                        d
                    } else {
                        shared_utils::log_eprintln!(
                            "⚠️  {}: {}",
                            "\x1b[33mCannot get animation duration, skipping conversion\x1b[0m",
                            input.display()
                        );
                        shared_utils::log_eprintln!("   💡 Possible cause: ffprobe not installed or file format doesn't support duration detection");
                        shared_utils::log_eprintln!(
                            "   💡 Suggestion: install ffprobe: brew install ffmpeg"
                        );
                        copy_original_if_adjacent_mode(input, config)?;
                        return Ok(make_skipped("Cannot get animation duration"));
                    }
                }
            };

            // Use meme-score to decide video vs GIF for all animated routes
            let force_video = std::env::var("MODERN_FORMAT_BOOST_FORCE_VIDEO").is_ok();
            let probe = match shared_utils::probe_video(input) {
                Ok(probe) => Some(probe),
                Err(e) => {
                    shared_utils::log_eprintln!(
                        "⚠️ [GIF Probe] Failed to probe animated input {}: {}",
                        input.display(),
                        e
                    );
                    None
                }
            };
            let meme_keep = if force_video {
                // Force video mode: always convert to video, skip meme-score
                false
            } else if let Some(ref p) = probe {
                if let Some(mut meta) =
                    shared_utils::gif_meta_from_probe_with_path(p, analysis.file_size, input)
                {
                    if let Ok((pal, exts)) = shared_utils::scan_gif_headers(input) {
                        meta.palette_size = pal;
                        meta.app_extensions = exts;
                    }

                    shared_utils::should_keep_as_gif(&meta)
                } else {
                    shared_utils::progress_mode::emit_stderr(&format!(
                        "🎞️  GIF [{}] probe failed → KEEP GIF",
                        input.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    true
                }
            } else {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "🎞️  GIF [{}] probe failed → KEEP GIF",
                    input.file_name().unwrap_or_default().to_string_lossy()
                ));
                true
            };

            if meme_keep {
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(make_skipped("GIF meme-score: keep as GIF"));
            } else {
                if is_lossless {
                    shared_utils::progress_mode::emit_stderr(&format!(
                        "🔄 Animated {}→AV1 MP4 (CRF 0, {:.1}s): {}",
                        format,
                        duration,
                        input.display()
                    ));
                    convert_to_av1_mp4(input, &options)?
                } else {
                    shared_utils::progress_mode::emit_stderr(&format!(
                        "🔄 Animated {}→AV1 MP4 (SMART QUALITY, {:.1}s): {}",
                        format,
                        duration,
                        input.display()
                    ));
                    convert_to_av1_mp4_matched(input, &options, &analysis)?
                }
            }
        }
        (_, false, false) => {
            // Modern lossy static already skipped above; only legacy lossy reach here.
            if config.match_quality {
                verbose_log!("🔄 Legacy Lossy→JXL (MATCH QUALITY): {}", input.display());
                convert_to_jxl_matched(input, &options, &analysis)?
            } else {
                verbose_log!("🔄 Legacy Lossy→JXL (Quality 100): {}", input.display());
                convert_to_jxl(input, &options, 0.1, analysis.hdr_info.as_ref())?
            }
        }
    };

    let output = ConversionOutput {
        original_path: result.input_path.clone(),
        output_path: result.output_path.clone().unwrap_or(result.input_path),
        skipped: result.skipped,
        message: result.message.clone(),
        original_size: result.input_size,
        output_size: result.output_size,
        size_reduction: result.size_reduction.map(|r| r as f32),
    };

    if output.skipped {
        verbose_log!("⏭️ {}", output.message);
    } else if output.is_jpeg_transcode() {
        shared_utils::verbose_eprintln!("{}", output.message);
    } else {
        shared_utils::log_eprintln!("{}", output.message);
    }

    Ok(output)
}

fn auto_convert_directory(
    input: &Path,
    config: &AutoConvertConfig,
    resume: bool,
) -> anyhow::Result<()> {
    // Check for Apple Photos library before any processing
    if let Err(e) = shared_utils::check_apple_photos_library(input) {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    if config.delete_original || config.in_place {
        if let Err(e) = check_dangerous_directory(input) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

    let mut config_with_base = config.clone();
    if config_with_base.output_dir.is_some() && config_with_base.base_dir.is_none() {
        config_with_base.base_dir = Some(input.to_path_buf());
    }

    let thread_config = shared_utils::thread_manager::get_balanced_thread_config(
        shared_utils::thread_manager::WorkloadType::Image,
    );
    let pool_size = thread_config.parallel_tasks;
    config_with_base.child_threads = thread_config.child_threads;

    let config = &config_with_base;

    let start_time = Instant::now();

    let saved_dir_timestamps = match shared_utils::save_directory_timestamps(input) {
        Ok(saved) => Some(saved),
        Err(e) => {
            shared_utils::log_eprintln!(
                "⚠️ [Metadata] Failed to snapshot directory timestamps for {}: {}",
                input.display(),
                e
            );
            None
        }
    };

    let files = shared_utils::collect_image_files_for_perceived_speed(
        input,
        shared_utils::SUPPORTED_IMAGE_EXTENSIONS,
        config.recursive,
    );

    let total = files.len();
    if total == 0 {
        println!("📂 No image files found in {}", input.display());

        if let Some(output_dir) = config.output_dir.as_ref() {
            if let Some(ref base_dir) = config.base_dir {
                shared_utils::preserve_directory_metadata_with_log(base_dir, output_dir);
            }
        }

        return Ok(());
    }

    if config.verbose {
        println!("📂 Found {} files to process", total);
        shared_utils::log_eprintln!(
            "⚡ Queue Strategy: deeper paths → fast JPEG/direct transcodes → smaller files → lower resolution"
        );
    }

    // Initialize checkpoint manager for resume/progress tracking
    let checkpoint = if resume {
        match shared_utils::checkpoint::CheckpointManager::new(input) {
            Ok(cp) => {
                if let Err(err) = cp.reset_if_output_root_missing(config.output_dir.as_deref()) {
                    shared_utils::log_eprintln!(
                        "⚠️ [checkpoint] Failed to reset stale resume state for missing output root: {}",
                        err
                    );
                }
                if cp.is_resume_mode() {
                    if config.verbose {
                        println!(
                            "📂 Resume: skipping {} already completed images",
                            cp.completed_count()
                        );
                    }
                    cp.sync_to_processed_list();
                } else {
                    shared_utils::clear_processed_list();
                }
                Some(cp)
            }
            Err(e) => {
                if config.verbose {
                    println!("⚠️ [checkpoint] Failed to initialize: {}", e);
                }
                None
            }
        }
    } else {
        shared_utils::clear_processed_list();
        None
    };

    if std::env::var("MFB_SKIP_DISK_PRECHECK").as_deref() != Ok("1") {
        let total_input_size: u64 = files
            .iter()
            .map(|f| match std::fs::metadata(f) {
                Ok(metadata) => metadata.len(),
                Err(err) => {
                    shared_utils::log_eprintln!(
                        "⚠️ [Disk Precheck] Failed to read file metadata ({}): {}",
                        f.display(),
                        err
                    );
                    0
                }
            })
            .sum();
        let check_path = config.output_dir.as_deref().unwrap_or(input);
        if let Some(avail) = shared_utils::system_memory::get_available_disk_bytes(check_path) {
            // Reserve 1 GB headroom on top of total input size (temp files, partial encodes, etc.)
            let required = total_input_size.saturating_add(1024 * 1024 * 1024);
            if avail < required {
                let avail_gb = avail as f64 / (1024.0 * 1024.0 * 1024.0);
                let required_gb = required as f64 / (1024.0 * 1024.0 * 1024.0);
                eprintln!(
                    "❌ Insufficient disk space on output volume.\n\
                     💾 Available: {:.2} GB\n\
                     💾 Required:  {:.2} GB (input size + 1 GB headroom)\n\
                     💡 Free up space or choose a different output location.",
                    avail_gb, required_gb
                );
                std::process::exit(1);
            }
            if config.verbose {
                println!(
                    "💾 Disk space OK: {:.2} GB available, {:.2} GB required",
                    avail as f64 / (1024.0 * 1024.0 * 1024.0),
                    required as f64 / (1024.0 * 1024.0 * 1024.0)
                );
            }
        }
    }

    let success = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let actual_input_bytes = std::sync::atomic::AtomicU64::new(0);
    let actual_output_bytes = std::sync::atomic::AtomicU64::new(0);
    let pause_controller = Arc::new(BatchPauseController::new());

    // Initialize Ctrl+C guard for long-running batch operations
    shared_utils::ctrlc_guard::init();

    shared_utils::progress_mode::enable_quiet_mode();
    let progress_bar = Arc::new(shared_utils::CoarseProgressBar::new(
        total as u64,
        "Running",
    ));

    let max_threads = pool_size;
    let child_threads = thread_config.child_threads;

    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(max_threads)
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            shared_utils::log_eprintln!(
                "⚠️  {}: {}, falling back to 2 threads",
                format!(
                    "\x1b[33mFailed to create {} thread pool\x1b[0m",
                    max_threads
                ),
                e
            );
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to create fallback thread pool: {}", e))?
        }
    };

    if config.verbose {
        shared_utils::log_eprintln!(
            "🔧 Thread Strategy: {} parallel tasks x {} threads/task (CPU cores: {})",
            max_threads,
            child_threads,
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        );
        if let Some(hint) = shared_utils::thread_manager::memory_cap_hint() {
            shared_utils::log_eprintln!("   💡 {}", hint);
        }
    }

    let next_index = AtomicUsize::new(0);
    pool.install(|| {
        rayon::scope(|scope| {
            for _ in 0..max_threads {
                let next_index = &next_index;
                scope.spawn(|_| loop {
                    if pause_controller.is_paused() {
                        break;
                    }

                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    if index >= total {
                        break;
                    }

                    let path = &files[index];
                    progress_bar.set_message(&path.file_name().unwrap_or_default().to_string_lossy());

                    // Check if already completed (thread-safe)
                    if let Some(cp) = checkpoint.as_ref() {
                        if cp.is_completed(path) {
                            skipped.fetch_add(1, Ordering::Relaxed);
                            let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                            shared_utils::progress_mode::write_progress_line_to_run_log(
                                start_time.elapsed().as_secs(),
                                current as u64,
                                total as u64,
                                &path.file_name().unwrap_or_default().to_string_lossy(),
                            );
                            progress_bar.set(current as u64);
                            continue;
                        }
                    }

                    match auto_convert_single_file(path, config) {
                        Ok(result) => {
                            if result.skipped {
                                skipped.fetch_add(1, Ordering::Relaxed);
                            } else {
                                success.fetch_add(1, Ordering::Relaxed);
                                shared_utils::progress_mode::image_processed_success();
                                actual_input_bytes.fetch_add(result.original_size, Ordering::Relaxed);
                                if let Some(out_size) = result.output_size {
                                    actual_output_bytes.fetch_add(out_size, Ordering::Relaxed);
                                }
                                // Mark as completed in checkpoint manager on success (thread-safe)
                                if let Some(cp) = checkpoint.as_ref() {
                                    if let Err(e) = cp.mark_completed(path) {
                                        shared_utils::log_eprintln!(
                                            "⚠️ [checkpoint] Failed to mark completed {}: {}",
                                            path.display(),
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            if msg.contains("Skipped") || msg.contains("skip") {
                                skipped.fetch_add(1, Ordering::Relaxed);
                            } else if let Some(reason) = disk_full_pause_reason(&msg) {
                                if pause_controller.request_pause(path, reason.clone()) {
                                    shared_utils::log_eprintln!(
                                        "⏸️ [Batch] Paused at {}: {}",
                                        path.display(),
                                        reason
                                    );
                                }
                                continue;
                            } else {
                                let err_str = e.to_string();
                                shared_utils::log_auto_error!("Image conversion", "Failed {}: {}", path.display(), e);
                                shared_utils::progress_mode::log_conversion_failure(path, &err_str);
                                failed.fetch_add(1, Ordering::Relaxed);
                                shared_utils::progress_mode::image_processed_failure();

                                if let Some(ref output_dir) = config.output_dir {
                                    if let Err(copy_err) = shared_utils::copy_on_skip_or_fail(
                                        path,
                                        Some(output_dir),
                                        config.base_dir.as_deref(),
                                        config.verbose,
                                    ) {
                                        shared_utils::log_eprintln!(
                                            "⚠️ [Recovery] Failed to copy original after image conversion failure ({}): {}",
                                            path.display(),
                                            copy_err
                                        );
                                    }
                                }
                            }
                        }
                    }
                    let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    shared_utils::progress_mode::write_progress_line_to_run_log(
                        start_time.elapsed().as_secs(),
                        current as u64,
                        total as u64,
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                    );
                    progress_bar.set(current as u64);
                });
            }
        });
    });

    progress_bar.finish();
    shared_utils::progress_mode::disable_quiet_mode();
    shared_utils::progress_mode::xmp_merge_finalize();
    shared_utils::progress_mode::flush_log_file();

    let success_count = success.load(Ordering::Relaxed);
    let skipped_count = skipped.load(Ordering::Relaxed);
    let failed_count = failed.load(Ordering::Relaxed);
    let processed_count = processed.load(Ordering::Relaxed);

    let mut result = BatchResult::new();
    result.succeeded = success_count;
    result.failed = failed_count;
    result.skipped = skipped_count;
    result.total = processed_count;
    if let Some(pause) = pause_controller.pause_info() {
        result.pause(
            pause.path,
            pause.reason,
            total.saturating_sub(processed_count),
        );
    }

    let final_input_bytes = actual_input_bytes.load(Ordering::Relaxed);
    let final_output_bytes = actual_output_bytes.load(Ordering::Relaxed);

    print_summary_report(
        &result,
        start_time.elapsed(),
        final_input_bytes,
        final_output_bytes,
        "Image Conversion",
    );

    if !result.paused {
        if let Some(ref output_dir) = config.output_dir {
            if let Some(ref base_dir) = config.base_dir {
                shared_utils::preserve_directory_metadata_with_log(base_dir, output_dir);
            }
        }
    }

    if let Some(ref saved) = saved_dir_timestamps {
        if !result.paused {
            if let Some(ref output_dir) = config.output_dir {
                if let Some(ref base_dir) = config.base_dir {
                    shared_utils::apply_saved_timestamps_to_dst(saved, base_dir, output_dir);
                }
            }
        }
        shared_utils::restore_directory_timestamps(saved);
        shared_utils::log_eprintln!("✅ Directory timestamps restored");
    }

    // Finalize checkpoint only on 100% success
    if let Some(cp) = checkpoint {
        if result.paused {
            if let Err(e) = cp.release_lock() {
                shared_utils::log_eprintln!("⚠️ [checkpoint] Release lock failed: {}", e);
            }
        } else if failed_count == 0 {
            if let Err(e) = cp.cleanup() {
                shared_utils::log_eprintln!("⚠️ [checkpoint] Cleanup failed: {}", e);
            }
        } else if let Err(e) = cp.release_lock() {
            shared_utils::log_eprintln!("⚠️ [checkpoint] Release lock failed: {}", e);
        }
    }

    Ok(())
}
