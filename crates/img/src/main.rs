use clap::{Parser, Subcommand};

use img::{calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description};
use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::modern_ui::{colors, symbols};
use shared_utils::quality_matcher::SourceCodec;
use shared_utils::{
    check_dangerous_directory, disk_full_pause_reason, print_summary_report, BatchPauseController,
    BatchResult,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "img")]
#[command(version, about = "Image quality analyzer and format upgrade tool", long_about = None)]
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

        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,

        #[arg(long)]
        no_allow_size_tolerance: bool,

        #[arg(short, long)]
        verbose: bool,

        /// Resume from last run: skip files already in progress file.
        #[arg(long, default_value_t = false)]
        resume: bool,

        /// Start fresh: ignore previous progress file, process all files.
        #[arg(long)]
        no_resume: bool,

        #[arg(long, value_parser = ["hevc", "av1"], default_value = "hevc")]
        codec: String,
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

    /// Internal: Check if a directory is already locked by MFB
    LockCheck {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },

    /// Internal: Get the lock hash for a directory
    PathHash {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    if let Err(e) = shared_utils::init_ghost_mode() {
        eprintln!("⚠️ Failed to initialize Ghost Mode isolation: {e}");
    }

    if let Err(e) =
        shared_utils::logging::init_logging("img", shared_utils::logging::LogConfig::default())
    {
        eprintln!("⚠️ Failed to initialize logging: {e}");
    }

    let cache = AnalysisCache::default_local()
        .map(Arc::new)
        .map_err(|e| {
            shared_utils::log_eprintln!("⚠️  [Cache] Failed to initialize SQLite cache: {}", e);
            e
        })
        .ok();

    if let Some(ref cache) = cache {
        if let Err(e) = cache.cleanup_old_records(30 * 24 * 3600) {
            shared_utils::log_eprintln!("⚠️ [Cache] Failed to cleanup old records: {}", e);
        }
    }

    let cli = Cli::parse();

    // --- Unified Directory Locking (Ghost Mode & Mutex) ---
    // Extract input path from relevant commands to lock the directory ONLY if it involves destructive or interactive shared state.
    let input_to_lock = match &cli.command {
        Commands::Run {
            input, in_place, ..
        } if *in_place => Some(input),
        Commands::Verify {
            original: input, ..
        }
        | Commands::RestoreTimestamps { source: input, .. }
        | Commands::LockCheck { input } => Some(input),
        _ => None,
    };

    let _lock_guard = if let Some(input) = input_to_lock {
        let input_abs = std::fs::canonicalize(input).unwrap_or_else(|_| input.clone());
        if input_abs.is_dir() {
            match shared_utils::acquire_dir_lock(&input_abs) {
                Ok(guard) => Some(guard),
                Err(e) => {
                    shared_utils::log_eprintln!("❌ {e}");
                    std::process::exit(3);
                }
            }
        } else {
            None
        }
    } else {
        None
    };
    // ------------------------------------------------------

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
            allow_size_tolerance,
            no_allow_size_tolerance,
            verbose,

            base_dir,
            resume: resume_flag,
            no_resume,
            codec,
        } => {
            use shared_utils::conversion_types::SelectedCodec;
            let resume = resume_flag && !no_resume;
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let should_delete = delete_original || in_place;

            let selected_codec = if codec.to_lowercase() == "av1" {
                SelectedCodec::Av1
            } else {
                SelectedCodec::Hevc
            };

            if selected_codec == SelectedCodec::Av1 && apple_compat {
                shared_utils::log_eprintln!("❌ Apple compatibility mode (--apple-compat) is ONLY supported for HEVC. AV1 strategy does not support Apple devices natively.");
                std::process::exit(1);
            }

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

            // Fail-fast if critical sub-tools are missing
            if let Err(e) =
                shared_utils::tools::require_tools(&["cjxl", "djxl", "exiftool", "ffmpeg"])
            {
                shared_utils::log_eprintln!("{e}");
                std::process::exit(1);
            }

            shared_utils::progress_mode::set_verbose_mode(verbose);
            // Create run log first; all subsequent output is captured here
            if let Err(e) = shared_utils::progress_mode::set_default_run_log_file("img") {
                shared_utils::log_eprintln!(
                    "⚠️  {}: {}",
                    "\x1b[33mCould not open run log file\x1b[0m",
                    e
                );
            }
            if verbose {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} {} ({} for animated→video)",
                    symbols::VIDEO,
                    flag_mode.description_en(),
                    selected_codec.as_str().to_uppercase()
                ));
                shared_utils::progress_mode::emit_stderr(&format!("{} Static: JPEG→JXL (reconstruct) │ Modern Lossless→JXL (d=0.0) │ PNG/Legacy→JXL (d=0.0/0.001)", symbols::IMAGE));
            }
            if apple_compat {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "{} Apple Compatibility: {}ENABLED{} (WebP→HEVC)",
                    symbols::SHIELD,
                    colors::BOLD,
                    colors::RESET
                ));
                std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1");
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
            shared_utils::database::report_db_status();

            let config = AutoConvertConfig {
                output_dir: output,
                base_dir,
                force,
                delete_original: should_delete,
                in_place,
                explore,
                match_quality,
                compress,
                apple_compat,
                use_gpu: true,
                ultimate,
                allow_size_tolerance,
                verbose,
                child_threads: 0,

                cache: cache.clone(),
                codec: selected_codec,
            };

            let workload = shared_utils::thread_manager::WorkloadType::Image;
            let thread_config = shared_utils::thread_manager::get_balanced_thread_config(workload);
            let mut config = config;
            config.child_threads = thread_config.child_threads;

            if input.is_file() {
                auto_convert_single_file(&input, &config)?;
            } else if input.is_dir() {
                auto_convert_directory(&input, &config, recursive, resume)?;
            } else {
                shared_utils::progress_mode::emit_stderr(&format!(
                    "❌ Error: Input path does not exist: {}",
                    input.display()
                ));
                std::process::exit(1);
            }
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

        Commands::Verify {
            original,
            converted,
        } => {
            verify_conversion(&original, &converted, cache.as_deref())?;
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
                                println!("   v{version}: {count} records {marker}");
                            }

                            let stale = stats.stale_records();
                            if stale > 0 {
                                println!("\n⚠️  {stale} stale records detected (will be auto-invalidated on next run)");
                            }
                        }

                        let usage_percent = (stats.db_size_bytes as f64
                            / shared_utils::analysis_cache::CACHE_SIZE_LIMIT_BYTES as f64)
                            * 100.0;
                        println!("\n💾 Storage Usage: {usage_percent:.1}% of 85 GB limit");

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

        Commands::LockCheck { input } => {
            let input_abs = std::fs::canonicalize(&input).unwrap_or_else(|_| input.clone());
            if input_abs.is_dir() {
                // Try to acquire lock. If it fails, report and exit immediately with code 3.
                match shared_utils::acquire_dir_lock(&input_abs) {
                    Ok(_lock) => {
                        println!("✅ Directory is available for processing.");
                    }
                    Err(e) => {
                        shared_utils::log_eprintln!("❌ {e}");
                        std::process::exit(3);
                    }
                }
            }
        }

        Commands::PathHash { input } => {
            let hash = shared_utils::hash_path_to_hex(&input).unwrap_or_else(|_| "err".to_string());
            println!("{hash}");
        }
    }

    Ok(())
}

fn verify_conversion(
    original: &std::path::Path,
    converted: &std::path::Path,
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
    println!("   Size reduction: {reduction:.2}%");

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

fn load_image_safe(path: &std::path::Path) -> anyhow::Result<image::DynamicImage> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_jxl = shared_utils::quality_matcher::parse_source_codec(&ext) == SourceCodec::JpegXl;

    if is_jxl {
        let temp_png_file = tempfile::Builder::new()
            .suffix(".png")
            .tempfile_in(shared_utils::get_mfb_tmp_dir()?)
            .map_err(|e| anyhow::anyhow!("Failed to create temp file in MFB tmp: {e}"))?;

        let temp_path = temp_png_file.path();

        let mut builder = shared_utils::jxl_builder::DjxlBuilder::new();
        builder.input(path).output(temp_path);

        let status = builder
            .build()
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to execute djxl: {e}"))?;

        if !status.success() {
            return Err(anyhow::anyhow!("djxl failed to decode JXL file"));
        }

        let img = shared_utils::image_detection::open_image_with_limits(temp_path)
            .map_err(|e| anyhow::anyhow!("Failed to open decoded PNG: {e}"))?;

        Ok(img)
    } else {
        shared_utils::image_detection::open_image_with_limits(path)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// Print image analysis in human-readable format.
/// Currently unused but kept for potential future CLI output mode.
#[derive(Clone)]
struct AutoConvertConfig {
    output_dir: Option<PathBuf>,
    base_dir: Option<PathBuf>,
    force: bool,
    delete_original: bool,
    in_place: bool,
    explore: bool,
    match_quality: bool,
    compress: bool,
    apple_compat: bool,
    use_gpu: bool,
    ultimate: bool,
    allow_size_tolerance: bool,
    verbose: bool,
    child_threads: usize,

    cache: Option<Arc<AnalysisCache>>,
    codec: shared_utils::conversion_types::SelectedCodec,
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

use img::conversion_api::ConversionOutput;

fn convert_result_to_output(result: shared_utils::ConversionResult) -> ConversionOutput {
    let input_path = result.input_path.clone();
    ConversionOutput {
        original_path: result.input_path,
        output_path: result.output_path.unwrap_or(input_path),
        skipped: result.skipped,
        ignored: result.ignored,
        message: result.message,
        original_size: result.input_size,
        output_size: result.output_size,
        size_reduction: result
            .size_reduction
            .map(shared_utils::numeric_cast::f64_to_f32_lossy),
        blake3: result.blake3,
    }
}

fn auto_convert_single_file(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<ConversionOutput> {
    use img::lossless_converter::ConvertOptions;

    // Pause if the user is being prompted to exit via Ctrl+C
    shared_utils::ctrlc_guard::wait_if_prompt_active();

    // Check for Apple Photos library before processing
    if let Err(e) = shared_utils::check_apple_photos_library(input) {
        eprintln!("{e}");
        std::process::exit(1);
    }

    let fixed_input = shared_utils::fix_extension_if_mismatch(input)?;
    let input = fixed_input.as_path();

    let _label = input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    shared_utils::progress_mode::set_log_context(&_label);
    let _log_guard = shared_utils::progress_mode::LogContextGuard;

    // Check for Live Photos first (before any analysis)
    if shared_utils::is_live_photo(input) {
        let reason =
            "Live Photo detected - img strictly processes static images only (handled by vid)";
        shared_utils::progress_mode::image_ignored(reason);
        let file_size = shared_utils::io_utils::metadata_with_retry(input).map_or(0, |m| m.len());
        // [FIX] Completely ignore: NO COPY, NO STATS
        return Ok(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: false,
            ignored: true,
            message: reason.to_string(),
            original_size: file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        });
    }

    let analysis =
        shared_utils::image_analyzer::analyze_image_with_cache(input, config.cache.as_deref())?;

    // --- Strict Static Isolation: Skip all animated assets ---
    if analysis.is_animated {
        let reason =
            "Animated media detected - img strictly processes static images only (handled by vid)";
        shared_utils::progress_mode::image_ignored(reason);
        // [FIX] Completely ignore: NO COPY, NO STATS
        return Ok(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: false,
            ignored: true,
            message: reason.to_string(),
            original_size: analysis.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        });
    }

    // Single source of truth for static skip: JXL + modern lossy (avoid generational loss).
    // Always skip static JXL (already optimal format)
    if analysis.format.to_uppercase() == "JXL" {
        let reason =
            "Source is static JPEG XL (already optimal) - skipping to avoid generational loss";
        shared_utils::progress_mode::image_skipped(reason);
        copy_original_if_adjacent_mode(input, config)?;
        return Ok(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            ignored: false,
            message: reason.to_string(),
            original_size: analysis.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        });
    }

    let skip =
        shared_utils::should_skip_image_format(analysis.format.as_str(), analysis.is_lossless);
    if skip.should_skip {
        let reason = if let Some(err) = &analysis.analysis_error {
            format!("Analysis failed ({err}) - skipping to avoid generational loss")
        } else {
            skip.reason
        };
        shared_utils::progress_mode::image_skipped(&reason);
        copy_original_if_adjacent_mode(input, config)?;
        return Ok(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            ignored: false,
            message: reason,
            original_size: analysis.file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        });
    }

    let _pixel_analysis = if !analysis.is_animated && analysis.format != "JPEG" {
        shared_utils::image_quality_detector::analyze_image_quality_with_cache(
            input,
            config.cache.as_deref(),
        )
    } else {
        None
    };
    if let Some(ref q) = _pixel_analysis {
        shared_utils::log_media_info_for_image_quality(q, input);
    }

    let quality_label = analysis.quality_summary();

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
        child_threads: if config.child_threads > 0 {
            config.child_threads
        } else {
            2
        },
        input_format: Some(analysis.format.clone()),
        quality_label: Some(quality_label),
        codec: config.codec,
    };

    let result = dispatch_static_conversion(input, &analysis, &options, config)?;

    let output = convert_result_to_output(result);

    if output.skipped {
        if config.verbose {
            println!("⏭️ {}", output.message);
        }
    } else if output.is_jpeg_transcode() {
        shared_utils::verbose_eprintln!("{}", output.message);
    } else {
        shared_utils::log_eprintln!("{}", output.message);
    }

    Ok(output)
}

fn dispatch_static_conversion(
    input: &Path,
    analysis: &shared_utils::image_analyzer::ImageAnalysis,
    options: &img::lossless_converter::ConvertOptions,
    config: &AutoConvertConfig,
) -> anyhow::Result<shared_utils::ConversionResult> {
    use img::lossless_converter::{convert_jpeg_to_jxl, convert_to_jxl};

    let format = analysis.format.as_str();
    let is_lossless = analysis.is_lossless;

    // 🔬 Level 4 Feedback: KNN Static Quality Score
    // JPEG bypass: cjxl transcode is fast enough to skip DB lookup.
    // Returns a BPP heuristic (confidence=0.0) when DB is unavailable.
    let quality = if format == "JPEG" {
        None
    } else {
        shared_utils::lookup_image_quality(analysis)
    };

    if let Some(ref q) = quality {
        if config.verbose {
            if let Some(reason) = q.fallback_reason.as_deref() {
                println!(
                    "   🔭 Quality Score: {:.2} (BPP heuristic, reason: {reason})",
                    q.score
                );
            } else {
                println!(
                    "   🔭 Quality Score: {:.2} (KNN, conf={:.2})",
                    q.score, q.confidence
                );
            }
        }
    }

    Ok(match (format, is_lossless) {
        ("WebP" | "AVIF" | "TIFF" | "HEIC" | "HEIF", true) => {
            if format == "HEIC" || format == "HEIF" {
                if let Some(h) = &analysis.heic_analysis {
                    if h.has_gainmap {
                        println!("🌈 HDR Synthesis: {} (Gainmap detected)", input.display());
                        return Ok(img::lossless_converter::convert_heic_gainmap_to_jxl(
                            input, options,
                        )?);
                    }
                }
            }
            if config.verbose {
                println!("🔄 Modern Lossless→JXL: {}", input.display());
            }
            convert_to_jxl(input, options, 0.0_f32, analysis.hdr_info.as_ref())?
        }
        ("JPEG", _) => {
            use shared_utils::image_jpeg_analysis::is_ultra_hdr_jpeg_file;
            if is_ultra_hdr_jpeg_file(input) {
                println!(
                    "🌈 UltraHDR Migration: {} (Gainmap detected)",
                    input.display()
                );
                return Ok(img::lossless_converter::convert_ultrahdr_jpeg_to_jxl(
                    input, options,
                )?);
            }

            if config.verbose {
                println!("🔄 JPEG→JXL lossless transcode: {}", input.display());
            }
            convert_jpeg_to_jxl(input, options, analysis.hdr_info.as_ref())?
        }
        (_, true) => {
            if config.verbose {
                println!("🔄 Legacy Lossless→JXL: {}", input.display());
            }
            convert_to_jxl(input, options, 0.0_f32, analysis.hdr_info.as_ref())?
        }
        _ => {
            if config.verbose {
                println!(
                    "🔄 {} Lossy→JXL (Near-Lossless): {}",
                    match format.to_uppercase().as_str() {
                        "PNG" => "Quantized PNG",
                        "GIF" => "Static GIF",
                        _ => "Legacy",
                    },
                    input.display()
                );
            }
            convert_to_jxl(input, options, 0.001_f32, analysis.hdr_info.as_ref())?
        }
    })
}

fn auto_convert_directory(
    input: &Path,
    config: &AutoConvertConfig,
    recursive: bool,
    resume: bool,
) -> anyhow::Result<()> {
    // Check for Apple Photos library before any processing
    if let Err(e) = shared_utils::check_apple_photos_library(input) {
        eprintln!("{e}");
        std::process::exit(1);
    }

    if config.delete_original || config.in_place {
        if let Err(e) = check_dangerous_directory(input) {
            eprintln!("{e}");
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
        shared_utils::IMAGE_EXTENSIONS_FOR_CONVERT,
        recursive,
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
        println!("📂 Found {total} files to process");
        shared_utils::log_eprintln!(
            "⚡ Queue Strategy: deeper paths → fast JPEG/direct transcodes → smaller files → lower resolution"
        );
    }

    // Initialize checkpoint manager for resume/progress tracking
    let checkpoint = if resume {
        match shared_utils::checkpoint::CheckpointManager::new_with_context(
            input,
            config.output_dir.as_deref(),
        ) {
            Ok(cp) => {
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
                    println!("⚠️ [checkpoint] Failed to initialize: {e}");
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
            .map(|f| match shared_utils::io_utils::metadata_with_retry(f) {
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
                let avail_gb =
                    shared_utils::numeric_cast::u64_to_f64(avail) / (1024.0 * 1024.0 * 1024.0);
                let required_gb =
                    shared_utils::numeric_cast::u64_to_f64(required) / (1024.0 * 1024.0 * 1024.0);
                eprintln!(
                    "❌ Insufficient disk space on output volume.\n\
                     💾 Available: {avail_gb:.2} GB\n\
                     💾 Required:  {required_gb:.2} GB (input size + 1 GB headroom)\n\
                     💡 Free up space or choose a different output location."
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
    let ignored = AtomicUsize::new(0);
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
                .map_err(|e| anyhow::anyhow!("Failed to create fallback thread pool: {e}"))?
        }
    };

    if config.verbose {
        shared_utils::log_eprintln!(
            "🔧 Thread Strategy: {} parallel tasks x {} threads/task (CPU cores: {})",
            max_threads,
            child_threads,
            std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
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
                            if result.ignored {
                                ignored.fetch_add(1, Ordering::Relaxed);
                            } else if result.skipped {
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
                                            "⚠️ [img] Failed to mark completed {}: {}",
                                            path.display(),
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if let Some(reason) = disk_full_pause_reason(&err_str) {
                                if pause_controller.request_pause(path, reason.clone()) {
                                    shared_utils::log_eprintln!(
                                        "⏸️ [Batch] Paused at {}: {}",
                                        path.display(),
                                        reason
                                    );
                                }
                                continue;
                            } else {
                                // Classify as read/analysis failure only on unambiguous sentinel types
                                let is_read_error = err_str.contains("Failed to open file")
                                    || err_str.contains("ImageReadError");

                                if is_read_error {
                                    shared_utils::log_auto_error!(
                                        "Image analysis",
                                        "⚠️  Failed to read/analyze {}: {}. Original file will be preserved.",
                                        path.display(),
                                        e
                                    );
                                } else {
                                    shared_utils::log_auto_error!(
                                        "Image conversion",
                                        "Failed {}: {}. Preserved original (Skipped conversion).",
                                        path.display(),
                                        e
                                    );
                                }

                                shared_utils::progress_mode::log_conversion_failure(path, &err_str);
                                failed.fetch_add(1, Ordering::Relaxed);
                                shared_utils::progress_mode::image_processed_failure();

                                // Copy original file to output directory to prevent data loss
                                if let Some(ref output_dir) = config.output_dir {
                                    if let Err(copy_err) = shared_utils::copy_on_skip_or_fail(
                                        path,
                                        Some(output_dir),
                                        config.base_dir.as_deref(),
                                        config.verbose,
                                    ) {
                                        shared_utils::log_eprintln!(
                                            "🚨 [CRITICAL] Failed to copy original after conversion failure ({}): {}. DATA LOSS RISK!",
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
    let ignored_count = ignored.load(Ordering::Relaxed);
    let processed_count = processed.load(Ordering::Relaxed);

    let mut result = BatchResult::new();
    result.succeeded = success_count;
    result.failed = failed_count;
    result.skipped = skipped_count;
    // [FIX] Completely ignore: remove from total reported
    result.total = processed_count.saturating_sub(ignored_count);
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
