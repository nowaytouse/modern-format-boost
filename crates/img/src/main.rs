#![allow(
    clippy::multiple_crate_versions,
    reason = "Legitimate deviation from standard linting rules justified by specific project architecture."
)]
#![allow(
    unexpected_cfgs,
    reason = "macos_ui is an optional feature that may not be defined in all builds"
)]
use clap::{Parser, Subcommand};
use img::Rational;

use core::sync::atomic::{AtomicUsize, Ordering};
use img::{
    ConfigFlags, ConvertFlags, calculate_psnr, calculate_ssim, psnr_quality_description,
    ssim_quality_description,
};
use shared_utils::ToolBuilder;
use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::modern_ui::{colors, symbols};
use shared_utils::quality_matcher::SourceCodec;
use shared_utils::{
    PauseController, Summary, check_dangerous_directory, disk_full_pause_reason, log_anomaly,
    log_detail, log_failure, log_fatal, log_hint, log_skip, print_summary,
};
use std::path::{Path, PathBuf};
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

        #[arg(long, default_value_t = true)]
        preserve_timestamps: bool,

        #[arg(long, default_value_t = true)]
        preserve: bool,

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

    /// Batch ingest unannotated static image samples into `SQLite` database for Active Learning
    IngestSamples {
        #[arg(value_name = "INPUT_DIR")]
        input: PathBuf,

        #[arg(short, long)]
        label: Option<String>,
    },
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn main() -> anyhow::Result<()> {
    if let Err(e) = shared_utils::init_ghost_mode() {
        log_anomaly!(
            shared_utils::static_logs::messages::LABEL_GHOST_MODE,
            &e.to_string()
        );
    }

    if let Err(e) = shared_utils::logging::init("img", &shared_utils::logging::LogConfig::default())
    {
        log_anomaly!(
            shared_utils::static_logs::messages::LABEL_LOGGING,
            &e.to_string()
        );
    }

    // Initialize Ctrl+C guard for long-running batch operations
    shared_utils::ctrlc_guard::init();

    let cache = AnalysisCache::default_local()
        .map(Arc::new)
        .inspect_err(|e| {
            log_anomaly!(
                shared_utils::static_logs::messages::LABEL_CACHE,
                &e.to_string()
            );
        })
        .ok();

    if let Some(ref cache) = cache
        && let Err(e) = cache.cleanup_old_records(shared_utils::constants::CACHE_PRUNE_AGE_SECS)
    {
        log_anomaly!(
            shared_utils::static_logs::messages::LABEL_CACHE,
            &e.to_string()
        );
    }

    let cli = Cli::parse();

    // --- Unified Directory Locking (Ghost Mode & Mutex) ---
    // Extract input path from relevant commands to lock the directory ONLY if it involves destructive or interactive shared state.
    let input_to_lock = match &cli.command {
        Commands::Run {
            input,
            in_place,
            delete_original,
            ..
        } if *in_place || *delete_original => Some(input),
        Commands::Verify {
            original: input, ..
        }
        | Commands::RestoreTimestamps { source: input, .. }
        | Commands::LockCheck { input }
        | Commands::PathHash { input }
        | Commands::IngestSamples { input, .. } => Some(input),
        _ => None,
    };

    let _lock_guard = input_to_lock.map_or_else(
        || None,
        |input| {
            let input_abs = std::fs::canonicalize(input).unwrap_or_else(|_| input.clone());
            if input_abs.is_dir() {
                match shared_utils::acquire_dir_lock(&input_abs) {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        log_fatal!(
                            shared_utils::static_logs::messages::LABEL_LOCK,
                            &e.to_string()
                        );
                        std::process::exit(shared_utils::constants::EXIT_CODE_LOCK_FAILURE);
                    }
                }
            } else {
                None
            }
        },
    );
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
            preserve_timestamps,
            preserve,
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
                log_fatal!(
                    shared_utils::static_logs::messages::LABEL_CONFIG,
                    shared_utils::static_logs::messages::APPLE_COMPAT_HEVC,
                );
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }

            let flag_mode =
                match shared_utils::validate_flags_result_with_ultimate(shared_utils::FlagRequest {
                    base: shared_utils::FlagBase {
                        explore,
                        match_quality,
                        compress,
                    },
                    tier: shared_utils::FlagTier { ultimate },
                }) {
                    Ok(mode) => mode,
                    Err(e) => {
                        log_fatal!(shared_utils::static_logs::messages::LABEL_CONFIG, &e);
                        std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
                    }
                };

            // Fail-fast if critical sub-tools are missing
            if let Err(e) = shared_utils::tools::require(&["cjxl", "djxl", "exiftool", "ffmpeg"]) {
                log_fatal!(shared_utils::static_logs::messages::LABEL_TOOLS, &e);
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }

            shared_utils::progress_mode::set_verbose_mode(verbose);
            // Create run log first; all subsequent output is captured here
            if let Err(e) = shared_utils::progress_mode::set_default_run_log_file("img") {
                log_anomaly!(
                    shared_utils::static_logs::messages::LABEL_RUN_LOG,
                    shared_utils::static_logs::messages::RUN_LOG_OPEN_FAIL
                );
                log_detail!(&format!("Detailed run log failure: {e}"));
            }
            if verbose {
                log_detail!(&format!(
                    "{} {} ({} for animated→video)",
                    symbols::VIDEO,
                    flag_mode.description_en(),
                    selected_codec.as_str().to_uppercase()
                ));
                log_detail!(&format!(
                    "{} Static: JPEG→JXL (reconstruct) │ Modern Lossless→JXL (d=0.0) │ PNG/Legacy→JXL (d=0.0/0.001)",
                    symbols::IMAGE
                ));
            }
            if apple_compat {
                log_detail!(&format!(
                    "{} Apple Compatibility: {}ENABLED{} (WebP→HEVC)",
                    symbols::SHIELD,
                    colors::BOLD,
                    colors::RESET
                ));
                unsafe { std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1") };
            }

            if in_place {
                log_detail!(&format!(
                    "{} In-place mode: {}ENABLED{} (auto-delete original)",
                    symbols::SAVE,
                    colors::BOLD,
                    colors::RESET
                ));
            }
            if ultimate {
                log_detail!(&format!(
                    "{} Ultimate Explore: {}ENABLED{} (max SSIM mode)",
                    symbols::SEARCH,
                    colors::BOLD,
                    colors::RESET
                ));
            }
            if !allow_size_tolerance {
                log_detail!(&format!(
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
                flags: {
                    // Batch flag construction using bitwise OR for optimal performance
                    ConfigFlags::empty()
                        | if force {
                            ConfigFlags::FORCE
                        } else {
                            ConfigFlags::empty()
                        }
                        | if should_delete {
                            ConfigFlags::DELETE_ORIGINAL
                        } else {
                            ConfigFlags::empty()
                        }
                        | if preserve_timestamps {
                            ConfigFlags::PRESERVE_TIMESTAMPS
                        } else {
                            ConfigFlags::empty()
                        }
                        | if preserve {
                            ConfigFlags::PRESERVE_METADATA
                        } else {
                            ConfigFlags::empty()
                        }
                        | if compress {
                            ConfigFlags::COMPRESS
                        } else {
                            ConfigFlags::empty()
                        }
                        | if apple_compat {
                            ConfigFlags::APPLE_COMPAT
                        } else {
                            ConfigFlags::empty()
                        }
                        | if in_place {
                            ConfigFlags::IN_PLACE
                        } else {
                            ConfigFlags::empty()
                        }
                        | if explore {
                            ConfigFlags::EXPLORE_SMALLER
                        } else {
                            ConfigFlags::empty()
                        }
                        | if match_quality {
                            ConfigFlags::MATCH_QUALITY
                        } else {
                            ConfigFlags::empty()
                        }
                        | ConfigFlags::USE_GPU
                        | if ultimate {
                            ConfigFlags::ULTIMATE_MODE
                        } else {
                            ConfigFlags::empty()
                        }
                        | if allow_size_tolerance {
                            ConfigFlags::ALLOW_SIZE_TOLERANCE
                        } else {
                            ConfigFlags::empty()
                        }
                        | if verbose {
                            ConfigFlags::VERBOSE
                        } else {
                            ConfigFlags::empty()
                        }
                },
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
                log_fatal!(
                    "Input",
                    &format!("Input path does not exist: {}", input.display()),
                );
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }
        }

        Commands::RestoreTimestamps { source, output } => {
            if let Err(e) = shared_utils::restore_timestamps_from_source_to_output(&source, &output)
            {
                log_anomaly!("Timestamp Restore", &e.to_string());
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
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
                        shared_utils::log_summary_header!("Cache");
                        shared_utils::log_stat!(
                            "Database Size",
                            "{:.2} MB ({:.3} GB)",
                            stats.db_size_mb(),
                            stats.db_size_gb()
                        );
                        shared_utils::log_stat!("Total Records", stats.total_records());
                        shared_utils::log_stat!("Analysis", stats.analysis_records);
                        shared_utils::log_stat!("Quality", stats.quality_records);
                        shared_utils::log_stat!("Video", stats.video_records);
                        shared_utils::log_stat!("Path Index", stats.path_index_entries);

                        shared_utils::log_detail!("");
                        shared_utils::log_stat!(
                            "Schema Version",
                            format!("v{}", stats.schema_version)
                        );
                        shared_utils::log_stat!(
                            "Current Algorithm",
                            format!("v{}", stats.current_algorithm_version)
                        );

                        if !stats.algorithm_version_distribution.is_empty() {
                            shared_utils::log_detail!("");
                            shared_utils::log_detail!("📈 Algorithm Version Distribution:");
                            let mut versions: Vec<_> =
                                stats.algorithm_version_distribution.iter().collect();
                            versions.sort_by_key(|(v, _)| *v);
                            for (version, count) in versions {
                                let marker = match (*version).cmp(&stats.current_algorithm_version)
                                {
                                    core::cmp::Ordering::Less => "⚠️  (stale)",
                                    core::cmp::Ordering::Equal => "✅ (current)",
                                    core::cmp::Ordering::Greater => "❓ (future)",
                                };
                                shared_utils::log_detail!(&format!(
                                    "   v{version}: {count} records {marker}"
                                ));
                            }

                            let stale = stats.stale_records();
                            if stale > 0 {
                                shared_utils::log_hint!(
                                    "Stale Data",
                                    &format!(
                                        "{stale} stale records detected (will be auto-invalidated on next run)"
                                    )
                                );
                            }
                        }

                        let permille = {
                            let ratio = Rational::from(stats.db_size_bytes)
                                / Rational::from(
                                    shared_utils::analysis_cache::CACHE_SIZE_LIMIT_BYTES.max(1),
                                );
                            let res: Rational = ratio * Rational::from(10_000);
                            res.to_f64()
                        };
                        let usage_percent = permille / 100.0;
                        shared_utils::log_detail!("");
                        shared_utils::log_stat!(
                            "Storage Usage",
                            "{:.1}% of {} GB limit",
                            usage_percent,
                            shared_utils::constants::CACHE_SIZE_LIMIT_BYTES / 1024 / 1024 / 1024
                        );

                        if usage_percent > shared_utils::constants::CACHE_USAGE_WARNING_THRESHOLD {
                            shared_utils::log_anomaly!("Cache", "Approaching size limit!");
                        }

                        shared_utils::log_detail!("═══════════════════════════════════════");
                    }
                    Err(e) => {
                        log_fatal!("Cache Stats", &e.to_string());
                        std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
                    }
                }
            } else {
                log_fatal!("Cache", "Cache is not initialized");
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }
        }

        Commands::LockCheck { input } => {
            let input_abs = std::fs::canonicalize(&input).unwrap_or_else(|_| input.clone());
            if input_abs.is_dir() {
                // Try to acquire lock. If it fails, report and exit immediately with code 3.
                match shared_utils::acquire_dir_lock(&input_abs) {
                    Ok(_lock) => {
                        shared_utils::log_success!(
                            "Directory Lock",
                            "Directory is available for processing."
                        );
                    }
                    Err(e) => {
                        log_fatal!("Directory Lock", &e.to_string());
                        std::process::exit(shared_utils::constants::EXIT_CODE_LOCK_FAILURE);
                    }
                }
            }
        }

        Commands::PathHash { input } => {
            let hash = shared_utils::hash_path_to_hex(&input).unwrap_or_else(|_| "err".to_string());
            shared_utils::log_detail!(&hash);
        }

        Commands::IngestSamples { input, label } => {
            let mut conn = shared_utils::database::open_pg_client()?;
            shared_utils::image_quality_db::init_quality_schema(&mut conn)?;

            let mut count = 0;
            let mut dirs_to_visit = vec![input];

            while let Some(dir) = dirs_to_visit.pop() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            dirs_to_visit.push(path);
                        } else if path.is_file() {
                            let ext = path
                                .extension()
                                .and_then(|s| s.to_str())
                                .unwrap_or("")
                                .to_lowercase();

                            if [
                                "jpg", "jpeg", "png", "heic", "heif", "jxl", "tiff", "bmp", "webp",
                            ]
                            .contains(&ext.as_str())
                            {
                                let default_label =
                                    label.clone().unwrap_or_else(|| "low".to_string());
                                if let Err(e) =
                                    shared_utils::image_quality_db::ingest_quality_sample(
                                        &mut conn,
                                        &path,
                                        &default_label,
                                        "fusion_v1",
                                    )
                                {
                                    log_anomaly!(
                                        "Ingest",
                                        &format!("Failed to ingest {}: {}", path.display(), e),
                                    );
                                } else {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
            shared_utils::log_success!(
                "Ingest",
                &format!("Successfully ingested {count} static image samples.")
            );
        }
    }

    {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            #[allow(
            unexpected_cfgs,
            reason = "macos_ui is an optional feature that may not be defined in all builds"
        )]
        #[allow(
            unexpected_cfgs,
            reason = "macos_ui is an optional feature that may not be defined in all builds"
        )]
        #[cfg(all(target_os = "macos", feature = "macos_ui"))]
            {
                shared_utils::macos_ui::wait_for_exit_confirmation();
            }
        }
    }

    Ok(())
}

fn verify_conversion(
    original: &std::path::Path,
    converted: &std::path::Path,
    cache: Option<&AnalysisCache>,
) -> anyhow::Result<()> {
    log_detail!("🔍 Verifying conversion quality...");
    log_detail!(&format!("Original:  {}", original.display()));
    log_detail!(&format!("Converted: {}", converted.display()));

    let original_analysis =
        shared_utils::image_analyzer::analyze_image_with_cache(original, cache)?;
    let converted_analysis =
        shared_utils::image_analyzer::analyze_image_with_cache(converted, cache)?;

    log_detail!("📊 Size Comparison:");
    log_detail!(&format!(
        "Original size:  {} bytes ({:.2} KB)",
        original_analysis.file_size,
        shared_utils::numeric_cast::u64_to_f64(original_analysis.file_size)
            / shared_utils::numeric_cast::u64_to_f64(shared_utils::constants::BYTES_PER_KB)
    ));
    log_detail!(&format!(
        "Converted size: {} bytes ({:.2} KB)",
        converted_analysis.file_size,
        shared_utils::numeric_cast::u64_to_f64(converted_analysis.file_size) / 1024.0
    ));

    let reduction = 100.0
        * (1.0
            - shared_utils::numeric_cast::u64_to_f64(converted_analysis.file_size)
                / shared_utils::numeric_cast::u64_to_f64(original_analysis.file_size));
    log_detail!(&format!("Size reduction: {reduction:.2}%"));

    let orig_img = load_image_safe(original)?;
    let conv_img = load_image_safe(converted)?;

    log_detail!("📏 Quality Metrics:");
    if let Some(psnr) = calculate_psnr(&orig_img, &conv_img) {
        if psnr.is_infinite() {
            log_detail!("PSNR: ∞ dB (Identical - mathematically lossless)");
        } else {
            log_detail!(&format!(
                "PSNR: {:.2} dB ({})",
                psnr,
                psnr_quality_description(psnr)
            ));
        }
    }

    if let Some(ssim) = calculate_ssim(&orig_img, &conv_img) {
        log_detail!(&format!(
            "SSIM: {:.6} ({})",
            ssim,
            ssim_quality_description(ssim)
        ));
    }

    log_detail!("✅ Verification complete");

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
    flags: ConfigFlags,
    child_threads: usize,

    cache: Option<Arc<AnalysisCache>>,
    codec: shared_utils::conversion_types::SelectedCodec,
}

impl AutoConvertConfig {
    const fn force(&self) -> bool {
        self.flags.contains(ConfigFlags::FORCE)
    }
    const fn delete_original(&self) -> bool {
        self.flags.contains(ConfigFlags::DELETE_ORIGINAL)
    }
    const fn compress(&self) -> bool {
        self.flags.contains(ConfigFlags::COMPRESS)
    }
    const fn apple_compat(&self) -> bool {
        self.flags.contains(ConfigFlags::APPLE_COMPAT)
    }
    const fn in_place(&self) -> bool {
        self.flags.contains(ConfigFlags::IN_PLACE)
    }
    const fn explore(&self) -> bool {
        self.flags.contains(ConfigFlags::EXPLORE_SMALLER)
    }
    const fn match_quality(&self) -> bool {
        self.flags.contains(ConfigFlags::MATCH_QUALITY)
    }
    const fn use_gpu(&self) -> bool {
        self.flags.contains(ConfigFlags::USE_GPU)
    }
    const fn ultimate(&self) -> bool {
        self.flags.contains(ConfigFlags::ULTIMATE_MODE)
    }
    const fn allow_size_tolerance(&self) -> bool {
        self.flags.contains(ConfigFlags::ALLOW_SIZE_TOLERANCE)
    }
    const fn verbose(&self) -> bool {
        self.flags.contains(ConfigFlags::VERBOSE)
    }
}

fn copy_original_if_adjacent_mode(input: &Path, config: &AutoConvertConfig) -> anyhow::Result<()> {
    shared_utils::copy_on_skip_or_fail(
        input,
        config.output_dir.as_deref(),
        config.base_dir.as_deref(),
        config.verbose(),
    )?;
    Ok(())
}

use img::conversion_api::ConversionOutput;

fn convert_result_to_output(result: shared_utils::TaskResult) -> ConversionOutput {
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

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn auto_convert_single_file(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<ConversionOutput> {
    use img::lossless_converter::ConvertOptions;

    // Pause if the user is being prompted to exit via Ctrl+C
    shared_utils::ctrlc_guard::wait_if_prompt_active();

    // Check for Apple Photos library before processing
    if let Err(e) = shared_utils::check_apple_photos_library(input) {
        log_fatal!("Apple Photos", &e);
        std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
    }

    if let Some(ref out_dir) = config.output_dir
        && let Err(e) = shared_utils::check_apple_photos_library(out_dir)
    {
        log_fatal!("Apple Photos", &e);
        std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
    }

    // Fix extension by content first so all downstream checks see the real format (avoids disguised-extension panic).
    // When an output directory is configured the source tree must remain immutable:
    // use the readonly variant that logs mismatches without renaming source files.
    let fixed_input = if config.output_dir.is_some() {
        shared_utils::check_extension_mismatch_readonly(input)?
    } else {
        shared_utils::fix_extension_if_mismatch(input)?
    };
    let input = fixed_input.as_path();

    let label = input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    shared_utils::progress_mode::set_log_context(&label);
    let _log_guard = shared_utils::progress_mode::LogContextGuard;

    // Check for Live Photos first (before any analysis)
    // Only skip in Apple compat mode to preserve the pair association.
    // In normal mode, we treat the HEIC as a regular image to be upgraded.
    if config.apple_compat() && shared_utils::live_photo::is_live(input) {
        let reason =
            "Live Photo detected in Apple compat mode - skipping to preserve pair (handled by vid)";
        shared_utils::progress_mode::image_skipped(reason);
        let file_size = shared_utils::io_utils::metadata_with_retry(input).map_or_else(
            |e| {
                log_anomaly!(
                    "Metadata",
                    &format!(
                        "Failed to read metadata for {}; defaulting to size 0. Error: {e}",
                        input.display()
                    ),
                );
                0
            },
            |m| m.len(),
        );
        copy_original_if_adjacent_mode(input, config)?;
        return Ok(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            ignored: false,
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

    let quality_label = analysis.quality_summary();

    let options = ConvertOptions {
        output_dir: config.output_dir.clone(),
        base_dir: config.base_dir.clone(),
        flags: {
            // Batch flag construction using bitwise OR for optimal performance
            ConvertFlags::empty()
                | if config.force() {
                    ConvertFlags::FORCE
                } else {
                    ConvertFlags::empty()
                }
                | if config.delete_original() {
                    ConvertFlags::DELETE_ORIGINAL
                } else {
                    ConvertFlags::empty()
                }
                | if config.in_place() {
                    ConvertFlags::IN_PLACE
                } else {
                    ConvertFlags::empty()
                }
                | if config.explore() {
                    ConvertFlags::EXPLORE
                } else {
                    ConvertFlags::empty()
                }
                | if config.match_quality() {
                    ConvertFlags::MATCH_QUALITY
                } else {
                    ConvertFlags::empty()
                }
                | if config.compress() {
                    ConvertFlags::COMPRESS
                } else {
                    ConvertFlags::empty()
                }
                | if config.apple_compat() {
                    ConvertFlags::APPLE_COMPAT
                } else {
                    ConvertFlags::empty()
                }
                | if config.use_gpu() {
                    ConvertFlags::USE_GPU
                } else {
                    ConvertFlags::empty()
                }
                | if config.ultimate() {
                    ConvertFlags::ULTIMATE
                } else {
                    ConvertFlags::empty()
                }
                | if config.allow_size_tolerance() {
                    ConvertFlags::ALLOW_SIZE_TOLERANCE
                } else {
                    ConvertFlags::empty()
                }
                | if config.verbose() {
                    ConvertFlags::VERBOSE
                } else {
                    ConvertFlags::empty()
                }
                | if config.allow_size_tolerance() {
                    ConvertFlags::ALLOW_SIZE_TOLERANCE
                } else {
                    ConvertFlags::empty()
                }
                | if config.verbose() {
                    ConvertFlags::VERBOSE
                } else {
                    ConvertFlags::empty()
                }
        },
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
        if config.verbose() {
            log_skip!(&label, &output.message);
        }
    } else {
        log_detail!(&output.message);
    }

    Ok(output)
}

fn dispatch_static_conversion(
    input: &Path,
    analysis: &shared_utils::image_analyzer::ImageAnalysis,
    options: &img::lossless_converter::ConvertOptions,
    config: &AutoConvertConfig,
) -> anyhow::Result<shared_utils::TaskResult> {
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

    if let Some(ref q) = quality
        && config.verbose()
    {
        if let Some(reason) = q.fallback_reason.as_deref() {
            log_detail!(&format!(
                "🔭 Quality Score: {:.2} (BPP heuristic, reason: {reason})",
                q.score
            ));
        } else {
            log_detail!(&format!(
                "🔭 Quality Score: {:.2} (KNN, conf={:.2})",
                q.score, q.confidence
            ));
        }
    }

    Ok(match (format, is_lossless) {
        ("WebP" | "AVIF" | "TIFF" | "HEIC" | "HEIF", true) => {
            if (format == "HEIC" || format == "HEIF")
                && let Some(h) = &analysis.heic_analysis
                && h.hdr.has_gainmap
            {
                log_detail!(&format!(
                    "🌈 HDR Synthesis: {} (Gainmap detected)",
                    input.display()
                ));
                return Ok(img::lossless_converter::convert_heic_gainmap_to_jxl(
                    input, options,
                )?);
            }
            if config.verbose() {
                log_detail!(&format!("🔄 Modern Lossless→JXL: {}", input.display()));
            }
            convert_to_jxl(input, options, 0.0_f32, analysis.hdr_info.as_ref())?
        }
        ("JPEG", _) => {
            use shared_utils::image_jpeg_analysis::is_ultra_hdr_jpeg_file;
            if is_ultra_hdr_jpeg_file(input) {
                log_detail!(&format!(
                    "🌈 UltraHDR Migration: {} (Gainmap detected)",
                    input.display()
                ));
                return Ok(img::lossless_converter::convert_ultrahdr_jpeg_to_jxl(
                    input, options,
                )?);
            }

            if config.verbose() {
                log_detail!(&format!(
                    "🔄 JPEG→JXL lossless transcode: {}",
                    input.display()
                ));
            }
            convert_jpeg_to_jxl(input, options, analysis.hdr_info.as_ref())?
        }
        (_, true) => {
            if config.verbose() {
                log_detail!(&format!("🔄 Legacy Lossless→JXL: {}", input.display()));
            }
            convert_to_jxl(input, options, 0.0_f32, analysis.hdr_info.as_ref())?
        }
        _ => {
            if config.verbose() {
                log_detail!(&format!(
                    "🔄 {} Lossy→JXL (Near-Lossless): {}",
                    match format.to_uppercase().as_str() {
                        "PNG" => "Quantized PNG",
                        "GIF" => "Static GIF",
                        _ => "Legacy",
                    },
                    input.display()
                ));
            }
            convert_to_jxl(
                input,
                options,
                shared_utils::constants::JXL_ULTIMATE_DISTANCE,
                analysis.hdr_info.as_ref(),
            )?
        }
    })
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn auto_convert_directory(
    input: &Path,
    config: &AutoConvertConfig,
    recursive: bool,
    resume: bool,
) -> anyhow::Result<()> {
    // Check for Apple Photos library before any processing
    if let Err(e) = shared_utils::check_apple_photos_library(input) {
        log_detail!("{e}");
        std::process::exit(1);
    }

    if let Some(ref out_dir) = config.output_dir
        && let Err(e) = shared_utils::check_apple_photos_library(out_dir)
    {
        log_detail!("{e}");
        std::process::exit(1);
    }

    if (config.delete_original() || config.in_place())
        && let Err(e) = check_dangerous_directory(input)
    {
        log_detail!("{e}");
        std::process::exit(1);
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
            log_anomaly!(
                "Metadata",
                &format!(
                    "Failed to snapshot directory timestamps for {}: {}",
                    input.display(),
                    e
                ),
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
        shared_utils::log_detail!(&format!("📂 No image files found in {}", input.display()));

        if let Some(output_dir) = config.output_dir.as_ref()
            && let Some(ref base_dir) = config.base_dir
        {
            shared_utils::preserve_directory_with_log(base_dir, output_dir);
        }

        return Ok(());
    }

    if config.verbose() {
        shared_utils::log_info!("Setup", &format!("Found {total} files to process"));
        log_detail!(
            "⚡ Queue Strategy: deeper paths → fast JPEG/direct transcodes → smaller files → lower resolution",
        );
    }

    // Initialize checkpoint manager for resume/progress tracking
    let checkpoint = if resume {
        match shared_utils::checkpoint::Manager::new_with_context(
            input,
            config.output_dir.as_deref(),
        ) {
            Ok(cp) => {
                if cp.is_resume_mode() {
                    if config.verbose() {
                        shared_utils::log_info!(
                            "Resume",
                            &format!("skipping {} already completed images", cp.completed_count())
                        );
                    }
                    cp.sync_to_processed_list();
                } else {
                    shared_utils::clear_processed_list();
                }
                Some(cp)
            }
            Err(e) => {
                if config.verbose() {
                    shared_utils::log_anomaly!("Checkpoint", &format!("Failed to initialize: {e}"));
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
                    log_anomaly!(
                        "Disk Precheck",
                        &format!("Failed to read file metadata ({}): {}", f.display(), err),
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
                log_detail!(
                    "❌ Insufficient disk space on output volume.\n\
                     💾 Available: {avail_gb:.2} GB\n\
                     💾 Required:  {required_gb:.2} GB (input size + 1 GB headroom)\n\
                     💡 Free up space or choose a different output location.",
                );
                std::process::exit(1);
            }
            if config.verbose() {
                shared_utils::log_info!(
                    "Disk Space",
                    &format!(
                        "OK: {:.2} GB available, {:.2} GB required",
                        shared_utils::numeric_cast::u64_to_f64(avail) / (1024.0 * 1024.0 * 1024.0),
                        shared_utils::numeric_cast::u64_to_f64(required)
                            / (1024.0 * 1024.0 * 1024.0)
                    )
                );
            }
        }
    }

    let success = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let ignored = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let actual_input_bytes = core::sync::atomic::AtomicU64::new(0);
    let actual_output_bytes = core::sync::atomic::AtomicU64::new(0);
    let pause_controller = Arc::new(PauseController::new());

    shared_utils::progress_mode::enable_quiet_mode();
    let progress_bar = Arc::new(shared_utils::CoarseProgressBar::new(
        shared_utils::numeric_cast::usize_to_u64(total),
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
            log_anomaly!(
                "Thread Pool",
                &format!(
                    "Failed to create {max_threads} thread pool, falling back to 2 threads: {e}"
                ),
            );
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to create fallback thread pool: {e}"))?
        }
    };

    if config.verbose() {
        log_detail!(&format!(
            "🔧 Thread Strategy: {} parallel tasks x {} threads/task (CPU cores: {})",
            max_threads,
            child_threads,
            std::thread::available_parallelism().map_or(4, core::num::NonZero::get)
        ));
        if let Some(hint) = shared_utils::thread_manager::memory_cap_hint() {
            log_hint!(hint);
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

                    let Some(path) = files.get(index) else {
                        break;
                    };
                    progress_bar.set_message(&path.file_name().unwrap_or_default().to_string_lossy());

                    // Check if already completed (thread-safe)
                    if let Some(cp) = checkpoint.as_ref()
                        && cp.is_completed(path) {
                            skipped.fetch_add(1, Ordering::Relaxed);
                            let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                            shared_utils::progress_mode::write_progress_line_to_run_log(
                                start_time.elapsed().as_secs(),
                                shared_utils::numeric_cast::usize_to_u64(current),
                                shared_utils::numeric_cast::usize_to_u64(total),
                                &path.file_name().unwrap_or_default().to_string_lossy(),
                            );
                            progress_bar.set(shared_utils::numeric_cast::usize_to_u64(current));
                            continue;
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
                                if let Some(cp) = checkpoint.as_ref()
                                    && let Err(e) = cp.mark_completed(path) {
                                        log_anomaly!("Checkpoint", &format!("Failed to mark completed {}: {}", path.display(), e));
                                    }
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if let Some(reason) = disk_full_pause_reason(&err_str) {
                                if pause_controller.request_pause(path, reason.clone()) {
                                    shared_utils::log_detail!(
                                        "⏸️ [Batch] Paused at {}: {}",
                                        path.display(),
                                        reason
                                    );
                                }
                                continue;
                            }

                            let is_skip = e.downcast_ref::<shared_utils::unified_error::UnifiedError>().map_or_else(
                                || err_str.contains("Skipped") || err_str.contains("already optimized"),
                                shared_utils::unified_error::UnifiedError::is_skip
                            );

                            if is_skip {
                                log_skip!(
                                    &path.file_name().unwrap_or_default().to_string_lossy(),
                                    &err_str
                                );
                                skipped.fetch_add(1, Ordering::Relaxed);
                                shared_utils::progress_mode::image_processed_success(); // Skip with copy is a partial success

                                // Copy original file to output directory to prevent data loss for skips
                                if let Some(ref output_dir) = config.output_dir
                                    && let Err(copy_err) = shared_utils::copy_on_skip_or_fail(
                                        path,
                                        Some(output_dir),
                                        config.base_dir.as_deref(),
                                        config.verbose(),
                                    ) {
                                        log_fatal!(
                                            "Critical Data Link",
                                            &format!("Failed to copy original after skip ({}): {}. DATA LOSS RISK!", path.display(), copy_err)
                                        );
                                    }
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
                                        "Failed {}: {}. Output discarded (Hard Error).",
                                        path.display(),
                                        e
                                    );
                                }

                                shared_utils::progress_mode::log_conversion_failure(path, &err_str);
                                failed.fetch_add(1, Ordering::Relaxed);
                                shared_utils::progress_mode::image_processed_failure();
                            }
                        }
                    }
                    let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    shared_utils::progress_mode::write_progress_line_to_run_log(
                        start_time.elapsed().as_secs(),
                        shared_utils::numeric_cast::usize_to_u64(current),
                        shared_utils::numeric_cast::usize_to_u64(total),
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                    );
                    progress_bar.set(shared_utils::numeric_cast::usize_to_u64(current));
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

    let mut result = Summary::new();
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

    print_summary(
        &result,
        start_time.elapsed(),
        final_input_bytes,
        final_output_bytes,
        "Image Conversion",
    );

    if !result.paused
        && let Some(ref output_dir) = config.output_dir
    {
        log_detail!("");
        shared_utils::log_static!(
            info,
            shared_utils::static_logs::messages::COPYING_UNSUPPORTED
        );
        let copy_result = shared_utils::copy_unsupported_files(
            config.base_dir.as_deref().unwrap_or_else(|| Path::new(".")),
            output_dir,
            recursive,
        );
        if copy_result.copied > 0 {
            log_detail!(&format!("Copied {} unsupported files", copy_result.copied));
        }
        if copy_result.failed > 0 {
            log_failure!(
                "Unsupported Files",
                &format!("Failed to copy {} files", copy_result.failed),
            );
        }

        log_detail!("");
        shared_utils::log_static!(info, shared_utils::static_logs::messages::OUTPUT_VERIFY);
        let verify = shared_utils::verify_output_completeness(
            config.base_dir.as_deref().unwrap_or_else(|| Path::new(".")),
            output_dir,
            recursive,
        );
        log_detail!(&verify.message);
        if !verify.passed {
            log_anomaly!(
                "Verification",
                "File count mismatch between input and output directories; some files may have been lost",
            );
        }
    }

    if !result.paused
        && let Some(ref output_dir) = config.output_dir
        && let Some(ref base_dir) = config.base_dir
    {
        log_detail!("");
        shared_utils::log_static!(info, shared_utils::static_logs::messages::METADATA_SYNC);
        shared_utils::preserve_directory_with_log(base_dir, output_dir);
    }

    if let Some(ref saved) = saved_dir_timestamps {
        if !result.paused
            && let Some(ref output_dir) = config.output_dir
            && let Some(ref base_dir) = config.base_dir
        {
            shared_utils::apply_saved_timestamps_to_dst(saved, base_dir, output_dir);
        }
        shared_utils::restore_directory_timestamps(saved);
        log_detail!("✅ Directory timestamps restored");
    }

    // Finalize checkpoint only on 100% success
    if let Some(cp) = checkpoint {
        if result.paused {
            if let Err(e) = cp.release_lock() {
                log_anomaly!("Checkpoint Lock", &format!("Release lock failed: {e}"));
            }
        } else if failed_count == 0 {
            if let Err(e) = cp.cleanup() {
                log_anomaly!("Checkpoint Cleanup", &format!("Cleanup failed: {e}"));
            }
        } else if let Err(e) = cp.release_lock() {
            log_anomaly!("Checkpoint Lock", &format!("Release lock failed: {e}"));
        }
    }

    {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            #[allow(
            unexpected_cfgs,
            reason = "macos_ui is an optional feature that may not be defined in all builds"
        )]
        #[allow(
            unexpected_cfgs,
            reason = "macos_ui is an optional feature that may not be defined in all builds"
        )]
        #[cfg(all(target_os = "macos", feature = "macos_ui"))]
            {
                shared_utils::macos_ui::wait_for_exit_confirmation();
            }
        }
    }

    Ok(())
}
