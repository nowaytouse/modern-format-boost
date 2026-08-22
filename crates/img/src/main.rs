use anyhow::Context;
use clap::{Parser, Subcommand};
use img::Rational;
use img::lossless_converter::{
    ConvertFlags as LosslessConvertFlags, ConvertOptions as LosslessConvertOptions,
    convert_jpeg_to_jxl,
};

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use foundation::ToolBuilder;
use foundation::analysis_cache::AnalysisCache;
use foundation::common_utils::calculate_blake3_hash;
use foundation::fast_img::{
    IntegrityResult, PhotosImportCandidate, apply_library_assets_to_marker,
    apply_tier2_library_assets_to_marker, build_fast_img_output_import_candidates,
    delete_verified_modern_lossy_static_sources,
    import_media_outputs_with_checkpointed_library_verifier, import_modern_lossy_static_tier,
    prune_empty_source_dirs_for_tier2_assets, reverify_media_outputs_with_library_verifier,
    safe_delete_jpeg_source, safe_delete_matching_xmp_sidecar,
    verify_final_avif_delivery_integrity, verify_final_jxl_delivery_integrity,
};
use foundation::image::format_detect::FormatKind;
use foundation::image::orientation::{
    PixelDiffResult, pixel_equivalence_diff_tolerance_for_format, verify_orientation_pixel_diff,
};
use foundation::modern_ui::{colors, symbols};
use foundation::pipeline::verification::{
    Blake3Entry, FastImgStageName, Gate1Checks, Gate1Local, Gate2Checks, Gate2Import, Gate3Checks,
    Gate3Deep, LibraryHandle, PipelineCtx, SkippedSourceEntry, VerificationGate, WorkingCopyMarker,
    deep_scan_complete_or_later, encode_complete_or_later, gate1_complete_or_later,
    gate2_complete_or_later, gate3_complete_or_later, import_complete_or_later,
    marker_checks_from_result, marker_path_for_working_copy, output_prepared_or_later,
    prepare_jxl_output_dir, read_marker, resolve_working_copy_dir, retry_resume_stage,
    working_copy_dir, write_marker_atomic,
};
use foundation::quality_matcher::SourceCodec;
use foundation::{
    ModernLossyStaticCandidate, PauseController, Summary, check_dangerous_directory,
    disk_full_pause_reason, log_detail, log_failure, log_fatal, log_hint, log_skip, log_stat,
    log_success, log_summary_header, print_summary, scan_modern_lossy_static_candidates,
};
use img::{
    ConfigFlags, calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description,
};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

        /// Archive mode: hard-overrides encoder effort/presets for maximum compression.
        #[arg(long, default_value_t = false)]
        archive: bool,

        #[arg(long, default_value_t = false)]
        allow_size_tolerance: bool,

        #[arg(long)]
        no_allow_size_tolerance: bool,

        /// Enable explicitly gated lab-only encoder/decoder fallbacks. Final verification is unchanged.
        #[arg(long = "allow_expert_options", default_value_t = false)]
        allow_expert_options: bool,

        #[arg(long, default_value_t = true)]
        preserve_timestamps: bool,

        #[arg(long, default_value_t = true)]
        preserve: bool,

        #[arg(short, long, default_value_t = true)]
        verbose: bool,

        /// ASCII symbols, no decorative ANSI (also respects `NO_COLOR` / `MODERN_FORMAT_PLAIN_UI=1`).
        #[arg(long, default_value_t = false)]
        plain: bool,

        /// Resume from last run: skip files already in progress file.
        #[arg(long, default_value_t = false)]
        resume: bool,

        /// Start fresh: ignore previous progress file, process all files.
        #[arg(long)]
        no_resume: bool,

        /// Static still delivery: `hevc`→JXL. `av1` is rejected; AVIF is reserved for fast-img Meme Mode.
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

    /// Perform deep diagnostic scan of the database infrastructure and data integrity
    DbHealth,

    /// Fast image encoding: true JPEGs → reversible JXL, or static images → AVIF.
    ///
    /// Detects true JPEGs via content identity, strips residual EXIF Orientation
    /// after encode, and deletes sources only after verification. JXL mode also
    /// delivers confirmed-lossy static WebP/JP2/JXL/AVIF/HEIC originals to
    /// Photos with UUID/hash custody proof; uncertain or lossless originals remain.
    ///
    /// Locked decisions: D1=Photos import required, D2=abort on delete failure,
    /// D3=pixel-diff plus tag assert, D4=Rust-only, D5=subcommand,
    /// D6=verified source delete mandatory, D7=JPEG-path-only detection.
    #[command(name = "fast-img")]
    FastImg {
        /// Input directory or single static image. JXL encoding accepts true JPEG; eligible
        /// modern lossy static originals use the verified Photos-delivery tier.
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Deprecated for Rev2 fast mode; working-copy output is derived from input.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Deprecated for fast-img; verified source cleanup is mandatory.
        #[arg(long, default_value_t = false)]
        delete_source: bool,

        /// Dry-run: no writes, no deletions.
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Recurse into subdirectories.
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Skip per-file interactive confirm before photo library import. Requires --shortest-path. [D10]
        #[arg(long, default_value_t = false)]
        auto_import: bool,

        /// Enable verified AVIF Photos import after Gate 1. JXL stays local-only.
        #[arg(long = "shortest-path", default_value_t = false)]
        shortest_path: bool,

        /// Archive mode: JPEG → JXL uses cjxl effort 11.
        #[arg(long, default_value_t = false)]
        archive: bool,

        /// Explicitly resume a verified matching interrupted task. Changed inputs are rejected.
        #[arg(long, default_value_t = false)]
        retry: bool,

        /// Start in a new adjacent output directory without consuming prior state.
        #[arg(long, default_value_t = false, conflicts_with = "retry")]
        no_resume: bool,

        /// Enable explicitly gated lab-only encoder/decoder fallbacks. Final verification is unchanged.
        #[arg(long = "allow_expert_options", default_value_t = false)]
        allow_expert_options: bool,

        /// Meme mode strategy. "jxl" (default) or "avif" (Meme Mode).
        #[arg(long, value_parser = ["jxl", "avif"], default_value = "jxl")]
        strategy: String,

        /// Accepted for CLI compatibility. Meme Mode keeps its bounded coarse-plus-binary search.
        #[arg(long = "extreme-precision", default_value_t = false)]
        extreme_precision: bool,
    },

    /// Restore true JXL files back to JPEG in an adjacent output tree.
    #[command(name = "restore-jpeg")]
    RestoreJpeg {
        /// Input directory (or single file) containing source JXL files.
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Output directory. Defaults to an adjacent *_`restored_jpeg` directory.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Recurse into subdirectories.
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Overwrite existing restored JPEGs.
        #[arg(short, long, default_value_t = false)]
        force: bool,

        /// Preserve source JXL files after verified JPEG reconstruction.
        #[arg(long, default_value_t = false)]
        keep_source: bool,
    },
}

const fn command_requires_database(command: &Commands) -> bool {
    !matches!(
        command,
        Commands::FastImg { .. } | Commands::RestoreJpeg { .. }
    )
}

fn validate_command_strategy(command: &Commands) -> anyhow::Result<()> {
    if matches!(
        command,
        Commands::Run {
            codec,
            ..
        } if codec == "av1"
    ) {
        anyhow::bail!(
            "manual AVIF selection is unavailable in img run; AVIF is reserved for fast-img \
             Meme Mode. Use img fast-img --strategy avif <input>"
        );
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let result = main_inner();
    foundation::progress_mode::flush_log_file();
    result
}

fn initialize_analysis_cache(command: &Commands) -> Option<Arc<AnalysisCache>> {
    if !command_requires_database(command) {
        return None;
    }

    // Full img flows require PostgreSQL. FastImg and restore-jpeg are deliberately
    // excluded because they do not use DB/cache state.
    if let Err(error) = foundation::database::open_pg_client() {
        foundation::log_fatal!(
            "Infrastructure",
            &format!(
                "PostgreSQL database is mandatory for full feature availability. Connection failed: {error}"
            )
        );
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    let cache = match AnalysisCache::default_local() {
        Ok(cache) => Some(Arc::new(cache)),
        Err(error) => {
            foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                "analysis_cache_unavailable",
                format!("failed to initialize persistent cache: {error}"),
            );
            None
        }
    };

    if let Some(cache) = &cache {
        match cache.cleanup_old_records(foundation::constants::CACHE_PRUNE_AGE_SECS) {
            Ok(removed) if removed > 0 => {
                foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                    "analysis_cache_age_prune_completed",
                    format!("removed={removed}"),
                );
            }
            Err(error) => {
                foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                    "analysis_cache_age_prune_failed",
                    format!("failed to prune aged cache rows: {error}"),
                );
            }
            Ok(_) => {}
        }
    }
    cache
}

fn command_lock_input(command: &Commands) -> Option<&Path> {
    match command {
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
        | Commands::PathHash { input }
        | Commands::IngestSamples { input, .. } => Some(input),
        _ => None,
    }
}

fn acquire_command_lock(command: &Commands) -> Option<foundation::infra::process_lock::DirLock> {
    let input = command_lock_input(command)?;
    let input = foundation::media_conversion_gate::canonicalize_for_tool_input(input);
    if !input.is_dir() {
        return None;
    }
    match foundation::acquire_dir_lock(&input) {
        Ok(guard) => Some(guard),
        Err(error) => {
            log_fatal!(
                foundation::infra::static_logs::messages::LABEL_LOCK,
                &error.to_string()
            );
            std::process::exit(foundation::constants::EXIT_CODE_LOCK_FAILURE);
        }
    }
}

fn main_inner() -> anyhow::Result<()> {
    foundation::entry_guard::assert_product_cli_entry("img").context("img entry guard")?;
    foundation::init_ghost_mode().context("Failed to initialize ghost mode")?;

    foundation::logging::init("img", &foundation::logging::LogConfig::default())
        .map_err(|e| e.context("Failed to initialize img logging"))?;

    let cli = Cli::parse();
    validate_command_strategy(&cli.command)?;

    // Initialize Ctrl+C guard for long-running batch operations
    foundation::ctrlc_guard::init();

    let cache = initialize_analysis_cache(&cli.command);
    let _lock_guard = acquire_command_lock(&cli.command);

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
            archive,
            allow_size_tolerance,
            no_allow_size_tolerance,
            allow_expert_options,
            preserve_timestamps,
            preserve,
            verbose,
            plain,

            base_dir,
            resume: resume_flag,
            no_resume,
            codec,
        } => {
            use foundation::delivery_codec_strategy::resolve_cli_img_static_delivery;

            let resume = foundation::checkpoint::resolve_resume_choice(
                &input,
                output.as_deref(),
                resume_flag,
                no_resume,
            )?;
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let should_delete = delete_original || in_place;

            let img_static_delivery = resolve_cli_img_static_delivery(&codec, apple_compat)?;

            let flag_mode =
                match foundation::validate_flags_result_with_ultimate(foundation::FlagRequest {
                    base: foundation::FlagBase {
                        explore,
                        match_quality,
                        compress,
                    },
                    tier: foundation::FlagTier { ultimate },
                }) {
                    Ok(mode) => mode,
                    Err(e) => {
                        log_fatal!(foundation::infra::static_logs::messages::LABEL_CONFIG, &e);
                        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                    }
                };

            // Fail-fast if critical sub-tools are missing
            if let Err(e) = foundation::tools::require(&["cjxl", "djxl", "exiftool", "ffmpeg"]) {
                log_fatal!(foundation::infra::static_logs::messages::LABEL_TOOLS, &e);
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }

            foundation::progress_mode::configure_terminal_ux(plain);
            foundation::progress_mode::set_verbose_mode(verbose);
            foundation::progress_mode::maybe_log_inference_analytics_hint(verbose);
            // Create run log first; all subsequent output is captured here
            if let Err(e) = foundation::progress_mode::set_default_run_log_file("img") {
                log_fatal!(
                    foundation::infra::static_logs::messages::LABEL_RUN_LOG,
                    &format!(
                        "{}: {}",
                        foundation::infra::static_logs::messages::RUN_LOG_OPEN_FAIL,
                        e
                    )
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
            foundation::log_summary_header!(&format!(
                "{} {}",
                symbols::VIDEO,
                flag_mode.description_en()
            ));
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_STRATEGY,
                foundation::delivery_codec_strategy::img_run_routing_summary(img_static_delivery)
            );
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_MAPPING,
                &foundation::infra::static_logs::messages::MSG_MAIN_IMAGE_MAPPING
                    .replace("{}", symbols::IMAGE)
            );
            if apple_compat {
                log_stat!(
                    foundation::infra::static_logs::messages::LABEL_CONFIG,
                    format!(
                        "{shield} {bold}Apple Compatibility Audit: Hardware-optimized encoding enabled{reset}",
                        shield = symbols::SHIELD,
                        bold = colors::BOLD,
                        reset = colors::RESET,
                    )
                );
                unsafe { std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1") };
            }

            if in_place {
                log_stat!(
                    foundation::infra::static_logs::messages::LABEL_CONFIG,
                    format!(
                        "{save} {bold}Destructive Write Audit: In-place modification enabled{reset}",
                        save = symbols::SAVE,
                        bold = colors::BOLD,
                        reset = colors::RESET,
                    )
                );
            }
            if ultimate {
                log_stat!(
                    foundation::infra::static_logs::messages::LABEL_CONFIG,
                    format!(
                        "{search} {bold}Quality Audit: Ultimate-tier precision encoding enabled{reset}",
                        search = symbols::SEARCH,
                        bold = colors::BOLD,
                        reset = colors::RESET,
                    )
                );
            }
            if archive {
                log_stat!(
                    foundation::infra::static_logs::messages::LABEL_CONFIG,
                    "Archive Audit: encoder effort/preset overrides enabled"
                );
            }
            if allow_expert_options {
                log_stat!(
                    foundation::infra::static_logs::messages::LABEL_CONFIG,
                    "Expert Options Audit: gated encoder/decoder fallbacks enabled; final verification remains mandatory"
                );
            }
            if !allow_size_tolerance {
                log_stat!(
                    foundation::infra::static_logs::messages::LABEL_CONFIG,
                    format!(
                        "{chart} {bold}Precision Audit: Strict bit-for-bit size threshold enforcement enabled{reset}",
                        chart = symbols::CHART,
                        bold = colors::BOLD,
                        reset = colors::RESET,
                    )
                );
            }

            foundation::database::report_db_status();

            let mut config_flags = ConfigFlags::USE_GPU;
            config_flags.set(ConfigFlags::FORCE, force);
            config_flags.set(ConfigFlags::DELETE_ORIGINAL, should_delete);
            config_flags.set(ConfigFlags::PRESERVE_TIMESTAMPS, preserve_timestamps);
            config_flags.set(ConfigFlags::PRESERVE_METADATA, preserve);
            config_flags.set(ConfigFlags::COMPRESS, compress);
            config_flags.set(ConfigFlags::APPLE_COMPAT, apple_compat);
            config_flags.set(ConfigFlags::IN_PLACE, in_place);
            config_flags.set(ConfigFlags::EXPLORE_SMALLER, explore);
            config_flags.set(ConfigFlags::MATCH_QUALITY, match_quality);
            config_flags.set(ConfigFlags::ULTIMATE_MODE, ultimate);
            config_flags.set(ConfigFlags::ARCHIVE_MODE, archive);
            config_flags.set(ConfigFlags::ALLOW_SIZE_TOLERANCE, allow_size_tolerance);
            config_flags.set(ConfigFlags::ALLOW_EXPERT_OPTIONS, allow_expert_options);
            config_flags.set(ConfigFlags::VERBOSE, verbose);
            let config = AutoConvertConfig {
                output_dir: output,
                base_dir,
                flags: config_flags,
                child_threads: 0,
                cache: cache.clone(),
                static_delivery: img_static_delivery,
                error_mode: foundation::BatchErrorMode::current(),
            };

            let workload = foundation::thread_manager::WorkloadType::Image;
            let thread_config = foundation::thread_manager::get_balanced_thread_config(workload);
            let mut config = config;
            config.child_threads = thread_config.child_threads;

            if input.is_file() {
                auto_convert_single_file(&input, &config)?;
            } else if input.is_dir() {
                auto_convert_directory(&input, &config, recursive, resume)?;
            } else {
                log_fatal!(
                    "Input",
                    &foundation::infra::static_logs::messages::MSG_CONVERSION_VALIDATE_EXIST
                        .replace("{}", &input.display().to_string()),
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
        }

        Commands::RestoreTimestamps { source, output } => {
            if let Err(e) = foundation::restore_timestamps_from_source_to_output(&source, &output) {
                foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                    "timestamp_restore_failed",
                    e.to_string(),
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
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
                        log_summary_header!(
                            foundation::infra::static_logs::messages::LABEL_CACHE_AUDIT
                        );
                        let records = stats.total_records();
                        let size_mb = stats.db_size_mb();
                        foundation::log_stat!(
                            foundation::infra::static_logs::messages::LABEL_CACHE_INVENTORY,
                            format!(
                                "Persistent Cache Audit: {records} records, database size {size_mb:.2} MB"
                            )
                        );
                        let permille = {
                            let ratio = Rational::from(stats.db_size_bytes)
                                / Rational::from(
                                    foundation::analysis_cache::CACHE_SIZE_LIMIT_BYTES.max(1),
                                );
                            let res: Rational = ratio * Rational::from(10_000);
                            res.to_f64()
                        };
                        let usage_percent = permille / 100.0;
                        let limit_gb =
                            foundation::constants::CACHE_SIZE_LIMIT_BYTES / 1024 / 1024 / 1024;

                        foundation::log_stat!(
                            foundation::infra::static_logs::messages::LABEL_CACHE_STORAGE,
                            format!(
                                "Persistent Cache Audit: Capacity utilization at {usage_percent:.1}% (limit {limit_gb} GB)"
                            )
                        );
                        let schema = stats.schema_version;
                        let algorithm = stats.current_algorithm_version;
                        foundation::log_stat!(
                            foundation::infra::static_logs::messages::LABEL_CACHE_SCHEMA,
                            format!(
                                "Persistent Cache Audit: schema v{schema}, current algorithm v{algorithm}"
                            )
                        );

                        if !stats.algorithm_version_distribution.is_empty() {
                            let mut versions: Vec<_> =
                                stats.algorithm_version_distribution.iter().collect();
                            versions.sort_by_key(|(v, _)| *v);
                            for (version, count) in versions {
                                let marker = match (*version).cmp(&stats.current_algorithm_version)
                                {
                                    core::cmp::Ordering::Less => "(legacy/stale)",
                                    core::cmp::Ordering::Equal => "(active/current)",
                                    core::cmp::Ordering::Greater => {
                                        foundation::modern_ui::symbols::pick(
                                            "❓ (experimental)",
                                            "[?] (experimental)",
                                        )
                                    }
                                };
                                foundation::log_detail!(format!(
                                    "Persistent Cache Audit: algorithm v{version} -> {count} records {marker}"
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        log_fatal!(
                            "Cache Audit",
                            format!("Persistent Cache Audit: Integrity scan failed: {e}")
                        );
                        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                    }
                }
            } else {
                log_fatal!(
                    "System Audit",
                    "Cache infrastructure is not initialized or unavailable in the current context."
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
        }

        Commands::LockCheck { input } => {
            let input_abs = foundation::media_conversion_gate::canonicalize_for_tool_input(&input);
            if input_abs.is_dir() {
                // Try to acquire lock. If it fails, report and exit immediately with code 3.
                match foundation::acquire_dir_lock(&input_abs) {
                    Ok(_lock) => {
                        foundation::log_success!(
                            foundation::infra::static_logs::messages::LABEL_SUCCESS,
                            "Directory Lock acquired: path is available for processing."
                        );
                    }
                    Err(e) => {
                        log_fatal!(
                            "Directory Lock Audit",
                            &foundation::infra::static_logs::messages::MSG_MAIN_LOCK_FAIL
                                .replace("{}", &e.to_string())
                        );
                        std::process::exit(foundation::constants::EXIT_CODE_LOCK_FAILURE);
                    }
                }
            }
        }

        Commands::PathHash { input } => {
            let hash = foundation::hash_path_to_hex(&input)?;
            foundation::log_detail!(&hash);
        }
        Commands::DbHealth => {
            foundation::log_info!(
                foundation::infra::static_logs::messages::LABEL_INFRASTRUCTURE_AUDIT,
                foundation::infra::static_logs::messages::DB_HEALTH_START
            );
            match foundation::database::check_database_health() {
                Ok(report) => {
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_SESSION_SUMMARY,
                        foundation::infra::static_logs::messages::DB_HEALTH_FINALIZED
                    );
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CONN.replacen(
                            "{}",
                            if report.connected {
                                foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CONN_OK
                            } else {
                                foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CONN_FAIL
                            },
                            1
                        )
                    );
                }
                Err(e) => {
                    foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                        "db_health_abort",
                        foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_ABORT
                            .replace("{}", &e.to_string()),
                    );
                }
            }
        }

        Commands::IngestSamples { input, label } => {
            let mut conn = foundation::database::open_pg_client()?;
            foundation::image_quality_db::init_quality_schema(&mut conn)?;

            if let Some(lbl) = &label {
                log_detail!(format!(
                    "{save} Active Learning Audit: Ingesting labeled samples [{lbl}] from {input_path}",
                    save = foundation::modern_ui::symbols::SAVE,
                    input_path = input.display(),
                ));
            } else {
                log_detail!(format!(
                    "{save} Active Learning Audit: Ingesting raw samples from {input_path}",
                    save = foundation::modern_ui::symbols::SAVE,
                    input_path = input.display(),
                ));
            }

            let mut count = 0;
            let mut failures = Vec::new();
            let mut dirs_to_visit = vec![input];

            while let Some(dir) = dirs_to_visit.pop() {
                let entries = match std::fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(e) => {
                        let message = format!("Failed to read directory {}: {}", dir.display(), e);
                        foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                            "ingest", &message,
                        );
                        failures.push(message);
                        continue;
                    }
                };

                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            let message = format!(
                                "Failed to inspect directory entry under {}: {}",
                                dir.display(),
                                e
                            );
                            foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                                "ingest", &message,
                            );
                            failures.push(message);
                            continue;
                        }
                    };

                    let path = entry.path();
                    if path.is_dir() {
                        dirs_to_visit.push(path);
                    } else if path.is_file() {
                        let ext =
                            foundation::media_conversion_gate::path_extension_lowercase_or_empty(
                                &path,
                                &format!("ingest scan {}", path.display()),
                            );

                        if [
                            "jpg", "jpeg", "png", "heic", "heif", "jxl", "tiff", "bmp", "webp",
                        ]
                        .contains(&ext.as_str())
                        {
                            let default_label =
                                foundation::media_conversion_gate::ingest_quality_label_or_default(
                                    label.as_deref(),
                                );
                            if let Err(e) = foundation::image_quality_db::ingest_quality_sample(
                                &mut conn,
                                &path,
                                &default_label,
                                "fusion_v1",
                            ) {
                                let message = format!("Failed to ingest {}: {}", path.display(), e);
                                foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                                    "ingest", &message,
                                );
                                failures.push(message);
                            } else {
                                count += 1;
                            }
                        }
                    }
                }
            }
            foundation::log_success!(
                foundation::infra::static_logs::messages::LABEL_SUCCESS,
                format!(
                    "{check} Active Learning Audit: Successfully ingested {count} feature vectors",
                    check = foundation::modern_ui::symbols::CHECK,
                )
            );
            if !failures.is_empty() {
                let sample_failures = failures
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" | ");
                anyhow::bail!(
                    "Ingest completed with {} successful samples and {} failures{}",
                    count,
                    failures.len(),
                    if sample_failures.is_empty() {
                        String::new()
                    } else {
                        format!("; sample failures: {sample_failures}")
                    }
                );
            }
        }

        Commands::FastImg {
            input,
            output,
            delete_source,
            dry_run,
            recursive,
            auto_import,
            shortest_path,
            archive,
            retry,
            no_resume,
            allow_expert_options,
            strategy,
            extreme_precision,
        } => {
            let options = FastImgRunOptions {
                input: &input,
                output_dir: output.as_deref(),
                delete_source: DeleteSourceFlag(delete_source),
                dry_run: DryRunFlag(dry_run),
                recursive: RecursiveFlag(recursive),
                auto_import: AutoImportFlag(auto_import),
                shortest_path: ShortestPathFlag(shortest_path),
                retry: RetryFlag(retry),
                fresh: FreshFlag(no_resume),
                archive,
                allow_expert_options,
                strategy: &strategy,
                extreme_precision,
            };
            run_fast_img(options)?;
        }
        Commands::RestoreJpeg {
            input,
            output,
            recursive,
            force,
            keep_source,
        } => {
            run_restore_jpeg(&input, output.as_deref(), recursive, force, keep_source)?;
        }
    }

    // Historically waited for macOS UI confirmation via foundation::macos_ui.
    // The foundation crate no longer exposes that module; keep this as a no-op.

    Ok(())
}

fn verify_conversion(
    original: &std::path::Path,
    converted: &std::path::Path,
    cache: Option<&AnalysisCache>,
) -> anyhow::Result<()> {
    log_detail!(foundation::infra::static_logs::messages::MSG_VERIFICATION_INIT);
    log_detail!(format!(
        "{icon} Forensic Verification: Reference bitstream identified as {path}",
        icon = foundation::modern_ui::symbols::IMAGE,
        path = original.display(),
    ));
    log_detail!(format!(
        "{icon} Forensic Verification: Evaluating candidate bitstream: {path}",
        icon = foundation::modern_ui::symbols::SAVE,
        path = converted.display(),
    ));

    let original_analysis = foundation::image_analyzer::analyze_image_with_cache(original, cache)?;
    let converted_analysis =
        foundation::image_analyzer::analyze_image_with_cache(converted, cache)?;

    log_summary_header!(foundation::infra::static_logs::messages::LABEL_STRUCTURAL_AUDIT);
    let original_size = original_analysis.file_size;
    let converted_size = converted_analysis.file_size;
    let savings_pct = 100.0
        * (1.0
            - foundation::numeric_cast::u64_to_f64(converted_size)
                / foundation::numeric_cast::u64_to_f64(original_size));
    foundation::log_report_stat!(
        foundation::infra::static_logs::messages::LABEL_INVENTORY_AUDIT,
        format!(
            "Forensic Verification: bitstream size {converted_size} B ({savings_pct:.2}% smaller than reference {original_size} B)"
        )
    );

    let orig_img = load_image_safe(original)?;
    let conv_img = load_image_safe(converted)?;

    log_summary_header!(foundation::infra::static_logs::messages::LABEL_PERCEPTUAL_AUDIT);

    if let Some(psnr) = calculate_psnr(&orig_img, &conv_img) {
        if psnr.is_infinite() {
            foundation::log_report_stat!(
                foundation::infra::static_logs::messages::LABEL_PSNR,
                foundation::infra::static_logs::messages::PSNR_INFINITY
            );
        } else {
            let quality = psnr_quality_description(psnr);
            foundation::log_report_stat!(
                foundation::infra::static_logs::messages::LABEL_PSNR,
                format!("Forensic Verification: PSNR verified at {psnr:.2} dB ({quality})")
            );
        }
    }

    if let Some(ssim) = calculate_ssim(&orig_img, &conv_img) {
        let quality = ssim_quality_description(ssim);
        foundation::log_report_stat!(
            foundation::infra::static_logs::messages::LABEL_SSIM,
            format!(
                "Forensic Verification: Structural Similarity (SSIM) verified at {ssim:.6} ({quality})"
            )
        );
    }

    log_success!(
        foundation::infra::static_logs::messages::LABEL_SUCCESS,
        foundation::infra::static_logs::messages::MSG_VERIFICATION_SUCCESS
    );

    Ok(())
}

fn load_image_safe(path: &std::path::Path) -> anyhow::Result<image::DynamicImage> {
    let ext = foundation::media_conversion_gate::path_extension_lowercase_or_empty(
        path,
        &format!("verify load_image {}", path.display()),
    );
    let is_jxl = foundation::quality_matcher::parse_source_codec(&ext) == SourceCodec::JpegXl;

    if is_jxl {
        let temp_png_file =
            foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
                "img_verify_jxl_png",
                None,
                Some(".png"),
            )
            .map_err(|e| anyhow::anyhow!("Failed to create temp file in MFB scratch: {e}"))?;

        let temp_path = temp_png_file.path();

        let mut builder = foundation::jxl_builder::DjxlBuilder::new();
        builder.input(path).output(temp_path);

        let status = builder
            .build()
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to execute djxl: {e}"))?;

        if !status.success() {
            return Err(anyhow::anyhow!("djxl failed to decode JXL file"));
        }

        let img = foundation::image_detection::open_image_with_limits(temp_path)
            .map_err(|e| anyhow::anyhow!("Failed to open decoded PNG: {e}"))?;

        Ok(img)
    } else {
        foundation::image_detection::open_image_with_limits(path)
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
    static_delivery: foundation::delivery_codec_strategy::ImgStaticDelivery,
    error_mode: foundation::BatchErrorMode,
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
    const fn allow_expert_options(&self) -> bool {
        self.flags.contains(ConfigFlags::ALLOW_EXPERT_OPTIONS)
    }
    const fn archive(&self) -> bool {
        self.flags.contains(ConfigFlags::ARCHIVE_MODE)
    }
    const fn verbose(&self) -> bool {
        self.flags.contains(ConfigFlags::VERBOSE)
    }
}

fn fast_static_modern_compression(
    input: &Path,
    detected_format: &foundation::image_detection::DetectedFormat,
) -> anyhow::Result<foundation::image_detection::CompressionType> {
    foundation::image_detection::detect_compression(detected_format, input).map_err(|err| {
        foundation::media_conversion_gate::probe_image_format_audit(
            "fast_static_skip_compression_detect_failed",
            input,
            format!(
                "fast static preflight refused re-encode after compression detection error: {err}"
            ),
        );
        anyhow::anyhow!(
            "Failed to determine modern input compression for {}: {err}",
            input.display()
        )
    })
}

const fn fast_img_avif_source_is_lossless(
    compression: foundation::image_detection::CompressionType,
) -> Option<bool> {
    use foundation::image_detection::CompressionType;

    match compression {
        CompressionType::Lossless => Some(true),
        CompressionType::Lossy => Some(false),
        CompressionType::Unknown | CompressionType::JpegReconstruction => None,
    }
}

fn fast_static_skip_or_ignore(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<Option<ConversionOutput>> {
    use foundation::image_detection::{CompressionType, DetectedFormat};

    let detected_format =
        foundation::image_detection::detect_format_from_bytes(input).map_err(|err| {
            foundation::media_conversion_gate::probe_image_format_audit(
                "fast_static_skip_format_detect_failed",
                input,
                format!(
                    "fast static preflight refused route guess after format detection error: {err}"
                ),
            );
            anyhow::anyhow!(
                "Failed to detect true input format for fast static preflight {}: {err}",
                input.display()
            )
        })?;

    if !matches!(
        detected_format,
        DetectedFormat::WebP
            | DetectedFormat::AVIF
            | DetectedFormat::HEIC
            | DetectedFormat::HEIF
            | DetectedFormat::JXL
    ) {
        return Ok(None);
    }

    let file_size = foundation::io_utils::metadata_with_retry(input)
        .map_err(|e| anyhow::anyhow!("Failed to read metadata for {}: {e}", input.display()))?
        .len();

    // Fast-path modern formats must not skip animated assets before full analysis.
    let preflight_analysis = foundation::image_analyzer::ImageAnalysis {
        format: detected_format.as_str().to_string(),
        file_size,
        ..foundation::image_analyzer::ImageAnalysis::default()
    };
    if let Some(outcome) =
        foundation::media_conversion_gate::animation_reject_outcome(input, &preflight_analysis)
    {
        foundation::progress_mode::image_ignored(input, &outcome.reason, Some(outcome.class));
        return Ok(Some(conversion_output_ignored(
            input,
            outcome.reason,
            file_size,
        )));
    }

    if detected_format == DetectedFormat::JXL {
        let reason = foundation::infra::static_logs::messages::JXL_OPTIMAL_SKIP;
        foundation::progress_mode::image_skipped(input, reason);
        copy_original_if_adjacent_mode(input, config)?;
        return Ok(Some(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            ignored: false,
            message: reason.to_string(),
            original_size: file_size,
            output_size: None,
            size_reduction: None,
            blake3: None,
        }));
    }

    let compression = fast_static_modern_compression(input, &detected_format)?;

    // Fail-closed routing (formats reaching here: WebP/AVIF/HEIC/HEIF):
    // - ConfirmedLossy → skip/retain (generational-loss guard, as before)
    // - ConfirmedLossless → continue to the lossless conversion route
    // - Unknown compression semantics → retain original; a possibly-lossless
    //   source must never take a second-generation lossy route
    let skip_reason = match compression {
        CompressionType::Lossy => {
            foundation::should_skip_image_format(detected_format.as_str(), false).reason
        }
        CompressionType::Lossless => return Ok(None),
        CompressionType::Unknown => format!(
            "{} with unproven compression semantics; retaining original (fail-closed)",
            detected_format.as_str()
        ),
        CompressionType::JpegReconstruction => {
            "JPEG XL with JPEG reconstruction data; reversible route only".to_string()
        }
    };

    foundation::progress_mode::image_skipped(input, &skip_reason);
    copy_original_if_adjacent_mode(input, config)?;
    Ok(Some(ConversionOutput {
        original_path: input.display().to_string(),
        output_path: input.display().to_string(),
        skipped: true,
        ignored: false,
        message: skip_reason,
        original_size: file_size,
        output_size: None,
        size_reduction: None,
        blake3: None,
    }))
}

fn copy_original_if_adjacent_mode(input: &Path, config: &AutoConvertConfig) -> anyhow::Result<()> {
    foundation::copy_on_skip_or_fail(
        input,
        config.output_dir.as_deref(),
        config.base_dir.as_deref(),
        config.verbose(),
    )?;
    Ok(())
}

use img::conversion_api::ConversionOutput;

fn conversion_output_ignored(input: &Path, reason: String, original_size: u64) -> ConversionOutput {
    ConversionOutput {
        original_path: input.display().to_string(),
        output_path: String::new(),
        skipped: false,
        ignored: true,
        message: reason,
        original_size,
        output_size: None,
        size_reduction: None,
        blake3: None,
    }
}

fn convert_result_to_output(result: foundation::TaskResult) -> anyhow::Result<ConversionOutput> {
    if !result.success && !result.ignored {
        anyhow::bail!(result.message);
    }
    let input_path = result.input_path.clone();
    let output_path = if let Some(v) = result.output_path.clone() {
        v
    } else {
        // Skipped/ignored tasks legitimately have no output path - use input path silently.
        // Only warn for converted tasks that somehow lack output_path (unexpected).
        if !result.skipped && !result.ignored {
            foundation::media_conversion_gate::delivery_api_batch_fallback_audit(
                "task_output_path",
                format!(
                    "converted task without output_path for {input_path}; using input path for reporting"
                ),
            );
        }
        input_path
    };
    Ok(ConversionOutput {
        original_path: result.input_path,
        output_path,
        skipped: result.skipped,
        ignored: result.ignored,
        message: result.message,
        original_size: result.input_size,
        output_size: result.output_size,
        size_reduction: result
            .size_reduction
            .map(foundation::numeric_cast::f64_to_f32_lossy),
        blake3: result.blake3,
    })
}

fn image_batch_should_abort(mode: foundation::BatchErrorMode, error: &anyhow::Error) -> bool {
    mode.should_abort_error(error)
}

#[cfg(test)]
mod conversion_result_adapter_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::expect_fun_call,
        clippy::panic
    )]
    use super::{convert_result_to_output, image_batch_should_abort};

    #[test]
    fn failed_task_result_is_not_promoted_to_conversion_success() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let input = temp.path().join("corrupt.jpg");
        std::fs::write(&input, b"corrupt")?;
        let result = foundation::TaskResult::failed_with_fallback(
            &input,
            &foundation::conversion::ConvertOptions::default(),
            "decode failed",
            "decode_failed",
        )?;

        let error = convert_result_to_output(result).expect_err("failed task must remain failed");
        assert_eq!(error.to_string(), "decode failed");
        Ok(())
    }

    #[test]
    fn image_batch_continues_only_classified_recoverable_errors() {
        let recoverable: anyhow::Error =
            foundation::UnifiedError::analysis_error("bad image").into();
        assert!(!image_batch_should_abort(
            foundation::BatchErrorMode::LogAndContinue,
            &recoverable
        ));
        assert!(image_batch_should_abort(
            foundation::BatchErrorMode::FailFast,
            &recoverable
        ));

        let unknown = anyhow::anyhow!("unclassified image failure");
        assert!(image_batch_should_abort(
            foundation::BatchErrorMode::LogAndContinue,
            &unknown
        ));
    }
}

fn auto_convert_single_file(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<ConversionOutput> {
    foundation::infra::static_logs::log_task_start(&input.to_string_lossy());

    // Pause if the user is being prompted to exit via Ctrl+C
    foundation::ctrlc_guard::wait_if_prompt_active();

    // Check for Apple Photos library before processing
    if let Err(e) = foundation::check_apple_photos_library(input) {
        log_fatal!(
            foundation::infra::static_logs::messages::LABEL_APPLE_PHOTOS,
            &e
        );
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    if let Some(ref out_dir) = config.output_dir
        && let Err(e) = foundation::check_apple_photos_library(out_dir)
    {
        log_fatal!(
            foundation::infra::static_logs::messages::LABEL_APPLE_PHOTOS,
            &e
        );
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    // Fix extension by content first so all downstream checks see the real format (avoids disguised-extension panic).
    // When an output directory is configured the source tree must remain immutable:
    // use the readonly variant that logs mismatches without renaming source files.
    let fixed_input = if config.output_dir.is_some() {
        foundation::check_extension_mismatch_readonly(input)?
    } else {
        foundation::fix_extension_if_mismatch(input)?
    };
    let input = fixed_input.as_path();

    let label = foundation::media_conversion_gate::path_file_name_for_log(input);
    foundation::progress_mode::set_log_context(&label);
    let _log_guard = foundation::progress_mode::LogContextGuard;

    // Check for Live Photos first (before any analysis)
    // Only skip in Apple compat mode to preserve the pair association.
    // In normal mode, we treat the HEIC as a regular image to be upgraded.
    if config.apple_compat() && foundation::live_photo::is_live(input) {
        let reason = "Live Photo detected in Apple compat mode - skipping to preserve pair";
        foundation::progress_mode::image_skipped(input, reason);
        let file_size = foundation::io_utils::metadata_with_retry(input)
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to read metadata for Live Photo skip {}: {e}",
                    input.display()
                )
            })?
            .len();
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

    if let Some(preflight_output) = fast_static_skip_or_ignore(input, config)? {
        return Ok(preflight_output);
    }

    let analysis =
        foundation::image_analyzer::analyze_image_with_cache(input, config.cache.as_deref())?;

    let verdict =
        foundation::media_conversion_gate::static_image_conversion_verdict(input, &analysis);
    if let Some(reason) = verdict.ignore_reason() {
        foundation::progress_mode::image_ignored(input, reason, verdict.ignore_class());
        return Ok(conversion_output_ignored(
            input,
            reason.to_string(),
            analysis.file_size,
        ));
    }

    // Single source of truth for static skip: JXL + modern lossy (avoid generational loss).
    // Always skip static JXL (already optimal format)
    if analysis.format.to_uppercase() == "JXL" {
        let reason =
            "Source is static JPEG XL (already optimal) - skipping to avoid generational loss";
        foundation::progress_mode::image_skipped(input, reason);
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

    let skip = foundation::should_skip_image_format(analysis.format.as_str(), analysis.is_lossless);
    if skip.should_skip {
        let reason = skip.reason;
        foundation::progress_mode::image_skipped(input, &reason);
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
        foundation::image_quality_detector::analyze_image_quality_with_cache(
            input,
            config.cache.as_deref(),
        )
    } else {
        None
    };
    if let Some(ref q) = pixel_analysis {
        foundation::log_media_info_for_image_quality(q, input);
    }

    let quality_label = analysis.quality_summary();

    let options = auto_convert_build_options(config, &analysis, quality_label);

    let result = dispatch_static_conversion(input, &analysis, &options, config)?;

    let output = convert_result_to_output(result)?;

    if output.skipped {
        if config.verbose() {
            log_skip!(&label, &output.message);
        }
    } else {
        log_detail!(&format!(
            "{} Finalizing visual asset: {}",
            foundation::infra::static_logs::messages::LABEL_DONE,
            label
        ));
    }

    Ok(output)
}

// Rationale: This function orchestrates complex image conversion workflows where breaking it down would scatter critical state logic.
fn dispatch_static_conversion(
    input: &Path,
    analysis: &foundation::image_analyzer::ImageAnalysis,
    options: &img::lossless_converter::ConvertOptions,
    config: &AutoConvertConfig,
) -> anyhow::Result<foundation::TaskResult> {
    use img::lossless_converter::{convert_jpeg_to_jxl, convert_to_jxl};

    let format = analysis.format.as_str();
    let is_lossless = analysis.is_lossless;

    // Optional forensic scoring. This is disabled by default because neither
    // KNN nor BPP estimates are authoritative conversion evidence.
    let quality = if format == "JPEG" {
        None
    } else if foundation::static_quality_db_lookup_enabled() {
        let path =
            (!analysis.file_path.is_empty()).then(|| std::path::Path::new(&analysis.file_path));
        foundation::lookup_image_quality_with_path(analysis, path)
    } else {
        None
    };

    if let Some(ref q) = quality {
        let label = foundation::infra::static_logs::messages::LABEL_QUALITY;
        if let Some(reason) = q.fallback_reason.as_deref() {
            foundation::log_stat!(
                label,
                format!(
                    "Forensic Quality Score: {:.2} (BPP heuristic | Reason: {reason})",
                    q.score
                )
            );
        } else {
            foundation::log_stat!(
                label,
                format!(
                    "Forensic Quality Score: {:.2} (KNN | Confidence: {:.1}%)",
                    q.score,
                    q.confidence * 100.0
                )
            );
        }
    }

    let is_genuine_png = if format == "PNG" {
        foundation::is_true_png(input)?
    } else {
        false
    };
    Ok(match (format, is_lossless) {
        ("PNG", _) if is_genuine_png => {
            if config.verbose() {
                foundation::log_detail!(&format!(
                    "{} Genuine PNG→Lossless JXL (effort 10, no size gate): {}",
                    foundation::infra::static_logs::messages::LABEL_DONE,
                    input.display()
                ));
            }
            convert_to_jxl(input, options, 0.0_f32, analysis.conversion_color_context())?
        }
        ("WebP" | "AVIF" | "TIFF" | "HEIC" | "HEIF", true) => {
            if (format == "HEIC" || format == "HEIF")
                && let Some(h) = &analysis.heic_analysis
                && h.hdr.has_gainmap
            {
                foundation::log_detail!(&format!(
                    "{} HDR Synthesis Cycle: {} (Gainmap detected)",
                    foundation::infra::static_logs::messages::LABEL_DONE,
                    input.display()
                ));
                return Ok(img::lossless_converter::convert_heic_gainmap_to_jxl(
                    input, options,
                )?);
            }
            if config.verbose() {
                foundation::log_detail!(&format!(
                    "{} Modern Lossless→JXL Cycle: {}",
                    foundation::infra::static_logs::messages::LABEL_DONE,
                    input.display()
                ));
            }
            convert_to_jxl(input, options, 0.0_f32, analysis.conversion_color_context())?
        }
        ("JPEG", _) => {
            use foundation::image_jpeg_analysis::is_ultra_hdr_jpeg_file;
            if is_ultra_hdr_jpeg_file(input)? {
                foundation::log_detail!(&format!(
                    "{} UltraHDR Migration Cycle: {} (Gainmap detected)",
                    foundation::infra::static_logs::messages::LABEL_DONE,
                    input.display()
                ));
                return Ok(img::lossless_converter::convert_ultrahdr_jpeg_to_jxl(
                    input, options,
                )?);
            }

            if config.verbose() {
                foundation::log_detail!(&format!(
                    "{} JPEG→JXL Lossless Transcode Cycle: {}",
                    foundation::infra::static_logs::messages::LABEL_DONE,
                    input.display()
                ));
            }
            convert_jpeg_to_jxl(input, options, analysis.conversion_color_context())?
        }
        (_, true) => {
            if config.verbose() {
                foundation::log_detail!(&format!(
                    "{} Legacy Lossless→JXL Cycle: {}",
                    foundation::infra::static_logs::messages::LABEL_DONE,
                    input.display()
                ));
            }
            convert_to_jxl(input, options, 0.0_f32, analysis.conversion_color_context())?
        }
        _ => {
            use foundation::delivery_codec_strategy::ImgStaticDelivery;
            if config.static_delivery == ImgStaticDelivery::Avif {
                // AVIF static delivery: always use quality=100 (lossy max quality).
                // AVIF advantage is in lossy compression; DB score mapping is not
                // applicable here — use fixed q=100 per design spec.
                foundation::log_detail!(&format!(
                    "{} Lossy→AVIF (q=100) conversion decision for: {}",
                    foundation::infra::static_logs::messages::LABEL_DONE,
                    input.display()
                ));
                return Ok(img::lossless_converter::convert_to_avif(
                    input,
                    Some(100),
                    options,
                )?);
            }
            if config.verbose() {
                log_detail!(&format!(
                    " {} Lossy→JXL (Near-Lossless): {}",
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
                foundation::constants::JXL_ULTIMATE_DISTANCE,
                analysis.conversion_color_context(),
            )?
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
    if let Err(e) = foundation::check_apple_photos_library(input) {
        log_detail!("{e}");
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    if let Some(ref out_dir) = config.output_dir
        && let Err(e) = foundation::check_apple_photos_library(out_dir)
    {
        log_detail!("{e}");
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    if (config.delete_original() || config.in_place())
        && let Err(e) = check_dangerous_directory(input)
    {
        log_detail!("{e}");
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    let mut config_with_base = config.clone();
    if config_with_base.output_dir.is_some() && config_with_base.base_dir.is_none() {
        config_with_base.base_dir = Some(input.to_path_buf());
    }

    let thread_config = foundation::thread_manager::get_balanced_thread_config(
        foundation::thread_manager::WorkloadType::Image,
    );
    let pool_size = thread_config.parallel_tasks;

    config_with_base.child_threads = thread_config.child_threads;

    let config = &config_with_base;

    let start_time = Instant::now();

    let saved_dir_timestamps = match foundation::save_directory_timestamps(input) {
        Ok(saved) => Some(saved),
        Err(e) => {
            foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                "dir_timestamp_snapshot",
                input,
                format!("failed to snapshot directory timestamps: {e}"),
            );
            None
        }
    };

    let files = foundation::collect_image_files_for_perceived_speed(
        input,
        foundation::IMAGE_EXTENSIONS_FOR_CONVERT,
        recursive,
    )?;

    tracing::info!(
        target: "static_log",
        root = %input.display(),
        file_count = files.len(),
        "Image batch queue built"
    );
    for path in &files {
        tracing::trace!(target: "static_log", path = %path.display(), outcome = "queued", "batch_queue");
    }

    let total = files.len();
    foundation::infra::static_logs::log_batch_start_audit("img", "Image Conversion", total);
    if total == 0 {
        foundation::log_detail!(&format!(
            "{} {}",
            foundation::infra::static_logs::messages::NO_IMAGES_FOUND,
            input.display()
        ));

        if let Some(output_dir) = config.output_dir.as_ref()
            && let Some(ref base_dir) = config.base_dir
        {
            foundation::preserve_directory_with_log(base_dir, output_dir)?;
        }

        return Ok(());
    }

    if config.verbose() {
        log_detail!(&format!(
            "{} Discovery Audit: Pipeline setup initiated - scanned and identified {} files pending processing",
            foundation::infra::static_logs::messages::LABEL_METADATA,
            total
        ));
        log_detail!(
            "  Queue Strategy: deeper paths → fast JPEG/direct encodes → smaller files → lower resolution",
        );
    }

    // Initialize checkpoint manager for resume/progress tracking
    let checkpoint = if resume {
        let cp = foundation::checkpoint::Manager::new_resuming_with_context(
            input,
            config.output_dir.as_deref(),
        )
        .with_context(|| {
            format!(
                "failed to initialize resume checkpoint for {}",
                input.display()
            )
        })?;
        if cp.is_resume_mode() {
            if config.verbose() {
                log_stat!(
                    foundation::infra::static_logs::messages::LABEL_METADATA,
                    format!(
                        "Batch Audit: Resume detected - skipping {} already completed images",
                        cp.completed_count()
                    )
                );
            }
            cp.sync_to_processed_list();
        } else {
            foundation::clear_processed_list();
        }
        Some(cp)
    } else {
        foundation::clear_processed_list();
        None
    };

    auto_convert_directory_disk_space_precheck(input, config, &files);

    let success = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let ignored = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    // Collect (path, reason) for every hard failure so we can enumerate them at
    // session end instead of asking the user to grep log shards.
    let failed_paths: Arc<std::sync::Mutex<Vec<(std::path::PathBuf, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let actual_input_bytes = core::sync::atomic::AtomicU64::new(0);
    let actual_output_bytes = core::sync::atomic::AtomicU64::new(0);
    let pause_controller = Arc::new(PauseController::new());
    let abort_requested = AtomicBool::new(false);
    let abort_reason = std::sync::Mutex::new(None::<String>);

    foundation::progress_mode::enable_quiet_mode();
    let progress_bar = Arc::new(foundation::CoarseProgressBar::new(
        foundation::numeric_cast::usize_to_u64(total),
        "Image Optimization",
    ));

    let max_threads = pool_size;
    let child_threads = thread_config.child_threads;

    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(max_threads)
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                "thread_pool_fallback",
                format!(
                    "failed to create {max_threads} thread pool ({e}); falling back to 2 threads"
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
            foundation::media_conversion_gate::runtime_available_parallelism_or_default(
                "img::thread_strategy_log",
            )
        ));
        if let Some(hint) = foundation::thread_manager::memory_cap_hint() {
            log_hint!(hint);
        }
    }

    let next_index = AtomicUsize::new(0);
    pool.install(|| {
rayon::scope(|scope| {
for _ in 0..max_threads {
let next_index = &next_index;
                scope.spawn(|_| loop {
                    if pause_controller.is_paused() || abort_requested.load(Ordering::SeqCst) {
                        break;
                    }

                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    if index >= total {
                        break;
                    }

                    let Some(path) = files.get(index) else {
                        break;
                    };

                    let file_name =
                        foundation::media_conversion_gate::path_file_name_for_log(path);
                    let span = tracing::info_span!("image_processing", file = %path.display());
                    let _enter = span.enter();

                    progress_bar.set_message(&file_name);
                    foundation::infra::static_logs::log_task_start_path(Some(path), &path.display().to_string());

                    // Check if already completed (thread-safe)
                    if let Some(cp) = checkpoint.as_ref() && cp.is_completed(path) {
                        foundation::progress_mode::image_skipped(
                            path,
                            "resume checkpoint: already completed in progress file",
                        );
                        skipped.fetch_add(1, Ordering::Relaxed);
                        let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                        foundation::progress_mode::write_progress_line_to_run_log(
                            start_time.elapsed().as_secs(),
                            foundation::numeric_cast::usize_to_u64(current),
                            foundation::numeric_cast::usize_to_u64(total),
                            &foundation::media_conversion_gate::path_file_name_for_log(path),
                        );
                        progress_bar.set(foundation::numeric_cast::usize_to_u64(current));
                        continue;
                    }

                    match auto_convert_single_file(path, config) {
                        Ok(result) => {
                            if result.ignored {
                                ignored.fetch_add(1, Ordering::Relaxed);
                            } else if result.skipped {
                                skipped.fetch_add(1, Ordering::Relaxed);
                            } else {
                                let checkpoint_error = checkpoint
                                    .as_ref()
                                    .and_then(|cp| cp.mark_completed(path).err());
                                if let Some(e) = checkpoint_error {
                                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                                        "checkpoint_mark_completed",
                                        path,
                                        format!("failed to mark completed: {e}"),
                                    );
                                    let reason = format!("failed to mark checkpoint complete: {e}");
                                    failed.fetch_add(1, Ordering::Relaxed);
                                    foundation::progress_mode::image_processed_failure();
                                    foundation::media_conversion_gate::mutex_guard_or_recover(
                                        "failed_paths_acc",
                                        failed_paths.lock(),
                                    )
                                    .push((path.clone(), reason.clone()));
                                    if !abort_requested.swap(true, Ordering::SeqCst) {
                                        *foundation::media_conversion_gate::mutex_guard_or_recover(
                                            "img_batch_abort_reason",
                                            abort_reason.lock(),
                                        ) = Some(format!("{}: {reason}", path.display()));
                                    }
                                } else {
                                    foundation::infra::static_logs::log_success_at_with_pipeline(
                                        &format!(
                                            "{} Convert Audit",
                                            foundation::modern_ui::symbols::pick(
                                                foundation::modern_ui::symbols::SUCCESS,
                                                foundation::modern_ui::symbols::plain::SUCCESS
                                            )
                                        ),
                                        "img",
                                        Some(path),
                                        &result.message,
                                    );
                                    success.fetch_add(1, Ordering::Relaxed);
                                    foundation::progress_mode::image_processed_success();
                                    actual_input_bytes
                                        .fetch_add(result.original_size, Ordering::Relaxed);
                                    if let Some(out_size) = result.output_size {
                                        actual_output_bytes.fetch_add(out_size, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if let Some(reason) = disk_full_pause_reason(&err_str) {
                                if pause_controller.request_pause(path, reason.clone()) {
                                    foundation::log_detail!(
                                        "⏸ [Batch] Paused at {}: {}",
                                        path.display(),
                                        reason
                                    );
                                }
                                continue;
                            }

                            let unified = e.chain().find_map(|cause| {
                                cause.downcast_ref::<foundation::unified_error::UnifiedError>()
                            });
                            let is_skip = match unified {
                                // String fallback: only the controlled "Skipped:" sentinel prefix
                                // counts as a skip. Substring matches (e.g. a hard error merely
                                // containing "Skipped" or "already optimized") are failures.
                                None => err_str.starts_with("Skipped:"),
                                Some(ue) => ue.is_skip(),
                            };

                            if is_skip {
                                // Copy original file to output directory to prevent data loss for skips
                                let copy_error = config.output_dir.as_ref().and_then(|output_dir| {
                                    foundation::copy_on_skip_or_fail(
                                        path,
                                        Some(output_dir),
                                        config.base_dir.as_deref(),
                                        config.verbose(),
                                    )
                                    .err()
                                });
                                if let Some(copy_err) = copy_error {
                                    log_fatal!(
                                        "Fatal Integrity Violation",
                                        &format!(
                                            "Critical Data Link failure after skip ({}): {}. DATA LOSS RISK!",
                                            path.display(),
                                            copy_err
                                        )
                                    );
                                    let reason =
                                        format!("failed to preserve skipped source: {copy_err}");
                                    failed.fetch_add(1, Ordering::Relaxed);
                                    foundation::progress_mode::image_processed_failure();
                                    foundation::media_conversion_gate::mutex_guard_or_recover(
                                        "failed_paths_acc",
                                        failed_paths.lock(),
                                    )
                                    .push((path.clone(), reason.clone()));
                                    if !abort_requested.swap(true, Ordering::SeqCst) {
                                        *foundation::media_conversion_gate::mutex_guard_or_recover(
                                            "img_batch_abort_reason",
                                            abort_reason.lock(),
                                        ) = Some(format!("{}: {reason}", path.display()));
                                    }
                                } else {
                                    foundation::progress_mode::image_skipped(path, &err_str);
                                    skipped.fetch_add(1, Ordering::Relaxed);
                                    foundation::progress_mode::image_processed_success();
                                }
                            } else {
                                // Classify as read/analysis failure only on unambiguous sentinel types
                                let is_read_error = err_str.contains("Failed to open file")
                                    || err_str.contains("ImageReadError");

                                if is_read_error {
                                    foundation::log_auto_error!(
                                        "Image analysis",
                                        "Failed to read/analyze {}: {}. Original file will be preserved.",
                                        path.display(),
                                        e
                                    );
                                } else {
                                    foundation::log_auto_error!(
                                        "Image conversion",
                                        "Failed {}: {}. Output discarded (Hard Error).",
                                        path.display(),
                                        e
                                    );
                                }

                                foundation::infra::static_logs::log_file_outcome_audit(
                                    "img",
                                    "failed",
                                    path,
                                    &err_str,
                                );
                                failed.fetch_add(1, Ordering::Relaxed);
                                foundation::progress_mode::image_processed_failure();
                                // Accumulate for end-of-session enumeration
                                {
                                    let mut v = foundation::media_conversion_gate::mutex_guard_or_recover("failed_paths_acc", failed_paths.lock());
                                    v.push((path.clone(), err_str.clone()));
                                }
                                if image_batch_should_abort(config.error_mode, &e)
                                    && !abort_requested.swap(true, Ordering::SeqCst)
                                {
                                    *foundation::media_conversion_gate::mutex_guard_or_recover(
                                        "img_batch_abort_reason",
                                        abort_reason.lock(),
                                    ) = Some(format!("{}: {err_str}", path.display()));
                                }
                            }
                        }
                    }
                    let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    foundation::progress_mode::write_progress_line_to_run_log(
                        start_time.elapsed().as_secs(),
                        foundation::numeric_cast::usize_to_u64(current),
                        foundation::numeric_cast::usize_to_u64(total),
                        &foundation::media_conversion_gate::path_file_name_for_log(path),
                    );
                    progress_bar.set(foundation::numeric_cast::usize_to_u64(current));
                });
}
});
});

    progress_bar.finish();
    foundation::progress_mode::disable_quiet_mode();
    foundation::progress_mode::xmp_merge_finalize();
    foundation::progress_mode::flush_log_file();

    let success_count = success.load(Ordering::Relaxed);
    let skipped_count = skipped.load(Ordering::Relaxed);
    let failed_count = failed.load(Ordering::Relaxed);
    let ignored_count = ignored.load(Ordering::Relaxed);
    let processed_count = processed.load(Ordering::Relaxed);

    let mut result = Summary::new();
    let mut post_run_errors = Vec::new();
    result.succeeded = success_count;
    result.failed = failed_count;
    result.skipped = skipped_count;
    result.ignored = ignored_count;
    result.total = processed_count;
    if let Some(pause) = pause_controller.pause_info() {
        result.pause(
            pause.path,
            pause.reason,
            total.saturating_sub(processed_count),
        );
    }
    let abort_reason = foundation::media_conversion_gate::mutex_guard_or_recover(
        "img_batch_abort_reason",
        abort_reason.lock(),
    )
    .clone();

    if !result.paused
        && abort_reason.is_none()
        && let Some(ref output_dir) = config.output_dir
    {
        log_detail!("");
        foundation::log_static!(
            info,
            foundation::infra::static_logs::messages::COPYING_UNSUPPORTED
        );
        let unsupported_base = foundation::media_conversion_gate::base_dir_or_default(
            config.base_dir.as_deref(),
            "copy_unsupported_base",
        );
        // Identify the unsupported bucket first (one batched sidecar call) so
        // the copy report can say what these files actually are; the copy
        // itself proceeds unchanged either way.
        foundation::siegfried::audit_unsupported_identities(
            &foundation::collect_unsupported_files(unsupported_base, recursive),
        );
        let copy_result =
            foundation::copy_unsupported_files(unsupported_base, output_dir, recursive);
        if copy_result.copied > 0 {
            log_detail!(&format!("Copied {} unsupported files", copy_result.copied));
        }
        if copy_result.failed > 0 {
            log_failure!(
                "Unsupported Files",
                &format!("Failed to copy {} files", copy_result.failed),
            );
            post_run_errors.push(format!(
                "Unsupported file copy failed for {} files in {}",
                copy_result.failed,
                output_dir.display()
            ));
        }

        auto_convert_directory_output_completeness_verification(
            config,
            output_dir,
            recursive,
            ignored_count,
            failed_count,
            &mut result,
            &mut post_run_errors,
        );
    }

    if !result.paused
        && abort_reason.is_none()
        && let Some(ref output_dir) = config.output_dir
        && let Some(ref base_dir) = config.base_dir
    {
        log_detail!("");
        foundation::log_static!(
            info,
            foundation::infra::static_logs::messages::METADATA_SYNC
        );
        if let Err(e) = foundation::preserve_directory(base_dir, output_dir) {
            foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                "metadata_tree_sync_failed",
                foundation::infra::static_logs::messages::MSG_METADATA_TREE_FAIL
                    .replace("{}", &e.to_string()),
            );
            post_run_errors.push(format!(
                "Directory metadata synchronization failed for {} -> {}: {e}",
                base_dir.display(),
                output_dir.display()
            ));
        }
    }

    if let Some(ref saved) = saved_dir_timestamps {
        if !result.paused
            && abort_reason.is_none()
            && let Some(ref output_dir) = config.output_dir
            && let Some(ref base_dir) = config.base_dir
            && let Err(e) = foundation::apply_saved_timestamps_to_dst(saved, base_dir, output_dir)
        {
            post_run_errors.push(format!(
                "Failed to apply saved directory timestamps to {}: {e}",
                output_dir.display()
            ));
        }
        if let Err(e) = foundation::restore_directory_timestamps(saved) {
            post_run_errors.push(format!(
                "Failed to restore source directory timestamps after batch run: {e}"
            ));
        }
        log_detail!(foundation::infra::static_logs::messages::DIR_TIMESTAMPS_RESTORED);
    }

    let final_input_bytes = actual_input_bytes.load(Ordering::Relaxed);
    let final_output_bytes = actual_output_bytes.load(Ordering::Relaxed);

    foundation::infra::static_logs::log_batch_complete_audit(
        "img",
        success_count,
        skipped_count,
        ignored_count,
        failed_count,
        processed_count,
    );

    print_summary(
        &result,
        start_time.elapsed(),
        final_input_bytes,
        final_output_bytes,
        "Image Conversion",
    );

    // Finalize checkpoint only on 100% success
    if let Some(cp) = checkpoint {
        if result.paused {
            if let Err(e) = cp.release_lock() {
                foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                    "checkpoint_lock_release_failed",
                    format!("release lock failed: {e}"),
                );
                post_run_errors.push(format!("Checkpoint lock release failed: {e}"));
            }
        } else if failed_count == 0 && abort_reason.is_none() {
            if let Err(e) = cp.cleanup() {
                foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                    "checkpoint_cleanup_failed",
                    format!("cleanup failed: {e}"),
                );
                post_run_errors.push(format!("Checkpoint cleanup failed: {e}"));
            }
        } else if let Err(e) = cp.release_lock() {
            foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                "checkpoint_lock_release_failed",
                format!("release lock failed: {e}"),
            );
            post_run_errors.push(format!("Checkpoint lock release failed: {e}"));
        }
    }

    if !post_run_errors.is_empty() {
        anyhow::bail!(post_run_errors.join(" | "));
    }

    if failed_count > 0 {
        // Enumerate every failed file with its reason so the user doesn't have
        // to grep log shards.
        {
            let paths = foundation::media_conversion_gate::mutex_guard_or_recover(
                "failed_paths_enum",
                failed_paths.lock(),
            );
            for (p, reason) in paths.iter() {
                foundation::log_auto_error!("Failed file", "{}: {}", p.display(), reason);
            }
        }
        if let Some(reason) = abort_reason {
            anyhow::bail!("Batch aborted by error policy after {reason}");
        }
        anyhow::bail!("Batch completed with {failed_count} failed file(s)");
    }

    Ok(())
}

/// Fast JPEG-only batch pipeline: adjacent JXL-only delivery, verified source delete, optional Photos gates.
#[derive(Clone, Copy)]
struct DeleteSourceFlag(bool);

#[derive(Clone, Copy)]
struct DryRunFlag(bool);

#[derive(Clone, Copy)]
struct RecursiveFlag(bool);

#[derive(Clone, Copy)]
struct AutoImportFlag(bool);

#[derive(Clone, Copy)]
struct ShortestPathFlag(bool);

#[derive(Clone, Copy)]
struct RetryFlag(bool);

#[derive(Clone, Copy)]
struct FreshFlag(bool);

#[derive(Clone, Copy)]
struct ArchiveFlag(bool);

#[derive(Clone, Copy)]
struct ExpertOptionsFlag(bool);

#[derive(Clone, Copy)]
struct FastImgRunOptions<'a> {
    input: &'a Path,
    output_dir: Option<&'a Path>,
    delete_source: DeleteSourceFlag,
    dry_run: DryRunFlag,
    recursive: RecursiveFlag,
    auto_import: AutoImportFlag,
    shortest_path: ShortestPathFlag,
    retry: RetryFlag,
    fresh: FreshFlag,
    archive: bool,
    allow_expert_options: bool,
    strategy: &'a str,
    extreme_precision: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FastImgPostGate1Policy {
    LocalOnlyDelivery,
    ShortestPathImportAndVerify,
}

/// `FastImg` candidate collection only needs container metadata. Keep it out of
/// `image_analyzer`: that path performs full quality/entropy analysis.
fn fast_img_scan_failure_rel_key(path: &Path, src_dir: &Path) -> anyhow::Result<String> {
    let rel_key = path
        .strip_prefix(src_dir)
        .with_context(|| {
            format!(
                "fast-img scan failure path escaped source root: source={} root={}",
                path.display(),
                src_dir.display()
            )
        })?
        .to_string_lossy()
        .to_string();
    Ok(rel_key)
}

fn fast_img_container_is_static(path: &Path, format: FormatKind) -> anyhow::Result<bool> {
    let detected_format = match format {
        FormatKind::Jpeg => return Ok(true),
        FormatKind::Png => foundation::image_detection::DetectedFormat::PNG,
        FormatKind::WebP => foundation::image_detection::DetectedFormat::WebP,
        FormatKind::Heic => foundation::image_detection::DetectedFormat::HEIC,
        FormatKind::Heif => foundation::image_detection::DetectedFormat::HEIF,
        FormatKind::Avif => foundation::image_detection::DetectedFormat::AVIF,
        FormatKind::Gif => foundation::image_detection::DetectedFormat::GIF,
        FormatKind::Bmp => foundation::image_detection::DetectedFormat::BMP,
        FormatKind::Jxl => foundation::image_detection::DetectedFormat::JXL,
        FormatKind::Tiff => foundation::image_detection::DetectedFormat::TIFF,
        FormatKind::Qoi => foundation::image_detection::DetectedFormat::QOI,
        FormatKind::Jp2 => foundation::image_detection::DetectedFormat::JP2,
        FormatKind::Ico => foundation::image_detection::DetectedFormat::ICO,
        FormatKind::Exr => foundation::image_detection::DetectedFormat::EXR,
        FormatKind::Flif => foundation::image_detection::DetectedFormat::FLIF,
        FormatKind::Psd => foundation::image_detection::DetectedFormat::PSD,
        FormatKind::Pnm => foundation::image_detection::DetectedFormat::PNM,
        FormatKind::Dds => foundation::image_detection::DetectedFormat::DDS,
        // Video containers and unknown bytes are never static-image candidates.
        // Exhaustive arms on purpose: adding a new FormatKind must be a conscious
        // decision here, not a silent fallthrough to "animated".
        FormatKind::Mp4
        | FormatKind::Mov
        | FormatKind::Mkv
        | FormatKind::Webm
        | FormatKind::Unknown => return Ok(false),
    };

    foundation::image_detection::detect_animation(path, &detected_format)
        .map(|(is_animated, _, _)| !is_animated)
        .with_context(|| {
            format!(
                "FastImg could not read static-container metadata for {}",
                path.display()
            )
        })
}

fn validate_fast_img_options(options: &FastImgRunOptions<'_>) -> anyhow::Result<()> {
    if options.extreme_precision {
        println!(
            "[FASTIMG ] --extreme-precision is reserved for the JXL-to-AVIF recovery path; Meme Mode keeps its bounded coarse-plus-binary search"
        );
    }
    if let Some(output_dir) = options.output_dir {
        anyhow::bail!(
            "--output is not supported by fast-img; the adjacent target-format output is fixed by source path ({} ignored)",
            output_dir.display()
        );
    }
    if options.delete_source.0 {
        tracing::warn!(
            target: "fast_img",
            "--delete-source is redundant; fast-img always deletes verified source files"
        );
    }
    if options.auto_import.0 && !options.shortest_path.0 {
        anyhow::bail!("--auto-import requires --shortest-path");
    }
    if options.strategy == "jxl" && options.shortest_path.0 {
        anyhow::bail!(
            "JXL shortest-path Photos import is disabled because it can leave Photos/iCloud uploads stuck. No files were changed. Use local reversible JXL encoding without --shortest-path, or use --strategy avif --shortest-path for verified Photos import."
        );
    }
    if options.strategy == "avif"
        && let Err(error) = foundation::tools::require(&["avifenc", "avifdec"])
    {
        foundation::log_fatal!(
            foundation::infra::static_logs::messages::LABEL_TOOLS,
            &error
        );
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }
    Ok(())
}

struct FastImgSourceInventory {
    source_files: Vec<PathBuf>,
    scan_failures: BTreeMap<String, String>,
    modern_lossy_candidates: Vec<ModernLossyStaticCandidate>,
    source_hashes: BTreeMap<String, String>,
    planned_encode_count: usize,
}

fn scan_fast_img_sources(
    candidates: Vec<PathBuf>,
    src_dir: &Path,
    strategy: &str,
) -> anyhow::Result<FastImgSourceInventory> {
    if strategy == "avif" {
        println!(
            "[FASTIMG ] enumerating static image containers in {} (no quality scan)",
            src_dir.display()
        );
    } else {
        println!(
            "[FASTIMG ] selecting true JPEG containers in {}",
            src_dir.display()
        );
    }

    let mut source_files = Vec::new();
    let mut scan_failures = BTreeMap::new();
    let format_identities =
        foundation::image::format_identity::resolve_format_identities(&candidates)
            .context("fast-img batched content identity scan failed")?;
    let modern_lossy_scan = if strategy == "jxl" {
        println!("[TIER 2  ] inspecting modern lossy static sources for verified Photos delivery");
        let modern_paths = candidates
            .iter()
            .zip(&format_identities)
            .filter(|(_, identity)| {
                foundation::image::modern_lossy_static::is_modern_static_image_format(
                    identity.family,
                )
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        scan_modern_lossy_static_candidates(src_dir, &modern_paths)?
    } else {
        foundation::ModernLossyStaticScan::default()
    };
    for (path, reason) in &modern_lossy_scan.probe_failures {
        println!(
            "[RETAIN  ] modern source could not be classified safely and will remain: {} ({reason})",
            path.display()
        );
    }
    for (path, identity) in candidates.into_iter().zip(format_identities) {
        if identity.extension_mismatch {
            tracing::warn!(
                target: "format_identity",
                path = %path.display(),
                content_family = ?identity.family,
                extension = ?identity.extension_hint,
                "file extension disagrees with content; content identity retained"
            );
        }
        if strategy == "avif" {
            let format = identity.family;
            if !matches!(
                format,
                FormatKind::Mp4
                    | FormatKind::Mov
                    | FormatKind::Mkv
                    | FormatKind::Webm
                    | FormatKind::Unknown
            ) {
                match fast_img_container_is_static(&path, format) {
                    Ok(true) => source_files.push(path),
                    Ok(false) => println!(
                        "[SKIP    ] Meme Mode rejects animated image container {}",
                        path.display()
                    ),
                    Err(error) => {
                        println!(
                            "[ERROR   ] Failed to read static-container metadata for {}: {}",
                            path.display(),
                            error
                        );
                        let rel_key = fast_img_scan_failure_rel_key(&path, src_dir)?;
                        scan_failures.insert(rel_key, error.to_string());
                        source_files.push(path);
                    }
                }
            }
        } else if identity.family == FormatKind::Jpeg {
            source_files.push(path);
        }
    }
    let source_hashes = fast_img_source_hash_set(src_dir, &source_files)?;
    let planned_encode_count = source_files.len().saturating_sub(scan_failures.len());
    if strategy == "avif" {
        println!(
            "[FASTIMG ] selected {} static images in {}",
            planned_encode_count,
            src_dir.display()
        );
        println!(
            "[MEME    ] AVIF quality search: q=100..0; use pure-media budget with smallest verified pure-media fallback"
        );
    } else {
        println!(
            "[FASTIMG ] selected {} true JPEGs in {}",
            source_files.len(),
            src_dir.display()
        );
    }
    Ok(FastImgSourceInventory {
        source_files,
        scan_failures,
        modern_lossy_candidates: modern_lossy_scan.candidates,
        source_hashes,
        planned_encode_count,
    })
}

fn run_fast_img(options: FastImgRunOptions<'_>) -> anyhow::Result<()> {
    validate_fast_img_options(&options)?;
    let FastImgRunOptions {
        input,
        output_dir: _,
        delete_source: _,
        dry_run,
        recursive,
        auto_import,
        shortest_path,
        retry,
        fresh,
        archive,
        allow_expert_options,
        strategy,
        extreme_precision: _,
    } = options;
    let output_format_name = if strategy == "avif" { "AVIF" } else { "JXL" };
    let mut retry_requested = retry.0;

    let input_plan = FastImgInputPlan::from_input(input, recursive.0)?;
    let src_dir = input_plan.src_root;
    let _source_lock = foundation::acquire_dir_lock(&src_dir).with_context(|| {
        format!(
            "fast-img could not acquire exclusive lock for {}",
            src_dir.display()
        )
    })?;
    let working_copy = if fresh.0 {
        foundation::pipeline::verification::resolve_fresh_working_copy_dir(&src_dir)
    } else {
        fast_img_resolve_working_copy_for_run(&src_dir, dry_run)?
    };
    let _working_copy_lock = if dry_run.0 {
        None
    } else {
        std::fs::create_dir_all(&working_copy).with_context(|| {
            format!(
                "fast-img could not create output directory before locking {}",
                working_copy.display()
            )
        })?;
        Some(
            foundation::acquire_dir_lock(&working_copy).with_context(|| {
                format!(
                    "fast-img could not acquire exclusive output lock for {}",
                    working_copy.display()
                )
            })?,
        )
    };

    let mut existing_marker = read_existing_fast_img_marker(&working_copy)?;
    let FastImgSourceInventory {
        source_files: source_jpegs,
        scan_failures,
        modern_lossy_candidates,
        source_hashes: current_source_hashes,
        planned_encode_count,
    } = scan_fast_img_sources(input_plan.candidates, &src_dir, strategy)?;
    if let Some(marker) = &existing_marker
        && marker.stage != FastImgStageName::CleanupComplete
        && fast_img_marker_input_state_is_stale(
            marker,
            &src_dir,
            source_jpegs.len(),
            &current_source_hashes,
            strategy,
        )?
    {
        if dry_run.0 {
            println!(
                "[DRY-RUN ] existing {} marker has stale inputs; would archive {} and rebuild",
                marker.stage.as_str(),
                working_copy.display()
            );
        } else {
            if retry_requested {
                anyhow::bail!(
                    "MFB_RESUME_INPUT_CHANGED: --retry cannot resume fast-img because the source path, strategy, relative paths, or BLAKE3 identities no longer match the saved task; use --no-resume for an isolated new output directory"
                );
            }
            fast_img_require_interactive_confirmation(
                "[STATE   ] Saved fast-img state does not match the media currently at this path. Archive the saved output and start a new task from the current media? [y/N] ",
                "MFB_RESUME_DECISION_REQUIRED: current inputs differ from saved state; no files were archived. Rerun with --no-resume to start an isolated new task",
            )?;
        }
        if !dry_run.0 {
            if let Some(archived) = fast_img_archive_stale_working_copy(&working_copy)? {
                println!(
                    "[RESUME  ] existing {} marker has stale inputs; archived prior output at {} and rebuilding",
                    marker.stage.as_str(),
                    archived.display()
                );
                tracing::warn!(
                    target: "fast_img",
                    stage = %marker.stage.as_str(),
                    working_copy = %working_copy.display(),
                    archived = %archived.display(),
                    marker_count = marker.src_jpeg_count,
                    source_count = source_jpegs.len(),
                    "fast-img stale marker was archived before rebuilding from current sources"
                );
            } else {
                println!(
                    "[RESUME  ] existing {} marker has stale inputs, but prior output is already absent; rebuilding",
                    marker.stage.as_str()
                );
                tracing::warn!(
                    target: "fast_img",
                    stage = %marker.stage.as_str(),
                    working_copy = %working_copy.display(),
                    marker_count = marker.src_jpeg_count,
                    source_count = source_jpegs.len(),
                    "fast-img stale marker had no working copy; rebuilding from current sources"
                );
            }
        }
        existing_marker = None;
    }

    if let Some(marker) = &existing_marker
        && marker.stage == FastImgStageName::CleanupComplete
        && !(strategy == "jxl"
            && marker.src_jpeg_count == 0
            && source_jpegs.is_empty()
            && !marker.tier2_imported_assets.is_empty()
            && (!modern_lossy_candidates.is_empty() || marker.tier2_in_progress))
    {
        match fast_img_cleanup_complete_source_state(
            marker,
            source_jpegs.len(),
            &current_source_hashes,
        ) {
            Ok(FastImgCleanupCompleteSourceState::RestoredOriginal) => {
                if !dry_run.0 {
                    if retry_requested {
                        anyhow::bail!(
                            "MFB_RESUME_INPUT_CHANGED: --retry cannot reopen a completed fast-img task after its original sources were restored; use --no-resume for an isolated new task"
                        );
                    }
                    fast_img_require_interactive_confirmation(
                        "[STATE   ] This path has a completed fast-img task, but its original source media are present again. Archive the completed output and start a new task? [y/N] ",
                        "MFB_RESUME_DECISION_REQUIRED: restored original sources were not treated as a new task; no files were archived. Rerun with --no-resume to start an isolated new task",
                    )?;
                    if let Some(archived) = fast_img_archive_stale_working_copy(&working_copy)? {
                        println!(
                            "[RESUME  ] archived completed output at {} before rebuilding restored sources",
                            archived.display()
                        );
                    } else {
                        println!(
                            "[RESUME  ] completed output at {} is already absent; rebuilding restored sources",
                            working_copy.display()
                        );
                    }
                }
                println!(
                    "[RESUME  ] existing cleanup marker belongs to a completed run, but original source files were restored; rebuilding from restored sources"
                );
                tracing::warn!(
                    target: "fast_img",
                    working_copy = %working_copy.display(),
                    source_count = source_jpegs.len(),
                    "fast-img cleanup marker ignored because original source files were restored after cleanup"
                );
                existing_marker = None;
            }
            Ok(FastImgCleanupCompleteSourceState::DeletedConverted) => {}
            Ok(FastImgCleanupCompleteSourceState::StaleCurrent) => {
                if !dry_run.0 {
                    if retry_requested {
                        anyhow::bail!(
                            "MFB_RESUME_INPUT_CHANGED: --retry cannot reopen a completed fast-img task because current relative paths or BLAKE3 identities differ; use --no-resume for an isolated new task"
                        );
                    }
                    fast_img_require_interactive_confirmation(
                        "[STATE   ] This path has a completed fast-img task, but the current media differ from its recorded inputs. Archive the completed output and start a new task? [y/N] ",
                        "MFB_RESUME_DECISION_REQUIRED: changed sources were not treated as a new task; no files were archived. Rerun with --no-resume to start an isolated new task",
                    )?;
                    if let Some(archived) = fast_img_archive_stale_working_copy(&working_copy)? {
                        println!(
                            "[RESUME  ] archived stale completed output at {} before rebuilding",
                            archived.display()
                        );
                    } else {
                        println!(
                            "[RESUME  ] stale completed output at {} is already absent; rebuilding",
                            working_copy.display()
                        );
                    }
                }
                println!(
                    "[RESUME  ] existing cleanup marker is stale for the current source set; rebuilding from current sources"
                );
                tracing::warn!(
                    target: "fast_img",
                    working_copy = %working_copy.display(),
                    source_count = source_jpegs.len(),
                    "fast-img cleanup marker ignored because current source files no longer match the completed run"
                );
                existing_marker = None;
            }
            Err(err) => return Err(err),
        }
    }

    if let Some(marker) = &existing_marker
        && !dry_run.0
        && !retry_requested
        && fast_img_requires_resume_decision(marker, shortest_path)
    {
        let prompt = format!(
            "[STATE   ] Found a saved fast-img task at stage {}. Its source path, relative paths, and recorded BLAKE3 identities match. Reconcile durable state and resume it? [y/N] ",
            marker.stage.as_str()
        );
        fast_img_require_interactive_confirmation(
            &prompt,
            "MFB_RESUME_DECISION_REQUIRED: saved task was not resumed; no state was changed. Rerun with --retry to resume or --no-resume to start an isolated new task",
        )?;
        retry_requested = true;
    }

    if let Some(marker) = &existing_marker
        && !dry_run.0
        && !retry_requested
        && fast_img_completed_marker_has_new_tier2_work(marker, modern_lossy_candidates.len())
    {
        fast_img_require_interactive_confirmation(
            "[STATE   ] A completed fast-img task exists, and modern lossy static media are present at the same source path. Reconcile each file with Photos and run a new verified Tier-2 delivery? [y/N] ",
            "MFB_RESUME_DECISION_REQUIRED: completed state was not reused for new Tier-2 media; no Photos import or source deletion was attempted. Rerun with --retry to proceed or --no-resume to isolate a new task",
        )?;
        retry_requested = true;
    }

    if strategy == "jxl"
        && (existing_marker
            .as_ref()
            .is_some_and(|marker| marker.tier2_in_progress)
            || !modern_lossy_candidates.is_empty())
        && existing_marker
            .as_ref()
            .is_some_and(|marker| marker.stage == FastImgStageName::CleanupComplete)
    {
        if dry_run.0 {
            println!(
                "[DRY-RUN ] would reconcile/import and custody-verify {} modern lossy static source(s)",
                modern_lossy_candidates.len()
            );
            return Ok(());
        }
        let Some(marker) = existing_marker.as_mut() else {
            anyhow::bail!("fast-img cleanup-complete marker disappeared before tier-2 delivery");
        };
        if marker.tier2_in_progress && !retry_requested {
            anyhow::bail!(
                "MFB_RESUME_DECISION_REQUIRED: fast-img tier-2 Photos delivery was interrupted; rerun with --retry to reconcile Photos custody and resume cleanup"
            );
        }
        let (deleted, already_deleted, pruned) =
            fast_img_deliver_modern_lossy_static_tier(marker, &src_dir, &modern_lossy_candidates)?;
        println!(
            "[TIER 2  ] verified Photos delivery complete; source deleted={deleted} already_absent={already_deleted} empty_dirs_pruned={pruned}"
        );
        return Ok(());
    }

    if let Some(marker) = &existing_marker
        && !retry_requested
        && fast_img_requires_resume_decision(marker, shortest_path)
    {
        anyhow::bail!(
            "MFB_RESUME_DECISION_REQUIRED: fast-img stopped at {}; rerun with --retry to continue or --no-resume to start in a new output directory",
            marker.stage.as_str()
        );
    }

    let retry_failed_sources_from_cleanup = if let Some(marker) = existing_marker.as_mut()
        && marker.stage == FastImgStageName::CleanupComplete
        && !marker.failed_sources.is_empty()
    {
        validate_cleanup_complete_marker(
            marker,
            &src_dir,
            source_jpegs.len(),
            &current_source_hashes,
        )?;
        if !retry_requested {
            anyhow::bail!(
                "fast-img previous cleanup completed with {} failed source(s); rerun with --retry to retry retained source files",
                marker.failed_sources.len()
            );
        }
        println!(
            "[RESUME  ] existing cleanup marker contains {} failed source(s); retrying retained source files",
            marker.failed_sources.len()
        );
        tracing::warn!(
            target: "fast_img",
            working_copy = %working_copy.display(),
            failed_sources = marker.failed_sources.len(),
            "fast-img cleanup marker retained successes and reopened failed-source retry"
        );
        marker.stage = FastImgStageName::OutputPrepared;
        marker.error = None;
        marker.gate1_checks = Gate1Checks::default();
        marker.gate2_checks = Gate2Checks::default();
        marker.gate3_checks = Gate3Checks::default();
        true
    } else {
        false
    };
    let retry_failed_sources_before_cleanup = if let Some(marker) = existing_marker.as_mut()
        && marker.stage != FastImgStageName::CleanupComplete
        && retry_requested
        && !marker.failed_sources.is_empty()
    {
        println!(
            "[RESUME  ] retrying {} retained source failure(s) before cleanup",
            marker.failed_sources.len()
        );
        marker.stage = FastImgStageName::OutputPrepared;
        marker.error = None;
        marker.gate1_checks = Gate1Checks::default();
        marker.gate2_checks = Gate2Checks::default();
        marker.gate3_checks = Gate3Checks::default();
        true
    } else {
        false
    };

    let resume_local_delivery_for_shortest_path = if let Some(marker) = &existing_marker
        && marker.stage == FastImgStageName::CleanupComplete
    {
        let resume_local_delivery_for_shortest_path =
            fast_img_cleanup_complete_should_resume_shortest_path_import(marker, shortest_path);
        validate_cleanup_complete_marker(
            marker,
            &src_dir,
            source_jpegs.len(),
            &current_source_hashes,
        )?;
        if !fast_img_marker_outputs_current(marker)? && !source_jpegs.is_empty() {
            println!(
                "[RESUME  ] existing cleanup marker has missing/drifted {output_format_name} output; rebuilding from source files"
            );
            tracing::warn!(
                target: "fast_img",
                working_copy = %working_copy.display(),
                "fast-img cleanup marker output proof is not current; rebuilding because source files still exist"
            );
            false
        } else if resume_local_delivery_for_shortest_path {
            println!(
                "[RESUME  ] existing {output_format_name}-only delivery will continue to shortest-path Photos import"
            );
            true
        } else {
            println!(
                "[DONE    ] existing cleanup_complete marker at {}",
                working_copy.display()
            );
            return Ok(());
        }
    } else {
        false
    };
    if let Some(marker) = &existing_marker
        && foundation::pipeline::verification::stage_requires_retry(&marker.stage)
        && !retry_requested
    {
        anyhow::bail!(
            "fast-img previous run stopped at {}; inspect {} or rerun with --retry",
            marker.stage.as_str(),
            working_copy.display()
        );
    }
    if dry_run.0 {
        if strategy == "avif" {
            println!(
                "[DRY-RUN ] would encode {} static images from {} into AVIF-only output {}",
                planned_encode_count,
                src_dir.display(),
                working_copy.display()
            );
        } else {
            println!(
                "[DRY-RUN ] would encode {} JPEGs from {} into JXL-only output {}",
                source_jpegs.len(),
                src_dir.display(),
                working_copy.display()
            );
            if !modern_lossy_candidates.is_empty() {
                println!(
                    "[DRY-RUN ] would reconcile/import and custody-verify {} modern lossy static source(s) in Photos",
                    modern_lossy_candidates.len()
                );
            }
        }
        return Ok(());
    }

    let reuse_marker_import_proof = existing_marker
        .as_ref()
        .is_some_and(fast_img_reuses_marker_import_proof_on_resume);
    let saved_dir_timestamps = foundation::save_directory_timestamps(&src_dir)
        .with_context(|| format!("snapshot fast-img directory metadata {}", src_dir.display()))?;

    let mut marker = existing_marker.unwrap_or_else(|| {
        WorkingCopyMarker::new(src_dir.clone(), working_copy.clone(), source_jpegs.len())
            .with_strategy(strategy.to_string())
    });
    if retry_failed_sources_from_cleanup {
        validate_cleanup_retry_marker_source_state(
            &marker,
            &src_dir,
            source_jpegs.len(),
            &current_source_hashes,
        )?;
    } else if resume_local_delivery_for_shortest_path {
        validate_cleanup_complete_marker(
            &marker,
            &src_dir,
            source_jpegs.len(),
            &current_source_hashes,
        )?;
    } else {
        validate_fast_img_marker_source_state(
            &marker,
            &src_dir,
            source_jpegs.len(),
            &current_source_hashes,
        )?;
    }
    let mut resume_stage = if resume_local_delivery_for_shortest_path {
        FastImgStageName::Gate1Passed
    } else {
        retry_resume_stage(&marker.stage, retry_requested)
    };
    let refresh_jxl_metadata =
        encode_complete_or_later(&resume_stage) && !gate1_complete_or_later(&resume_stage);
    let previous_resume_stage = marker.stage.clone();
    if !resume_local_delivery_for_shortest_path
        && fast_img_downgrade_resume_if_outputs_stale(&mut marker, &mut resume_stage)?
    {
        println!(
            "[RESUME  ] existing marker has missing/drifted {output_format_name} output; rebuilding local {output_format_name} outputs"
        );
        tracing::warn!(
            target: "fast_img",
            stage = %previous_resume_stage.as_str(),
            working_copy = %working_copy.display(),
            "fast-img marker output proof is not current; downgrading resume stage to output_prepared"
        );
    }
    if !resume_local_delivery_for_shortest_path && !retry_failed_sources_from_cleanup {
        marker.src_jpeg_count = source_jpegs.len();
    }
    marker.stage = if output_prepared_or_later(&resume_stage) {
        resume_stage
    } else {
        FastImgStageName::ScanComplete
    };
    marker.error = None;
    write_marker_atomic(&marker)?;

    if marker.stage == FastImgStageName::ScanComplete {
        let msg = fast_img_delete_notice_message(
            planned_encode_count,
            modern_lossy_candidates.len(),
            &src_dir,
            strategy,
        );
        println!("{msg}");
        tracing::info!(target: "fast_img", message = %msg, "fast-img delete notice acknowledged automatically");
    }

    if !output_prepared_or_later(&marker.stage) {
        println!(
            "[PREPARE ] {output_format_name} output {}",
            working_copy.display()
        );
        prepare_jxl_output_dir(&working_copy).with_context(|| {
            format!(
                "create fast-img adjacent {output_format_name} output directory {}",
                working_copy.display()
            )
        })?;
        marker.stage = FastImgStageName::OutputPrepared;
        write_marker_atomic(&marker)?;
    }

    fast_img_refresh_and_persist_marker_deliveries(
        &mut marker,
        &src_dir,
        strategy,
        refresh_jxl_metadata,
    )?;

    if !encode_complete_or_later(&marker.stage) {
        fast_img_run_encode_phase(
            &mut marker,
            &source_jpegs,
            &current_source_hashes,
            &scan_failures,
            &src_dir,
            &working_copy,
            RetryFlag(retry_failed_sources_from_cleanup || retry_failed_sources_before_cleanup),
            ArchiveFlag(archive),
            ExpertOptionsFlag(allow_expert_options),
            strategy,
        )?;
    }

    foundation::restore_delivery_directory_metadata(&saved_dir_timestamps, &src_dir, &working_copy)
        .with_context(|| {
            format!(
                "restore fast-img directory metadata {} -> {} before Gate 1",
                src_dir.display(),
                working_copy.display()
            )
        })?;

    fast_img_run_verification_and_delivery_pipeline(
        &mut marker,
        &source_jpegs,
        &current_source_hashes,
        &src_dir,
        &working_copy,
        &saved_dir_timestamps,
        retry_failed_sources_from_cleanup,
        resume_local_delivery_for_shortest_path,
        shortest_path,
        auto_import,
        reuse_marker_import_proof,
        &modern_lossy_candidates,
        strategy,
    )?;

    Ok(())
}

const fn fast_img_post_gate1_policy(shortest_path: ShortestPathFlag) -> FastImgPostGate1Policy {
    if shortest_path.0 {
        FastImgPostGate1Policy::ShortestPathImportAndVerify
    } else {
        FastImgPostGate1Policy::LocalOnlyDelivery
    }
}

fn fast_img_reuses_marker_import_proof_on_resume(marker: &WorkingCopyMarker) -> bool {
    matches!(
        marker.stage,
        FastImgStageName::Gate2Failed | FastImgStageName::Gate3Failed
    ) && fast_img_marker_has_complete_import_proof(marker)
}

fn fast_img_requires_resume_decision(
    marker: &WorkingCopyMarker,
    shortest_path: ShortestPathFlag,
) -> bool {
    marker.tier2_in_progress
        || marker.stage != FastImgStageName::CleanupComplete
        || !marker.failed_sources.is_empty()
        || fast_img_cleanup_complete_should_resume_shortest_path_import(marker, shortest_path)
}

fn fast_img_require_interactive_confirmation(prompt: &str, declined: &str) -> anyhow::Result<()> {
    if foundation::fast_img::prompt_user_confirm(prompt)? {
        Ok(())
    } else {
        anyhow::bail!("{declined}")
    }
}

fn fast_img_completed_marker_has_new_tier2_work(
    marker: &WorkingCopyMarker,
    modern_lossy_candidate_count: usize,
) -> bool {
    marker.stage == FastImgStageName::CleanupComplete
        && !marker.tier2_in_progress
        && modern_lossy_candidate_count != 0
}

fn fast_img_marker_has_complete_import_proof(marker: &WorkingCopyMarker) -> bool {
    marker.blake3_log.len() == marker.expected_output_count()
        && marker.photos_imported_assets.len() == marker.expected_output_count()
        && marker.blake3_log.iter().all(|(_source_rel, entry)| {
            let Some(out_rel) = entry.out_rel.as_deref() else {
                return false;
            };
            entry
                .library_asset
                .as_ref()
                .is_some_and(|hash| *hash == entry.out)
                && marker.photos_imported_assets.iter().any(|asset| {
                    asset.rel_path == out_rel
                        && asset.blake3 == entry.out
                        && asset
                            .photos_uuid
                            .as_deref()
                            .is_some_and(|uuid| !uuid.is_empty())
                })
        })
}

fn fast_img_delete_notice_message(
    jpeg_count: usize,
    modern_lossy_count: usize,
    src_dir: &Path,
    strategy: &str,
) -> String {
    let mode_name = if strategy == "avif" {
        "AVIF-only (Meme Mode)"
    } else {
        "JXL-only"
    };
    let source_type_plural = if strategy == "avif" {
        "static images"
    } else {
        "JPEGs"
    };
    let source_type_singular = if strategy == "avif" {
        "static image"
    } else {
        "JPEG"
    };

    let tier2_notice = if modern_lossy_count > 0 {
        format!(
            " It will also import, custody-verify, and delete {modern_lossy_count} modern lossy static source file(s)."
        )
    } else {
        String::new()
    };
    format!(
        "[NOTICE  ] fast-img {mode_name} delivery for {jpeg_count} {source_type_plural} from {}. \
         This workflow will directly delete original {source_type_singular} files after strict verification.{tier2_notice} \
         Back up the source folder first if you need to keep them.",
        src_dir.display()
    )
}

const fn fast_img_cleanup_complete_has_shortest_path_proof(marker: &WorkingCopyMarker) -> bool {
    marker.gate2_checks.count.0
        && marker.gate2_checks.blake3_sample.0
        && marker.gate2_checks.no_error.0
        && marker.gate3_checks.count_x3.0
        && marker.gate3_checks.sync.0
        && marker.gate3_checks.quarantine.0
        && marker.gate3_checks.chain.0
}

fn fast_img_cleanup_complete_should_resume_shortest_path_import(
    marker: &WorkingCopyMarker,
    shortest_path: ShortestPathFlag,
) -> bool {
    shortest_path.0
        && marker.stage == FastImgStageName::CleanupComplete
        && !fast_img_cleanup_complete_has_shortest_path_proof(marker)
}

fn fast_img_effective_expected_count(
    marker: &WorkingCopyMarker,
    _current_count: usize,
    _resume_local_delivery_for_shortest_path: bool,
) -> usize {
    marker.expected_output_count()
}

fn fast_img_pipeline_ctx(
    marker: &WorkingCopyMarker,
    expected_count: usize,
    library_handle: Option<LibraryHandle>,
    output_format: Option<foundation::image::format_detect::FormatKind>,
) -> PipelineCtx {
    PipelineCtx {
        working_copy: marker.working_copy.clone(),
        src_dir: marker.src_dir.clone(),
        blake3_log: marker.blake3_log.clone(),
        expected_count,
        library_handle,
        output_format,
    }
}

#[derive(Clone)]
struct FastImgTranscodeJob {
    source: PathBuf,
    src_hash: String,
    rel_key: String,
    out_rel_key: String,
}

/// Group digits in threes for exact-byte display ("1,324,056,789").
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (digits.len() - idx).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

/// "1.23 GiB (1,324,056,789 bytes)" — binary units, exact bytes alongside.
fn exact_bytes_label(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = foundation::numeric_cast::u64_to_f64(bytes);
    let human = if value >= KIB * KIB * KIB {
        format!("{:.2} GiB", value / (KIB * KIB * KIB))
    } else if value >= KIB * KIB {
        format!("{:.2} MiB", value / (KIB * KIB))
    } else if value >= KIB {
        format!("{:.2} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    };
    format!("{human} ({} bytes)", group_thousands(bytes))
}

/// Session-scoped size accounting: ONLY files encoded in THIS run.
/// Skipped (already exists / resume-reused), failed, and deferred items are
/// excluded — their bytes never enter these sums.
fn print_fast_img_session_size_summary(
    files_converted: u64,
    source_bytes_actual: u64,
    output_bytes_actual: u64,
    resume_reused: u64,
) -> anyhow::Result<()> {
    if files_converted == 0 && resume_reused == 0 {
        println!("[SIZE    ] no files converted in this session");
        return Ok(());
    }
    if files_converted == 0 && resume_reused > 0 {
        println!("[SIZE    ] resume_reused_count:  {resume_reused}");
    }
    let source_i64 =
        foundation::numeric_cast::u64_to_i64_strict(source_bytes_actual, "session_source_bytes")
            .context("session source bytes exceed i64 range")?;
    let output_i64 =
        foundation::numeric_cast::u64_to_i64_strict(output_bytes_actual, "session_output_bytes")
            .context("session output bytes exceed i64 range")?;
    let saved_bytes = source_i64 - output_i64;
    // Cross-check the printed delta against its operands (subtraction above is the
    // definition; this guards future refactors that decouple them).
    anyhow::ensure!(
        saved_bytes == source_i64 - output_i64,
        "size summary cross-check failed: saved {saved_bytes} != {source_i64} - {output_i64}"
    );
    if output_bytes_actual > source_bytes_actual {
        println!(
            "[WARN    ] session output is larger than source (expansion, not an error): +{}",
            exact_bytes_label(output_bytes_actual - source_bytes_actual)
        );
    }
    if source_bytes_actual == 0 {
        // Converted files whose sources stat to zero bytes: ratio undefined.
        println!(
            "[SIZE    ] converted {files_converted} file(s); source bytes 0 — no ratio computed"
        );
        return Ok(());
    }
    let saved_pct = (foundation::numeric_cast::i64_to_f64(saved_bytes)
        / foundation::numeric_cast::u64_to_f64(source_bytes_actual))
        * 100.0;
    println!("[SIZE    ] files_converted:      {files_converted}");
    println!(
        "[SIZE    ] source_bytes_actual:  {}",
        exact_bytes_label(source_bytes_actual)
    );
    println!(
        "[SIZE    ] output_bytes_actual:  {}",
        exact_bytes_label(output_bytes_actual)
    );
    let saved_label = if saved_bytes < 0 {
        format!("-{}", exact_bytes_label(saved_bytes.unsigned_abs()))
    } else {
        exact_bytes_label(saved_bytes.unsigned_abs())
    };
    println!("[SIZE    ] saved_bytes:          {saved_label}");
    println!("[SIZE    ] saved_pct:            {saved_pct:.1}%");
    Ok(())
}

fn fast_img_marker_delivery_byte_totals(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
) -> anyhow::Result<(u64, u64)> {
    let mut source_bytes = 0u64;
    let mut output_bytes = 0u64;
    for (rel, entry) in &marker.blake3_log {
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        if source.exists() {
            let len = std::fs::metadata(&source)
                .with_context(|| format!("stat fast-img delivery source {}", source.display()))?
                .len();
            source_bytes = source_bytes
                .checked_add(len)
                .context("fast-img delivery source byte total overflowed u64")?;
        }
        let output = fast_img_marker_entry_output_path(marker, rel, entry)?;
        if output.exists() {
            let len = std::fs::metadata(&output)
                .with_context(|| format!("stat fast-img delivery output {}", output.display()))?
                .len();
            output_bytes = output_bytes
                .checked_add(len)
                .context("fast-img delivery output byte total overflowed u64")?;
        }
    }
    Ok((source_bytes, output_bytes))
}

fn fast_img_planned_output_rel(
    source: &Path,
    working_copy: &Path,
    rel: &Path,
    strategy: &str,
) -> anyhow::Result<(PathBuf, String)> {
    let ext_name = if strategy == "avif" { "AVIF" } else { "JXL" };
    let naive_out = working_copy.join(rel.with_extension(ext_name));
    let reserved_out = foundation::conversion::reserve_output_path(source, &naive_out);
    let out_rel_key =
        fast_img_output_rel_key(&reserved_out, working_copy, "fast_img_planned_output_rel")?;
    if reserved_out != naive_out {
        println!(
            "[NOTICE  ] {} reserved output {out_rel_key} due to filename collision with an existing reservation or on-disk {ext_name}",
            rel.to_string_lossy()
        );
        tracing::warn!(
            target: "fast_img",
            source_rel = %rel.to_string_lossy(),
            naive = %naive_out.display(),
            reserved = %reserved_out.display(),
            "fast-img pre-reserved disambiguated output path for collision"
        );
    }
    Ok((reserved_out, out_rel_key))
}

fn fast_img_output_rel_key(
    output: &Path,
    working_copy: &Path,
    context: &'static str,
) -> anyhow::Result<String> {
    let out_rel_path =
        foundation::media_conversion_gate::strip_prefix_or_self(output, working_copy, context);
    if out_rel_path == output {
        anyhow::bail!(
            "fast-img reserved output {} is outside working copy {}",
            output.display(),
            working_copy.display()
        );
    }
    Ok(out_rel_path.to_string_lossy().to_string())
}

fn fast_img_emit_explicit_skip(rel_key: &str, reason: &str) {
    println!("[SKIP    ] {rel_key} retained: {reason}");
    tracing::warn!(
        target: "fast_img",
        rel = %rel_key,
        reason = %reason,
        "fast-img explicit source skip"
    );
}

struct FastImgTranscodeProof {
    rel_key: String,
    out_rel: String,
    src_hash: String,
    out_hash: String,
}

struct FastImgSkippedSourceProof {
    rel_key: String,
    src_hash: String,
    reason: String,
}

enum FastImgTranscodeOutcome {
    Converted(FastImgTranscodeProof),
    Skipped(FastImgSkippedSourceProof),
}

struct FastImgTranscodeError {
    rel_key: String,
    out_rel_key: String,
    src_hash: String,
    reason: String,
}

type FastImgJobResult = std::result::Result<FastImgTranscodeOutcome, FastImgTranscodeError>;

fn fast_img_effective_encode_parallelism(
    pending_count: usize,
    configured_parallel_tasks: usize,
    configured_child_threads: usize,
) -> (usize, usize) {
    let parallel_tasks = if pending_count == 0 {
        1
    } else {
        configured_parallel_tasks.clamp(1, pending_count)
    };
    (parallel_tasks, configured_child_threads.max(1))
}

/// Quality exploration result for AVIF Meme Mode.
enum AvifQualityExploreResult {
    /// Highest verified candidate within the source's pure-media budget.
    Found {
        domain: foundation::exploration_policy::EncoderDomain,
        outcome: foundation::exploration_policy::ExplorationOutcome,
        quality: u8,
        temp_path: PathBuf,
        output_size: u64,
        pure_media_size: u64,
        content_blake3: String,
        selection: &'static str,
    },
    /// The source cannot produce a verifiable AVIF at any permitted quality.
    SourceUnavailable { reason: String },
}

struct FastImgAvifEncoderInput {
    path: PathBuf,
    // Keeps the decoded intermediate alive for the lifetime of the input.
    _temp_guard: Option<tempfile::NamedTempFile>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AvifInputDecoder {
    WebP,
    Avif,
    Heif,
    Jxl,
    Jp2,
    ImageMagick,
}

fn avif_decoder_allows_imagemagick_fallback(
    decoder: AvifInputDecoder,
    allow_expert_options: bool,
) -> bool {
    decoder != AvifInputDecoder::ImageMagick && allow_expert_options
}

const fn avif_input_decoder(format: FormatKind) -> Option<AvifInputDecoder> {
    match format {
        FormatKind::WebP => Some(AvifInputDecoder::WebP),
        FormatKind::Avif => Some(AvifInputDecoder::Avif),
        FormatKind::Heic | FormatKind::Heif => Some(AvifInputDecoder::Heif),
        FormatKind::Jxl => Some(AvifInputDecoder::Jxl),
        FormatKind::Jp2 => Some(AvifInputDecoder::Jp2),
        FormatKind::Gif
        | FormatKind::Bmp
        | FormatKind::Tiff
        | FormatKind::Qoi
        | FormatKind::Ico
        | FormatKind::Exr
        | FormatKind::Flif
        | FormatKind::Psd
        | FormatKind::Pnm
        | FormatKind::Dds => Some(AvifInputDecoder::ImageMagick),
        // avifenc consumes JPEG/PNG directly; video containers and unknown
        // bytes are outside the static-AVIF encode domain entirely.
        FormatKind::Jpeg
        | FormatKind::Png
        | FormatKind::Mp4
        | FormatKind::Mov
        | FormatKind::Mkv
        | FormatKind::Webm
        | FormatKind::Unknown => None,
    }
}

fn avif_input_decoder_command(
    decoder: AvifInputDecoder,
    source: &Path,
    temp_path: &Path,
) -> anyhow::Result<(std::process::Command, &'static str)> {
    let command = match decoder {
        AvifInputDecoder::WebP => {
            let mut builder = foundation::image_builders::DwebpBuilder::new();
            builder.input(source).output(temp_path);
            (builder.build(), "dwebp")
        }
        AvifInputDecoder::Avif => {
            let executable =
                foundation::common_utils::resolve_tool_path(foundation::constants::TOOL_AVIFDEC)
                    .context("official avifdec is required for AVIF source normalization")?;
            let mut command = std::process::Command::new(executable);
            command
                .arg("--jobs")
                .arg("all")
                .arg("--depth")
                .arg("16")
                .arg("--")
                .arg(source)
                .arg(temp_path);
            (command, foundation::constants::TOOL_AVIFDEC)
        }
        AvifInputDecoder::Heif => {
            let executable = foundation::common_utils::resolve_tool_path("heif-convert")
                .context("official libheif heif-convert is required for HEIC/HEIF normalization")?;
            let mut command = std::process::Command::new(executable);
            command.arg("--quiet").arg(source).arg(temp_path);
            (command, "heif-convert")
        }
        AvifInputDecoder::Jxl => {
            let mut builder = foundation::DjxlBuilder::new();
            builder.input(source).output(temp_path);
            (builder.build(), foundation::constants::TOOL_DJXL)
        }
        AvifInputDecoder::Jp2 => {
            let executable = foundation::common_utils::resolve_tool_path("opj_decompress")
                .context("official OpenJPEG opj_decompress is required for JP2 normalization")?;
            let mut command = std::process::Command::new(executable);
            command.arg("-i").arg(source).arg("-o").arg(temp_path);
            (command, "opj_decompress")
        }
        AvifInputDecoder::ImageMagick => {
            let mut builder = foundation::MagickBuilder::new();
            let input_spec = format!("{}[0]", source.display());
            builder
                .input(&input_spec)
                .arg("-alpha")
                .arg("set")
                .arg("-strip")
                .depth(16)
                .output(temp_path);
            (builder.build(), "ImageMagick")
        }
    };
    Ok(command)
}

fn run_avif_input_decoder(
    decoder: AvifInputDecoder,
    source: &Path,
    temp_path: &Path,
) -> anyhow::Result<&'static str> {
    if decoder == AvifInputDecoder::ImageMagick {
        let mut identify = foundation::IdentifyBuilder::new();
        identify.format("%n\n").input(source);
        let process_res = foundation::process_runner::ManagedProcess::spawn(&mut identify.build())
            .and_then(|proc| {
                proc.wait_liveness_timeout(
                    std::time::Duration::from_secs(10),
                    foundation::process_runner::image_process_hard_timeout(),
                    "identify frame count",
                )
            });
        let output = process_res.with_context(|| {
            format!(
                "ImageMagick identify failed to run for static decode preflight of {}",
                source.display()
            )
        })?;
        anyhow::ensure!(
            output.status.success(),
            "ImageMagick identify returned non-zero exit status for {}",
            source.display()
        );
        let first_line = output.stdout.lines().next().ok_or_else(|| {
            anyhow::anyhow!(
                "ImageMagick identify produced empty output for {}",
                source.display()
            )
        })?;
        let count: usize = first_line.trim().parse().with_context(|| {
            format!(
                "ImageMagick identify frame count parse failed for '{}' in {}",
                first_line.trim(),
                source.display()
            )
        })?;
        anyhow::ensure!(
            count == 1,
            "ImageMagick static decode refused multi-image input ({count} frames/pages in {})",
            source.display()
        );
    }
    let (mut command, tool_name) = avif_input_decoder_command(decoder, source, temp_path)?;
    let output = foundation::process_runner::ManagedProcess::spawn(&mut command)
        .and_then(|process| {
            process.wait_liveness_timeout(
                std::time::Duration::from_secs(120),
                foundation::process_runner::image_process_hard_timeout(),
                &format!("{tool_name} static decode"),
            )
        })
        .with_context(|| format!("FastImg could not run {tool_name} for {}", source.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "{tool_name} could not decode {}: {}",
            source.display(),
            output.stderr.trim()
        );
    }
    if !temp_path.is_file() {
        anyhow::bail!(
            "{tool_name} reported success without a decoded PNG for {}",
            source.display()
        );
    }
    let decoded_size = std::fs::metadata(temp_path)
        .with_context(|| format!("inspect {tool_name} decoded PNG {}", temp_path.display()))?
        .len();
    if decoded_size == 0 {
        anyhow::bail!(
            "{tool_name} reported success with an empty decoded PNG for {}",
            source.display()
        );
    }
    if !foundation::image::png_validation::is_true_png(temp_path)? {
        anyhow::bail!(
            "{tool_name} output failed strict PNG validation for {}",
            source.display()
        );
    }
    Ok(tool_name)
}

/// Use avifenc-supported JPEG/PNG inputs directly. Other static formats use an
/// authoritative decoder; `ImageMagick` adapters/fallbacks require explicit expert mode.
fn prepare_fast_img_avif_encoder_input(
    source: &Path,
    format: FormatKind,
    allow_expert_options: bool,
) -> anyhow::Result<FastImgAvifEncoderInput> {
    if matches!(format, FormatKind::Jpeg | FormatKind::Png) {
        return Ok(FastImgAvifEncoderInput {
            path: source.to_path_buf(),
            _temp_guard: None,
        });
    }
    let Some(decoder) = avif_input_decoder(format) else {
        anyhow::bail!("FastImg has no static decoder from {format:?} to an avifenc input");
    };
    if decoder == AvifInputDecoder::ImageMagick && !allow_expert_options {
        anyhow::bail!(
            "FastImg {format:?} to AVIF requires an ImageMagick adapter; pass --allow-expert-options to enable this explicit fallback"
        );
    }

    let temp = foundation::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "fast_img_official_avif_input",
        None,
        Some(".png"),
    )
    .context("FastImg could not allocate a temporary PNG for official static decode")?;
    let temp_path = temp.path().to_path_buf();
    let tool_name = match run_avif_input_decoder(decoder, source, &temp_path) {
        Ok(tool_name) => tool_name,
        Err(primary_error)
            if avif_decoder_allows_imagemagick_fallback(decoder, allow_expert_options) =>
        {
            std::fs::write(&temp_path, []).with_context(|| {
                format!(
                    "FastImg could not reset temporary decoder output {}",
                    temp_path.display()
                )
            })?;
            foundation::log_detail!(&format!(
                "FastImg AVIF: authoritative decoder failed for {format:?}; trying ImageMagick fallback: {primary_error}"
            ));
            run_avif_input_decoder(AvifInputDecoder::ImageMagick, source, &temp_path).with_context(
                || {
                    format!(
                        "authoritative decoder failed before ImageMagick fallback: {primary_error}"
                    )
                },
            )?
        }
        Err(error) if decoder != AvifInputDecoder::ImageMagick => {
            return Err(error).context(
                "authoritative decoder failed; ImageMagick fallback is disabled unless --allow-expert-options is set",
            );
        }
        Err(error) => return Err(error),
    };

    foundation::log_detail!(&format!(
        "FastImg AVIF: decoded {format:?} once with {tool_name}: {}",
        source.display()
    ));
    Ok(FastImgAvifEncoderInput {
        path: temp_path,
        _temp_guard: Some(temp),
    })
}

fn avif_quality_probe_error_is_source_invariant(message: &str) -> bool {
    message.contains("pixel-diff: cannot open source image")
        || (message.contains("avifenc failed at q=")
            && (message.contains("Unrecognized file format")
                || message.contains("Unsupported file format")))
}

const AVIF_MEME_SPEED: u8 =
    foundation::exploration_policy::AvifSpeedDomain::MEME_QUALITY_SEARCH.value();

/// Keep every AVIF quality probe in one comparable speed domain.
#[must_use]
const fn avif_meme_speed_domain() -> foundation::exploration_policy::AvifSpeedDomain {
    foundation::exploration_policy::AvifSpeedDomain::MEME_QUALITY_SEARCH
}
pub(crate) use foundation::infra::constants::AVIF_MEME_MIN_QUALITY;

#[derive(Clone, Debug)]
struct AvifMemeCandidate {
    speed_domain: foundation::exploration_policy::AvifSpeedDomain,
    quality: u8,
    temp_path: PathBuf,
    output_size: u64,
    pure_media_size: u64,
    content_blake3: String,
}

#[derive(Debug, Default)]
struct AvifMemeQualityEvidence {
    highest_fitting_quality: Option<u8>,
    lowest_oversize_quality: Option<u8>,
    failed_qualities: BTreeSet<u8>,
}

impl AvifMemeQualityEvidence {
    fn record_fit(&mut self, candidate: &AvifMemeCandidate) {
        debug_assert_eq!(
            candidate.speed_domain,
            avif_meme_speed_domain(),
            "locator candidates must never become final AVIF evidence"
        );
        self.highest_fitting_quality = Some(
            self.highest_fitting_quality
                .map_or(candidate.quality, |current| current.max(candidate.quality)),
        );
    }

    fn record_oversize(&mut self, candidate: &AvifMemeCandidate) {
        debug_assert_eq!(
            candidate.speed_domain,
            avif_meme_speed_domain(),
            "locator candidates must never become final AVIF evidence"
        );
        self.lowest_oversize_quality = Some(
            self.lowest_oversize_quality
                .map_or(candidate.quality, |current| current.min(candidate.quality)),
        );
    }

    fn record_failed(&mut self, quality: u8) {
        self.failed_qualities.insert(quality);
    }

    const fn verified_bracket(&self) -> Option<(u8, u8)> {
        match (self.highest_fitting_quality, self.lowest_oversize_quality) {
            (Some(low), Some(high)) if low < high => Some((low, high)),
            _ => None,
        }
    }

    fn next_refinement_quality(&self) -> Option<u8> {
        let (low, high) = self.verified_bracket()?;
        let midpoint = low + (high - low) / 2;
        (low.saturating_add(1)..high)
            .filter(|quality| !self.failed_qualities.contains(quality))
            .min_by_key(|quality| (quality.abs_diff(midpoint), std::cmp::Reverse(*quality)))
    }
}

fn finish_avif_meme_after_terminal_probe_error(
    reason: &str,
    fitting_candidate: &mut Option<AvifMemeCandidate>,
) -> Option<AvifQualityExploreResult> {
    if !avif_quality_probe_error_is_source_invariant(reason) {
        return None;
    }
    let Some(candidate) = fitting_candidate.take() else {
        return Some(AvifQualityExploreResult::SourceUnavailable {
            reason: reason.to_string(),
        });
    };
    Some(AvifQualityExploreResult::Found {
        domain: candidate.speed_domain.encoder_domain(),
        outcome: foundation::exploration_policy::ExplorationOutcome::ExploredOptimized,
        quality: candidate.quality,
        temp_path: candidate.temp_path,
        output_size: candidate.output_size,
        pure_media_size: candidate.pure_media_size,
        content_blake3: candidate.content_blake3,
        selection: "terminal_probe_fitting_fallback",
    })
}

#[cfg(test)]
pub(crate) fn jpeg_pure_media_size(path: &Path) -> anyhow::Result<u64> {
    foundation::image::static_payload::jpeg(path)
}

#[cfg(test)]
pub(crate) fn png_pure_media_size(path: &Path) -> anyhow::Result<u64> {
    foundation::image::static_payload::png(path)
}

pub(crate) fn avif_mdat_payload_size(path: &Path) -> anyhow::Result<u64> {
    foundation::image::static_payload::isobmff_mdat(path)
}

pub(crate) fn source_pure_media_size(path: &Path, format: FormatKind) -> anyhow::Result<u64> {
    foundation::image::static_payload::measure_as(path, format)
}

#[cfg(test)]
pub(crate) fn jxl_pure_bitstream_size(path: &Path) -> anyhow::Result<u64> {
    foundation::image::static_payload::jxl(path)
}
const fn avif_meme_candidate_fits_source(
    candidate: &AvifMemeCandidate,
    source_pure_media_size: u64,
    policy: foundation::exploration_policy::SizePolicy,
) -> bool {
    policy.fits(candidate.pure_media_size, source_pure_media_size)
}

type AvifMemeProbeOutcome = foundation::exploration_policy::ProbeOutcome<AvifMemeCandidate, String>;

fn probe_single_avif_quality(
    source: &Path,
    encoder_input: &Path,
    quality: u8,
    speed_domain: foundation::exploration_policy::AvifSpeedDomain,
    source_pure_media_size: u64,
    convert_options: &img::lossless_converter::ConvertOptions,
    metadata_retry: &mut img::lossless_converter::AvifencMetadataRetryState,
) -> AvifMemeProbeOutcome {
    foundation::log_detail!(&format!(
        "AVIF Meme Mode quality probe: q={quality} (speed={}) for {}",
        speed_domain.value(),
        source.display()
    ));
    let (temp_path, output_size, content_blake3) = match
        img::lossless_converter::convert_to_avif_verified_probe_from_encoder_input_with_speed_and_state(
            source,
            encoder_input,
            quality,
            Some(speed_domain.value()),
            metadata_retry,
            convert_options,
        )
    {
        Ok(candidate) => candidate,
        Err(error) => return AvifMemeProbeOutcome::Failed(error.to_string()),
    };

    let pure_media_size = match avif_mdat_payload_size(&temp_path) {
        Ok(size) => size,
        Err(error) => {
            foundation::media_conversion_gate::delivery_remove_file_or_audit(
                "fast_img_probe_cleanup",
                &temp_path,
            );
            return AvifMemeProbeOutcome::Unverifiable(format!(
                "pure-media measurement failed: {error}"
            ));
        }
    };

    foundation::log_detail!(&format!(
        "AVIF Meme Mode quality probe: q={quality} verified complete_file_size={output_size}B pure_media_size={pure_media_size}B"
    ));
    let candidate = AvifMemeCandidate {
        speed_domain,
        quality,
        temp_path,
        output_size,
        pure_media_size,
        content_blake3,
    };
    let active_policy = foundation::exploration_policy::SizePolicy::strict_or_allow_growth(
        convert_options.effective_allow_size_tolerance(),
        foundation::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
    );
    if avif_meme_candidate_fits_source(&candidate, source_pure_media_size, active_policy) {
        AvifMemeProbeOutcome::Fits(candidate)
    } else {
        AvifMemeProbeOutcome::Oversize(candidate)
    }
}

/// Find a bounded quality anchor. The locator candidate is always discarded;
/// only its coordinate is reused, and the final speed=0 domain must probe it
/// again before it can become delivery evidence.
fn locate_avif_meme_quality(
    source: &Path,
    encoder_input: &Path,
    source_pure_media_size: u64,
    convert_options: &img::lossless_converter::ConvertOptions,
    metadata_retry: &mut img::lossless_converter::AvifencMetadataRetryState,
) -> Option<u8> {
    const COARSE_STEP: u8 = 10;
    let speed_domain = foundation::exploration_policy::AvifSpeedDomain::MEME_QUALITY_LOCATOR;
    let mut quality = 100_u8;
    loop {
        let outcome = probe_single_avif_quality(
            source,
            encoder_input,
            quality,
            speed_domain,
            source_pure_media_size,
            convert_options,
            metadata_retry,
        );
        match outcome {
            AvifMemeProbeOutcome::Fits(candidate) => {
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "fast_img_locator_probe_cleanup",
                    &candidate.temp_path,
                );
                return Some(quality);
            }
            AvifMemeProbeOutcome::Oversize(candidate) => {
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "fast_img_locator_probe_cleanup",
                    &candidate.temp_path,
                );
            }
            AvifMemeProbeOutcome::Failed(_) | AvifMemeProbeOutcome::Unverifiable(_) => {}
        }
        if quality == AVIF_MEME_MIN_QUALITY {
            return None;
        }
        quality = quality.saturating_sub(COARSE_STEP);
    }
}

fn explore_avif_meme_quality(
    source: &Path,
    format: FormatKind,
    encoder_input: &Path,
    convert_options: &img::lossless_converter::ConvertOptions,
    metadata_retry: &mut img::lossless_converter::AvifencMetadataRetryState,
) -> AvifQualityExploreResult {
    const COARSE_STEP: u8 = 10;
    let source_pure_media_size = match source_pure_media_size(source, format) {
        Ok(size) => size,
        Err(error) => {
            return AvifQualityExploreResult::SourceUnavailable {
                reason: format!(
                    "cannot measure source pure-media payload; preserving source without complete-file fallback: {error}"
                ),
            };
        }
    };

    let mut evidence = AvifMemeQualityEvidence::default();
    let mut coarse_quality = 100_u8;
    let mut locator_attempted = false;
    let mut locator_needed = false;
    let mut locator_quality = None;
    let mut current_passed_candidate: Option<AvifMemeCandidate> = None;
    let mut last_probe_error = None;

    // Phase 1: Coarse step probing (100 down to 0)
    loop {
        match probe_single_avif_quality(
            source,
            encoder_input,
            coarse_quality,
            avif_meme_speed_domain(),
            source_pure_media_size,
            convert_options,
            metadata_retry,
        ) {
            AvifMemeProbeOutcome::Fits(candidate) => {
                evidence.record_fit(&candidate);
                if candidate.quality == 100 {
                    return AvifQualityExploreResult::Found {
                        domain: avif_meme_speed_domain().encoder_domain(),
                        outcome:
                            foundation::exploration_policy::ExplorationOutcome::ExploredOptimized,
                        quality: candidate.quality,
                        temp_path: candidate.temp_path,
                        output_size: candidate.output_size,
                        pure_media_size: candidate.pure_media_size,
                        content_blake3: candidate.content_blake3,
                        selection: "highest_verified_fitting",
                    };
                }
                current_passed_candidate = Some(candidate);
                break;
            }
            AvifMemeProbeOutcome::Oversize(candidate) => {
                evidence.record_oversize(&candidate);
                if coarse_quality == 100 {
                    locator_needed = true;
                }
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "fast_img_probe_cleanup",
                    &candidate.temp_path,
                );
            }
            AvifMemeProbeOutcome::Failed(reason) | AvifMemeProbeOutcome::Unverifiable(reason) => {
                evidence.record_failed(coarse_quality);
                if let Some(result) = finish_avif_meme_after_terminal_probe_error(
                    &reason,
                    &mut current_passed_candidate,
                ) {
                    return result;
                }
                last_probe_error = Some(reason);
            }
        }

        if coarse_quality == AVIF_MEME_MIN_QUALITY {
            break;
        }
        if coarse_quality == 100 && locator_needed && !locator_attempted {
            locator_attempted = true;
            locator_quality = locate_avif_meme_quality(
                source,
                encoder_input,
                source_pure_media_size,
                convert_options,
                metadata_retry,
            );
        }
        coarse_quality = if coarse_quality == 100 {
            locator_quality
                .filter(|quality| *quality < 100)
                .unwrap_or_else(|| coarse_quality.saturating_sub(COARSE_STEP))
        } else {
            coarse_quality.saturating_sub(COARSE_STEP)
        };
    }

    // Phase 2: refine only inside a bracket proven by a fitting probe and a
    // distinct oversized probe. Failed probes are remembered solely to avoid
    // repeating the same point; they never move either size boundary.
    for _ in 0..img::lossless_converter::AVIF_QUALITY_BINARY_PROBE_BUDGET {
        let Some(mid) = evidence.next_refinement_quality() else {
            break;
        };
        match probe_single_avif_quality(
            source,
            encoder_input,
            mid,
            avif_meme_speed_domain(),
            source_pure_media_size,
            convert_options,
            metadata_retry,
        ) {
            AvifMemeProbeOutcome::Fits(candidate) => {
                evidence.record_fit(&candidate);
                if let Some(old) = current_passed_candidate.replace(candidate) {
                    foundation::media_conversion_gate::delivery_remove_file_or_audit(
                        "fast_img_probe_cleanup",
                        &old.temp_path,
                    );
                }
            }
            AvifMemeProbeOutcome::Oversize(candidate) => {
                evidence.record_oversize(&candidate);
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "fast_img_probe_cleanup",
                    &candidate.temp_path,
                );
            }
            AvifMemeProbeOutcome::Failed(reason) | AvifMemeProbeOutcome::Unverifiable(reason) => {
                evidence.record_failed(mid);
                if let Some(result) = finish_avif_meme_after_terminal_probe_error(
                    &reason,
                    &mut current_passed_candidate,
                ) {
                    return result;
                }
                last_probe_error = Some(reason);
            }
        }
    }

    if let Some(final_candidate) = current_passed_candidate {
        let selection = if evidence.failed_qualities.is_empty() {
            "highest_verified_fitting"
        } else {
            "highest_verified_fitting_with_probe_gaps"
        };
        return AvifQualityExploreResult::Found {
            domain: final_candidate.speed_domain.encoder_domain(),
            outcome: foundation::exploration_policy::ExplorationOutcome::ExploredOptimized,
            quality: final_candidate.quality,
            temp_path: final_candidate.temp_path,
            output_size: final_candidate.output_size,
            pure_media_size: final_candidate.pure_media_size,
            content_blake3: final_candidate.content_blake3,
            selection,
        };
    }

    AvifQualityExploreResult::SourceUnavailable {
        reason: format!(
            "no AVIF candidate passed the Meme Mode quality gate at q=100..={AVIF_MEME_MIN_QUALITY}{}",
            last_probe_error
                .as_deref()
                .map_or_else(String::new, |reason| format!(
                    "; last probe error: {reason}"
                ))
        ),
    }
}

fn fast_img_adopt_static_avif(
    source: &Path,
    output: &Path,
) -> anyhow::Result<Option<foundation::TaskResult>> {
    foundation::ensure_parent_dir_exists(output)?;
    let temp_output = foundation::conversion::temp_path_for_output(output);
    let _temp_guard = foundation::conversion::TempOutputGuard::new(temp_output.clone());
    let input_size = std::fs::metadata(source)
        .with_context(|| format!("stat Meme Mode source AVIF {}", source.display()))?
        .len();
    let copied = std::fs::copy(source, &temp_output).with_context(|| {
        format!(
            "copy Meme Mode source AVIF unchanged {} -> {}",
            source.display(),
            temp_output.display()
        )
    })?;
    anyhow::ensure!(
        copied == input_size,
        "Meme Mode source AVIF copy length mismatch: expected={input_size} copied={copied}"
    );
    if let Err(err) = foundation::metadata::verify_output_embedded_metadata(
        source,
        &temp_output,
        foundation::metadata::MetadataOutputPolicy::Clear,
    ) {
        tracing::info!(
            target: "fast_img",
            source = %source.display(),
            error = %err,
            "Existing AVIF is not eligible for byte-exact adoption; re-encoding with clear metadata policy"
        );
        return Ok(None);
    }
    foundation::io_utils::robust_move(&temp_output, output).with_context(|| {
        format!(
            "commit adopted Meme Mode AVIF {} -> {}",
            temp_output.display(),
            output.display()
        )
    })?;
    Ok(Some(foundation::TaskResult::converted_with_message(
        source,
        output,
        input_size,
        copied,
        "Meme Mode adopted a metadata-clean static AVIF without re-encoding",
    )))
}

const fn avif_lossless_candidate_fits_source(
    candidate_pure_media_size: u64,
    source_pure_media_size: u64,
    policy: foundation::exploration_policy::SizePolicy,
) -> bool {
    policy.fits(candidate_pure_media_size, source_pure_media_size)
}

fn try_fast_img_lossless_avif(
    source: &Path,
    source_format: FormatKind,
    encoder_input: &Path,
    options: &foundation::ConvertOptions,
    metadata_retry: &mut img::lossless_converter::AvifencMetadataRetryState,
) -> anyhow::Result<Option<foundation::TaskResult>> {
    let source_pure_media_size = source_pure_media_size(source, source_format)?;
    let probe =
        img::lossless_converter::convert_to_avif_verified_lossless_probe_from_encoder_input_with_speed_and_state(
            source,
            encoder_input,
            AVIF_MEME_SPEED,
            metadata_retry,
            options,
        );
    let (temp_path, complete_file_size, content_blake3) = match probe {
        Ok(candidate) => candidate,
        Err(error) => {
            foundation::media_conversion_gate::delivery_api_batch_fallback_audit(
                "fast_img_lossless_avif_probe",
                format!(
                    "True-lossless AVIF probe unavailable for {}; continuing with explicit Meme lossy intent: {error}",
                    source.display()
                ),
            );
            return Ok(None);
        }
    };
    let candidate_pure_media_size = match avif_mdat_payload_size(&temp_path) {
        Ok(size) => size,
        Err(error) => {
            foundation::media_conversion_gate::delivery_remove_file_or_audit(
                "fast_img_lossless_probe_cleanup",
                &temp_path,
            );
            return Err(error);
        }
    };
    let active_policy = foundation::exploration_policy::SizePolicy::strict_or_allow_growth(
        options.effective_allow_size_tolerance(),
        foundation::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
    );
    if !avif_lossless_candidate_fits_source(
        candidate_pure_media_size,
        source_pure_media_size,
        active_policy,
    ) {
        foundation::log_detail!(&format!(
            "Meme Mode true-lossless AVIF is outside the active size policy: complete_file={complete_file_size}B pure_media={candidate_pure_media_size}B source_pure_media={source_pure_media_size}B; continuing with q-boundary exploration"
        ));
        foundation::media_conversion_gate::delivery_remove_file_or_audit(
            "fast_img_lossless_probe_oversize_cleanup",
            &temp_path,
        );
        return Ok(None);
    }
    let result = img::lossless_converter::finalize_meme_avif_probe(
        source,
        &temp_path,
        &content_blake3,
        options,
    )?
    .with_optimization_outcome(
        foundation::exploration_policy::ExplorationOutcome::LosslessTranscoded,
    );
    foundation::log_detail!(&format!(
        "Meme Mode accepted true-lossless AVIF at speed={AVIF_MEME_SPEED}: pure_media={candidate_pure_media_size}B < source={source_pure_media_size}B"
    ));
    Ok(Some(result))
}

fn fast_img_run_encode_job_inner(
    job: &FastImgTranscodeJob,
    src_dir: &Path,
    working_copy: &Path,
    child_threads: usize,
    archive: bool,
    allow_expert_options: bool,
    strategy: &str,
) -> anyhow::Result<FastImgTranscodeOutcome> {
    let result = if strategy == "avif" {
        let format = foundation::image::format_detect::detect_true_format(&job.source)?;
        let adopted = if format == FormatKind::Avif {
            fast_img_adopt_static_avif(&job.source, &working_copy.join(&job.out_rel_key))?
        } else {
            None
        };
        if let Some(result) = adopted {
            result
        } else {
            let convert_options = foundation::ConvertOptions {
                output_dir: Some(working_copy.to_path_buf()),
                base_dir: Some(src_dir.to_path_buf()),
                flags: foundation::ConvertFlags::FORCE,
                ..Default::default()
            };

            let encoder_input =
                prepare_fast_img_avif_encoder_input(&job.source, format, allow_expert_options)
                    .with_context(|| {
                        format!(
                            "AVIF target encoding unavailable after official decode for {}",
                            job.source.display()
                        )
                    })?;
            let mut metadata_retry =
                img::lossless_converter::AvifencMetadataRetryState::strip_all();
            let detected_source = foundation::image_detection::detect_format_from_bytes(
                &job.source,
            )
            .with_context(|| {
                format!(
                    "detect Meme Mode source format for {}",
                    job.source.display()
                )
            })?;
            let source_compression =
                foundation::image_detection::detect_compression(&detected_source, &job.source)
                    .with_context(|| {
                        format!(
                            "classify Meme Mode source compression for {}",
                            job.source.display()
                        )
                    })?;
            let Some(source_is_lossless) = fast_img_avif_source_is_lossless(source_compression)
            else {
                return Ok(FastImgTranscodeOutcome::Skipped(
                    FastImgSkippedSourceProof {
                        rel_key: job.rel_key.clone(),
                        src_hash: fast_img_verify_source_hash_unchanged(
                            &job.source,
                            &job.src_hash,
                        )?,
                        reason: format!(
                            "compression semantics are {source_compression:?}; retaining source because lossy AVIF re-encoding is not proven safe"
                        ),
                    },
                ));
            };
            let lossless_result = if source_is_lossless {
                try_fast_img_lossless_avif(
                    &job.source,
                    format,
                    &encoder_input.path,
                    &convert_options,
                    &mut metadata_retry,
                )?
            } else {
                None
            };
            if let Some(result) = lossless_result {
                result
            } else {
                match explore_avif_meme_quality(
                    &job.source,
                    format,
                    &encoder_input.path,
                    &convert_options,
                    &mut metadata_retry,
                ) {
                    AvifQualityExploreResult::Found {
                        domain,
                        outcome,
                        quality,
                        temp_path,
                        output_size,
                        pure_media_size,
                        content_blake3,
                        selection,
                    } => {
                        foundation::log_detail!(&format!(
                            "Meme Mode (AVIF): {outcome:?} selected {domain:?} q={quality} selection={selection} complete_file_size={output_size}B pure_media_size={pure_media_size}B for {} ({format:?})",
                            job.source.display()
                        ));
                        img::lossless_converter::finalize_meme_avif_probe(
                            &job.source,
                            &temp_path,
                            &content_blake3,
                            &convert_options,
                        )?
                        .with_optimization_outcome(outcome)
                    }
                    AvifQualityExploreResult::SourceUnavailable { reason } => {
                        anyhow::bail!(
                            "AVIF source preflight could not produce a verifiable target output for {}: {reason}",
                            job.source.display()
                        );
                    }
                }
            }
        }
    } else {
        let options = LosslessConvertOptions {
            output_dir: Some(working_copy.to_path_buf()),
            base_dir: Some(src_dir.to_path_buf()),
            flags: LosslessConvertFlags::FORCE
                | LosslessConvertFlags::APPLE_COMPAT
                | LosslessConvertFlags::REQUIRE_JPEG_RECONSTRUCTION
                | LosslessConvertFlags::REQUIRE_OUTPUT_DELIVERY
                | LosslessConvertFlags::ULTIMATE
                | if archive {
                    LosslessConvertFlags::ARCHIVE
                } else {
                    LosslessConvertFlags::empty()
                }
                | if allow_expert_options {
                    LosslessConvertFlags::ALLOW_EXPERT_OPTIONS
                } else {
                    LosslessConvertFlags::empty()
                },
            child_threads,
            input_format: Some("JPEG".to_string()),
            quality_label: None,
            codec: foundation::conversion_types::SelectedCodec::default(),
        };
        convert_jpeg_to_jxl(&job.source, &options, None)?
    };
    if result.skipped && result.output_path.is_none() {
        return Ok(FastImgTranscodeOutcome::Skipped(
            FastImgSkippedSourceProof {
                rel_key: job.rel_key.clone(),
                src_hash: calculate_blake3_hash(&job.source)?,
                reason: result.message,
            },
        ));
    }
    let out_path = result.output_path.as_ref().map(Path::new).ok_or_else(|| {
        anyhow::anyhow!(
            "encode produced no output path for {}",
            job.source.display()
        )
    })?;

    let is_avif_output = strategy == "avif";
    let orientation_tolerance = if is_avif_output {
        pixel_equivalence_diff_tolerance_for_format(FormatKind::Avif).ok_or_else(|| {
            anyhow::anyhow!("missing shared orientation tolerance for AVIF output")
        })?
    } else {
        pixel_equivalence_diff_tolerance_for_format(FormatKind::Jxl)
            .ok_or_else(|| anyhow::anyhow!("missing shared orientation tolerance for JXL output"))?
    };
    let output_format = if is_avif_output {
        FormatKind::Avif
    } else {
        FormatKind::Jxl
    };
    match verify_orientation_pixel_diff(
        &job.source,
        out_path,
        output_format,
        orientation_tolerance,
    )? {
        PixelDiffResult::Match => {}
        PixelDiffResult::SkippedToolAbsent { tool } => {
            anyhow::bail!(
                "orientation pixel diff unavailable for {}: missing {tool}",
                out_path.display()
            );
        }
        PixelDiffResult::Mismatch { max_delta, channel } => {
            anyhow::bail!(
                "orientation pixel diff failed for {}: max_delta={max_delta} channel={channel}",
                out_path.display()
            );
        }
    }

    let src_hash = match fast_img_verify_source_hash_unchanged(&job.source, &job.src_hash) {
        Ok(hash) => hash,
        Err(source_error) => {
            match std::fs::remove_file(out_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(cleanup_error) => {
                    return Err(anyhow::anyhow!(
                        "{source_error}; additionally failed to remove rejected output {}: {cleanup_error}",
                        out_path.display()
                    ));
                }
            }
            return Err(source_error);
        }
    };
    let out_hash = calculate_blake3_hash(out_path)?;
    // Use the planned relative key as the primary identity. Only override it
    // when the converter reports a different in-tree output path, e.g. a
    // collision suffix from reserve_unique_output_path.
    let planned_out_path = working_copy.join(&job.out_rel_key);
    let actual_out_rel = if out_path == planned_out_path {
        job.out_rel_key.clone()
    } else {
        let rel = foundation::media_conversion_gate::strip_prefix_or_self(
            out_path,
            working_copy,
            "fast_img_actual_output_rel",
        );
        if rel == out_path {
            job.out_rel_key.clone()
        } else {
            rel.to_string_lossy().to_string()
        }
    };
    Ok(FastImgTranscodeOutcome::Converted(FastImgTranscodeProof {
        rel_key: job.rel_key.clone(),
        out_rel: actual_out_rel,
        src_hash,
        out_hash,
    }))
}

fn fast_img_verify_source_hash_unchanged(
    source: &Path,
    expected_hash: &str,
) -> anyhow::Result<String> {
    let actual_hash = calculate_blake3_hash(source)
        .with_context(|| format!("hash fast-img source after encode: {}", source.display()))?;
    anyhow::ensure!(
        actual_hash == expected_hash,
        "fast-img source changed while it was being encoded: {} (before={expected_hash}, after={actual_hash}); output rejected and source retained",
        source.display()
    );
    Ok(actual_hash)
}

fn fast_img_run_encode_job(
    job: &FastImgTranscodeJob,
    src_dir: &Path,
    working_copy: &Path,
    child_threads: usize,
    archive: bool,
    allow_expert_options: bool,
    strategy: &str,
) -> FastImgJobResult {
    fast_img_run_encode_job_inner(
        job,
        src_dir,
        working_copy,
        child_threads,
        archive,
        allow_expert_options,
        strategy,
    )
    .map_err(|err| FastImgTranscodeError {
        rel_key: job.rel_key.clone(),
        out_rel_key: job.out_rel_key.clone(),
        src_hash: job.src_hash.clone(),
        reason: err.to_string(),
    })
}

fn fast_img_remove_failed_encode_output(
    working_copy: &Path,
    err: &FastImgTranscodeError,
) -> anyhow::Result<()> {
    let out_rel = fast_img_checked_rel_path(&err.out_rel_key)?;
    let primary_output = working_copy.join(&out_rel);
    match std::fs::remove_file(&primary_output) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "remove failed fast-img output before Gate 1: {}",
                    primary_output.display()
                )
            });
        }
    }

    tracing::warn!(
        target: "fast_img",
        source_rel = %err.rel_key,
        primary_output = %primary_output.display(),
        "removed failed fast-img output before continuing batch"
    );
    Ok(())
}

fn fast_img_refresh_reused_jxl_delivery(source: &Path, output: &Path) -> anyhow::Result<String> {
    let committed = foundation::conversion::commit_temp_to_output_with_metadata(
        output,
        output,
        true,
        Some(source),
    )
    .with_context(|| {
        format!(
            "refresh fast-img reused JXL delivery metadata {} -> {}",
            source.display(),
            output.display()
        )
    })?;
    anyhow::ensure!(
        committed,
        "refresh fast-img reused JXL delivery metadata returned skipped for {}",
        output.display()
    );
    let refreshed_hash = calculate_blake3_hash(output).with_context(|| {
        format!(
            "hash refreshed fast-img reused JXL output {}",
            output.display()
        )
    })?;
    tracing::info!(
        target: "fast_img",
        source = %source.display(),
        output = %output.display(),
        output_blake3 = %refreshed_hash,
        "fast-img reused JXL delivery metadata refreshed"
    );
    Ok(refreshed_hash)
}
pub const FAST_IMG_AVIF_CLEAN_POLICY_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReuseDecision {
    Reusable { hash: String },
    NeedsReencode { reason: String },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RefreshSummary {
    refreshed: usize,
    invalidated: usize,
    marker_changed: bool,
}

fn fast_img_reset_delivery_gate_proofs(marker: &mut WorkingCopyMarker, stage: FastImgStageName) {
    marker.stage = stage;
    marker.gate1_checks = Gate1Checks::default();
    marker.gate2_checks = Gate2Checks::default();
    marker.gate3_checks = Gate3Checks::default();
    marker.photos_imported_assets.clear();
    for entry in marker.blake3_log.values_mut() {
        entry.library_asset = None;
    }
    marker.error = None;
}

fn fast_img_downgrade_stale_resume_marker(
    marker: &mut WorkingCopyMarker,
    resume_stage: &mut FastImgStageName,
) {
    *resume_stage = FastImgStageName::OutputPrepared;
    fast_img_reset_delivery_gate_proofs(marker, FastImgStageName::OutputPrepared);
}

fn fast_img_downgrade_resume_if_outputs_stale(
    marker: &mut WorkingCopyMarker,
    resume_stage: &mut FastImgStageName,
) -> anyhow::Result<bool> {
    if !encode_complete_or_later(resume_stage) || fast_img_marker_outputs_current(marker)? {
        return Ok(false);
    }
    fast_img_downgrade_stale_resume_marker(marker, resume_stage);
    Ok(true)
}

fn fast_img_check_reused_delivery(
    output: &Path,
    strategy: &str,
    marker_policy_ver: u32,
    expected_output_hash: &str,
) -> anyhow::Result<ReuseDecision> {
    let metadata = match std::fs::metadata(output) {
        Ok(m) => m,
        Err(_) => {
            return Ok(ReuseDecision::NeedsReencode {
                reason: format!("output is missing or unreadable: {}", output.display()),
            });
        }
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return Ok(ReuseDecision::NeedsReencode {
            reason: format!(
                "output is missing or not a non-empty regular file: {}",
                output.display()
            ),
        });
    }

    let label = if strategy == "avif" { "AVIF" } else { "JXL" };
    if strategy == "avif" && marker_policy_ver < FAST_IMG_AVIF_CLEAN_POLICY_VERSION {
        return Ok(ReuseDecision::NeedsReencode {
            reason: format!(
                "legacy AVIF marker policy version ({marker_policy_ver} < {FAST_IMG_AVIF_CLEAN_POLICY_VERSION}); forcing clean re-encode"
            ),
        });
    }
    if expected_output_hash.is_empty() {
        return Ok(ReuseDecision::NeedsReencode {
            reason: format!("recorded {label} output hash proof is empty; forcing re-encode"),
        });
    }
    let actual_hash = calculate_blake3_hash(output)?;
    if actual_hash != expected_output_hash {
        return Ok(ReuseDecision::NeedsReencode {
            reason: format!(
                "{label} output BLAKE3 mismatch (recorded: {expected_output_hash}, actual: {actual_hash}); forcing clean re-encode"
            ),
        });
    }
    Ok(ReuseDecision::Reusable {
        hash: expected_output_hash.to_string(),
    })
}

fn fast_img_refresh_marker_deliveries(
    marker: &mut WorkingCopyMarker,
    src_dir: &Path,
    strategy: &str,
    refresh_jxl_metadata: bool,
) -> anyhow::Result<RefreshSummary> {
    fast_img_refresh_marker_deliveries_with(
        marker,
        src_dir,
        strategy,
        refresh_jxl_metadata,
        fast_img_refresh_reused_jxl_delivery,
    )
}

fn fast_img_refresh_marker_deliveries_with<F>(
    marker: &mut WorkingCopyMarker,
    src_dir: &Path,
    strategy: &str,
    refresh_jxl_metadata: bool,
    refresh_jxl: F,
) -> anyhow::Result<RefreshSummary>
where
    F: Fn(&Path, &Path) -> anyhow::Result<String>,
{
    let target_ext = if strategy == "avif" { "AVIF" } else { "JXL" };
    let mut summary = RefreshSummary::default();
    let mut refreshed_output_changed = false;
    for (rel, entry) in &mut marker.blake3_log {
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        let out_rel = if let Some(out_rel) = entry.out_rel.as_deref() {
            fast_img_checked_rel_path(out_rel)?
        } else {
            let derived = fast_img_checked_rel_path(rel)?.with_extension(target_ext);
            entry.out_rel = Some(derived.to_string_lossy().to_string());
            summary.marker_changed = true;
            derived
        };
        let output = marker.working_copy.join(&out_rel);
        let mut decision = fast_img_check_reused_delivery(
            &output,
            strategy,
            marker.metadata_policy_version,
            &entry.out,
        )?;
        if strategy != "avif"
            && refresh_jxl_metadata
            && matches!(decision, ReuseDecision::Reusable { .. })
        {
            anyhow::ensure!(
                source.is_file(),
                "fast-img JXL metadata refresh requires missing source {}; preserving output and marker proof",
                source.display()
            );
            decision = match refresh_jxl(&source, &output) {
                Ok(hash) => ReuseDecision::Reusable { hash },
                Err(err) => ReuseDecision::NeedsReencode {
                    reason: format!("JXL delivery refresh failed: {err}"),
                },
            };
        }
        match decision {
            ReuseDecision::Reusable { hash } => {
                if entry.out != hash {
                    if !entry.out.is_empty() {
                        refreshed_output_changed = true;
                    }
                    entry.library_asset = None;
                    entry.out = hash;
                    summary.marker_changed = true;
                }
                summary.refreshed += 1;
            }
            ReuseDecision::NeedsReencode { reason } => {
                anyhow::ensure!(
                    source.is_file(),
                    "fast-img output requires re-encode but source is missing {}; preserving output and marker proof ({reason})",
                    source.display()
                );
                tracing::warn!(
                    target: "fast_img",
                    rel = %rel,
                    reason = %reason,
                    "marker delivery refresh invalidated output; clearing recorded proof for re-encode"
                );
                entry.out.clear();
                entry.library_asset = None;
                summary.marker_changed = true;
                foundation::media_conversion_gate::delivery_remove_file_or_audit(
                    "fast_img invalidate obsolete reused output",
                    &output,
                );
                summary.invalidated += 1;
            }
        }
    }

    if summary.invalidated > 0 {
        fast_img_reset_delivery_gate_proofs(marker, FastImgStageName::OutputPrepared);
        summary.marker_changed = true;
    } else if refreshed_output_changed {
        let stage = if gate1_complete_or_later(&marker.stage) {
            FastImgStageName::TranscodeComplete
        } else {
            marker.stage.clone()
        };
        fast_img_reset_delivery_gate_proofs(marker, stage);
        summary.marker_changed = true;
    }

    Ok(summary)
}

fn fast_img_refresh_and_persist_marker_deliveries(
    marker: &mut WorkingCopyMarker,
    src_dir: &Path,
    strategy: &str,
    refresh_jxl_metadata: bool,
) -> anyhow::Result<RefreshSummary> {
    let summary =
        fast_img_refresh_marker_deliveries(marker, src_dir, strategy, refresh_jxl_metadata)?;
    if summary.marker_changed {
        write_marker_atomic(marker)?;
    }
    Ok(summary)
}

fn fast_img_validate_recorded_source_hashes_current(
    marker: &WorkingCopyMarker,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    if fast_img_cleanup_resume_source_subset_matches(marker, current_source_hashes)? {
        return Ok(());
    }
    let marker_source_hashes = fast_img_marker_recorded_source_hashes(marker)?;
    for (rel, recorded_hash) in &marker_source_hashes {
        match current_source_hashes.get(rel) {
            Some(current) if current == recorded_hash => {}
            Some(_) => anyhow::bail!("fast-img source BLAKE3 changed before delivery for {rel}"),
            None => anyhow::bail!(
                "fast-img recorded source {rel} missing from current scan before delivery"
            ),
        }
    }
    Ok(())
}

fn fast_img_cleanup_resume_source_subset_matches(
    marker: &WorkingCopyMarker,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<bool> {
    if !matches!(
        marker.stage,
        FastImgStageName::Gate1Passed | FastImgStageName::Gate3Passed
    ) {
        return Ok(false);
    }
    let recorded = fast_img_marker_recorded_source_hashes(marker)?;
    if current_source_hashes
        .iter()
        .any(|(rel, hash)| recorded.get(rel) != Some(hash))
    {
        return Ok(false);
    }
    Ok(marker
        .skipped_sources
        .iter()
        .chain(marker.failed_sources.iter())
        .all(|(rel, entry)| current_source_hashes.get(rel) == Some(&entry.src)))
}

fn fast_img_validate_source_delete_disposition(marker: &WorkingCopyMarker) -> anyhow::Result<bool> {
    marker
        .validate_source_disposition_disjoint()
        .map_err(|err| anyhow::anyhow!("fast-img source delete disposition overlap: {err}"))?;
    if marker.source_disposition_over_recorded() {
        anyhow::bail!(
            "fast-img source delete gate over-recorded disposition: marker_count={} converted={} skipped={} failed={}",
            marker.src_jpeg_count,
            marker.blake3_log.len(),
            marker.skipped_sources.len(),
            marker.failed_sources.len()
        );
    }
    let complete = marker.source_disposition_is_complete();
    if !complete {
        tracing::warn!(
            target: "fast_img_delete",
            marker_count = marker.src_jpeg_count,
            converted = marker.blake3_log.len(),
            skipped = marker.skipped_sources.len(),
            failed = marker.failed_sources.len(),
            unaccounted = marker
                .src_jpeg_count
                .saturating_sub(marker.recorded_source_count()),
            "fast-img source delete gate proceeding with incomplete disposition; only blake3_log sources are eligible for deletion"
        );
    }
    Ok(complete)
}

fn fast_img_reconcile_unrecorded_source_disposition(
    marker: &mut WorkingCopyMarker,
    src_dir: &Path,
    source_jpegs: &[PathBuf],
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<usize> {
    let mut reconciled = 0usize;
    for source in source_jpegs {
        let rel = source.strip_prefix(src_dir).map_err(|err| {
            anyhow::anyhow!(
                "fast-img reconcile produced path outside source root: source={} root={} ({err})",
                source.display(),
                src_dir.display()
            )
        })?;
        let rel_key = rel.to_string_lossy().to_string();
        if marker.blake3_log.contains_key(&rel_key)
            || marker.skipped_sources.contains_key(&rel_key)
            || marker.failed_sources.contains_key(&rel_key)
        {
            continue;
        }
        let src_hash = current_source_hashes.get(&rel_key).ok_or_else(|| {
            anyhow::anyhow!("missing scanned source hash for reconcile of {rel_key}")
        })?;
        marker.skipped_sources.insert(
            rel_key.clone(),
            SkippedSourceEntry {
                src: src_hash.clone(),
                reason: "fast-img encode left source without disposition record; source retained unmodified"
                    .to_string(),
            },
        );
        reconciled += 1;
        println!("[SKIP    ] {rel_key} retained: encode disposition was not recorded");
        tracing::warn!(
            target: "fast_img",
            rel = %rel_key,
            "fast-img reconciled unrecorded source disposition as explicit skip"
        );
    }
    Ok(reconciled)
}

fn fast_img_validate_jxl_only_delivery_exit(
    marker: &WorkingCopyMarker,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
    strategy: &str,
) -> anyhow::Result<()> {
    let mode_name = if strategy == "avif" {
        "AVIF-only (Meme Mode)"
    } else {
        "JXL-only"
    };
    if marker.src_jpeg_count != current_count
        && !fast_img_cleanup_resume_source_subset_matches(marker, current_source_hashes)?
    {
        anyhow::bail!(
            "fast-img source count changed before {mode_name} delivery: marker={} current={current_count}",
            marker.src_jpeg_count
        );
    }
    fast_img_validate_source_delete_disposition(marker)?;
    fast_img_validate_recorded_source_hashes_current(marker, current_source_hashes)?;
    if let Some((rel, _entry)) = marker
        .blake3_log
        .iter()
        .find(|(_rel, entry)| entry.out_rel.is_none() || entry.out.is_empty())
    {
        anyhow::bail!("fast-img {mode_name} output hash incomplete for {rel}");
    }
    if !fast_img_marker_outputs_current(marker)? {
        anyhow::bail!(
            "fast-img {} output proof missing/drifted before delivery",
            if strategy == "avif" { "AVIF" } else { "JXL" }
        );
    }

    Ok(())
}

fn fast_img_validate_cleanup_retry_jxl_only_delivery_exit(
    marker: &WorkingCopyMarker,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
    strategy: &str,
) -> anyhow::Result<()> {
    if current_count != current_source_hashes.len() {
        anyhow::bail!(
            "fast-img cleanup retry source hash count mismatch: current={current_count} hashes={}",
            current_source_hashes.len()
        );
    }
    if marker.recorded_source_count() != marker.src_jpeg_count {
        fast_img_validate_source_delete_disposition(marker)?;
    }
    let marker_source_hashes = fast_img_marker_recorded_source_hashes(marker)?;
    for (rel, current_hash) in current_source_hashes {
        let Some(recorded_hash) = marker_source_hashes.get(rel) else {
            anyhow::bail!("fast-img cleanup retry current source missing marker proof for {rel}");
        };
        if recorded_hash != current_hash {
            anyhow::bail!("fast-img cleanup retry retained source BLAKE3 changed for {rel}");
        }
    }
    if let Some((rel, _entry)) = marker
        .blake3_log
        .iter()
        .find(|(_rel, entry)| entry.out_rel.is_none() || entry.out.is_empty())
    {
        anyhow::bail!("fast-img cleanup retry output hash incomplete for {rel}");
    }
    if !fast_img_marker_outputs_current(marker)? {
        anyhow::bail!(
            "fast-img cleanup retry {} output proof missing/drifted before delivery",
            if strategy == "avif" { "AVIF" } else { "JXL" }
        );
    }
    Ok(())
}

fn fast_img_marker_outputs_current(marker: &WorkingCopyMarker) -> anyhow::Result<bool> {
    if marker.blake3_log.len() != marker.expected_output_count() {
        return Ok(false);
    }
    for (rel, entry) in &marker.blake3_log {
        if entry.out_rel.is_none() || entry.out.is_empty() {
            return Ok(false);
        }
        let output = fast_img_marker_entry_output_path(marker, rel, entry)?;
        let output_metadata = match std::fs::metadata(&output) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => {
                return Err(anyhow::anyhow!(
                    "stat fast-img marker JXL output {}: {err}",
                    output.display()
                ));
            }
        };
        if output_metadata.len() == 0 {
            return Ok(false);
        }
        let output_hash = calculate_blake3_hash(&output)?;
        if output_hash != entry.out {
            return Ok(false);
        }
    }
    Ok(true)
}

fn fast_img_verified_output_format(output: &Path, strategy: &str) -> anyhow::Result<FormatKind> {
    let expected = foundation::delivery_codec_strategy::strategy_to_format_kind(strategy)
        .ok_or_else(|| anyhow::anyhow!("unsupported fast-img strategy {strategy:?}"))?;
    let actual = foundation::image::format_detect::detect_true_format(output)
        .with_context(|| format!("detect fast-img output format {}", output.display()))?;
    anyhow::ensure!(
        actual == expected,
        "fast-img output content format mismatch for {}: expected {expected:?}, detected {actual:?}",
        output.display()
    );
    Ok(actual)
}

fn fast_img_delete_verified_source_jpegs(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    strategy: &str,
) -> anyhow::Result<(usize, usize)> {
    fast_img_preflight_verified_source_deletion(marker, src_dir)?;
    let mut candidates = Vec::new();
    for (rel, entry) in &marker.blake3_log {
        candidates.push(FastImgSourceDeleteCandidate {
            source: src_dir.join(fast_img_checked_rel_path(rel)?),
            output: fast_img_marker_entry_output_path(marker, rel, entry)?,
        });
    }

    let existing = candidates
        .iter()
        .filter(|candidate| candidate.source.exists())
        .collect::<Vec<_>>();
    let mut verified = Vec::new();
    if !existing.is_empty() {
        let thread_config = foundation::thread_manager::get_balanced_thread_config(
            foundation::thread_manager::WorkloadType::Image,
        );
        let parallelism =
            fast_img_effective_verify_parallelism(existing.len(), thread_config.parallel_tasks);
        let expected_format =
            foundation::delivery_codec_strategy::strategy_to_format_kind(strategy)
                .ok_or_else(|| anyhow::anyhow!("unsupported fast-img strategy {strategy:?}"))?;
        let (format_label, tool_label) = match expected_format {
            FormatKind::Avif => ("AVIF", "avifdec"),
            FormatKind::Jxl => ("JXL", "djxl"),
            _ => anyhow::bail!("unsupported fast-img output format {expected_format:?}"),
        };
        println!(
            "[VERIFY  ] final {} delete proofs pending {} · parallel {} {} checks",
            format_label,
            existing.len(),
            parallelism,
            tool_label
        );
        tracing::info!(
            target: "fast_img_delete",
            pending = existing.len(),
            parallelism,
            "fast-img final {} delete proof verification start",
            format_label
        );
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(parallelism)
            .build()
            .map_err(|err| anyhow::anyhow!("fast-img verify thread pool init failed: {err}"))?;
        let results = pool.install(|| {
            existing
                .par_iter()
                .map(|candidate| {
                    let integrity = match fast_img_verified_output_format(&candidate.output, strategy)? {
                        FormatKind::Avif => verify_final_avif_delivery_integrity(
                            &candidate.source,
                            &candidate.output,
                        )
                        .map_err(|err| {
                            anyhow::anyhow!(
                                "fast-img source delete gate final AVIF proof failed for {} -> {}: {err}",
                                candidate.source.display(),
                                candidate.output.display()
                            )
                        })?,
                        FormatKind::Jxl => verify_final_jxl_delivery_integrity(
                            &candidate.source,
                            &candidate.output,
                        )
                        .map_err(|err| {
                            anyhow::anyhow!(
                                "fast-img source delete gate final JXL proof failed for {} -> {}: {err}",
                                candidate.source.display(),
                                candidate.output.display()
                            )
                        })?,
                        format => anyhow::bail!(
                            "unsupported fast-img verified output format {format:?}"
                        ),
                    };
                    Ok(FastImgVerifiedSourceDelete {
                        source: candidate.source.clone(),
                        output: candidate.output.clone(),
                        integrity,
                    })
                })
                .collect::<Vec<anyhow::Result<_>>>()
        });
        for result in results {
            verified.push(result?);
        }
    }

    let mut deleted = 0usize;
    let mut already_deleted = 0usize;
    for candidate in candidates {
        if !candidate.source.exists() {
            safe_delete_matching_xmp_sidecar(&candidate.source, &candidate.output).map_err(
                |err| {
                    anyhow::anyhow!(
                        "fast-img failed to delete matching XMP sidecar for already-absent source JPEG {} using output {}: {err}",
                        candidate.source.display(),
                        candidate.output.display()
                    )
                },
            )?;
            already_deleted += 1;
            tracing::info!(
                target: "fast_img_delete",
                source = %candidate.source.display(),
                "verified source JPEG already absent"
            );
        }
    }
    for candidate in verified {
        safe_delete_jpeg_source(&candidate.source, &candidate.output, &candidate.integrity)
            .map_err(|err| {
                anyhow::anyhow!(
                    "fast-img failed to delete verified source JPEG {} using output {}: {err}",
                    candidate.source.display(),
                    candidate.output.display()
                )
            })?;
        deleted += 1;
    }
    Ok((deleted, already_deleted))
}

struct FastImgSourceDeleteCandidate {
    source: PathBuf,
    output: PathBuf,
}

struct FastImgVerifiedSourceDelete {
    source: PathBuf,
    output: PathBuf,
    integrity: IntegrityResult,
}

fn fast_img_effective_verify_parallelism(
    pending_count: usize,
    configured_parallel_tasks: usize,
) -> usize {
    if pending_count == 0 {
        1
    } else {
        configured_parallel_tasks.clamp(1, pending_count).min(4)
    }
}

#[cfg(test)]
fn fast_img_delete_verified_source_jpegs_with(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    mut verify_integrity: impl FnMut(&Path, &Path) -> anyhow::Result<IntegrityResult>,
) -> anyhow::Result<(usize, usize)> {
    fast_img_preflight_verified_source_deletion(marker, src_dir)?;
    let mut deleted = 0usize;
    let mut already_deleted = 0usize;
    for (rel, entry) in &marker.blake3_log {
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        let output = fast_img_marker_entry_output_path(marker, rel, entry)?;
        if !source.exists() {
            safe_delete_matching_xmp_sidecar(&source, &output).map_err(|err| {
                anyhow::anyhow!(
                    "fast-img failed to delete matching XMP sidecar for already-absent source JPEG {} using output {}: {err}",
                    source.display(),
                    output.display()
                )
            })?;
            already_deleted += 1;
            tracing::info!(
                target: "fast_img_delete",
                source = %source.display(),
                "verified source JPEG already absent"
            );
            continue;
        }
        let integrity = verify_integrity(&source, &output)?;
        safe_delete_jpeg_source(&source, &output, &integrity).map_err(|err| {
            anyhow::anyhow!(
                "fast-img failed to delete verified source JPEG {} using output {}: {err}",
                source.display(),
                output.display()
            )
        })?;
        deleted += 1;
    }
    Ok((deleted, already_deleted))
}

fn fast_img_preflight_verified_source_deletion(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
) -> anyhow::Result<()> {
    if !gate1_complete_or_later(&marker.stage) {
        anyhow::bail!(
            "fast-img source delete gate requires Gate 1 passed; current stage={}",
            marker.stage.as_str()
        );
    }
    fast_img_validate_source_delete_disposition(marker)?;
    for (rel, entry) in &marker.skipped_sources {
        if entry.src.is_empty() || entry.reason.trim().is_empty() {
            anyhow::bail!(
                "fast-img source delete gate has incomplete skipped-source proof for {rel}"
            );
        }
    }
    for (rel, entry) in &marker.failed_sources {
        if entry.src.is_empty() || entry.reason.trim().is_empty() {
            anyhow::bail!(
                "fast-img source delete gate has incomplete failed-source proof for {rel}"
            );
        }
    }
    for (rel, entry) in &marker.blake3_log {
        if entry.src.is_empty() || entry.out.is_empty() {
            anyhow::bail!("fast-img source delete gate has incomplete hash proof for {rel}");
        }
        let output = fast_img_marker_entry_output_path(marker, rel, entry)?;
        let output_metadata = std::fs::metadata(&output)
            .with_context(|| format!("stat verified JXL output {}", output.display()))?;
        if output_metadata.len() == 0 {
            anyhow::bail!(
                "fast-img source delete gate found empty output {}",
                output.display()
            );
        }
        let output_hash = calculate_blake3_hash(&output)?;
        if output_hash != entry.out {
            anyhow::bail!(
                "fast-img source delete gate output hash drifted for {}",
                output.display()
            );
        }

        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        if source.exists() {
            let source_hash = calculate_blake3_hash(&source)?;
            if source_hash != entry.src {
                anyhow::bail!(
                    "fast-img source delete gate source hash drifted for {}",
                    source.display()
                );
            }
        }
    }
    Ok(())
}

fn fast_img_prune_empty_source_dirs(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
) -> anyhow::Result<usize> {
    if !src_dir.is_dir() {
        return Ok(0);
    }
    let mut dirs = Vec::new();
    for rel in marker.blake3_log.keys() {
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        let mut current = source.parent();
        while let Some(dir) = current {
            if dir == src_dir {
                break;
            }
            dirs.push(dir.to_path_buf());
            current = dir.parent();
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut pruned = 0usize;
    for dir in dirs {
        let mut entries = std::fs::read_dir(&dir)
            .with_context(|| format!("read fast-img source dir {}", dir.display()))?;
        if entries.next().transpose()?.is_some() {
            continue;
        }
        std::fs::remove_dir(&dir)
            .with_context(|| format!("delete empty fast-img source directory {}", dir.display()))?;
        pruned += 1;
        tracing::info!(
            target: "fast_img_delete",
            path = %dir.display(),
            "delete-gate PASS: removing empty source directory"
        );
    }
    Ok(pruned)
}

fn fast_img_marker_entry_output_path(
    marker: &WorkingCopyMarker,
    rel: &str,
    entry: &Blake3Entry,
) -> anyhow::Result<PathBuf> {
    let out_rel = if let Some(out_rel) = entry.out_rel.as_deref() {
        fast_img_checked_rel_path(out_rel)?
    } else {
        let target_ext = if marker.strategy == "avif" {
            "AVIF"
        } else {
            "JXL"
        };
        fast_img_checked_rel_path(rel)?.with_extension(target_ext)
    };
    Ok(marker.working_copy.join(out_rel))
}

fn fast_img_checked_rel_path(rel: &str) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(rel);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("fast-img marker contains unsafe relative path: {rel}");
    }
    Ok(path)
}

fn read_existing_fast_img_marker(working_copy: &Path) -> anyhow::Result<Option<WorkingCopyMarker>> {
    match read_marker(working_copy) {
        Ok(marker) => Ok(Some(marker)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(anyhow::anyhow!(
            "fast-img marker at {} is unreadable/corrupt; refusing to overwrite resume state: {err}",
            marker_path_for_working_copy(working_copy).display()
        )),
    }
}

fn fast_img_archive_stale_working_copy(working_copy: &Path) -> anyhow::Result<Option<PathBuf>> {
    let metadata = match std::fs::symlink_metadata(working_copy) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "inspect stale fast-img working copy {}",
                    working_copy.display()
                )
            });
        }
    };
    let file_type = metadata.file_type();
    if !(file_type.is_dir() || file_type.is_file() || file_type.is_symlink()) {
        anyhow::bail!(
            "fast-img stale working copy has unsupported filesystem type: {}",
            working_copy.display()
        );
    }
    let parent = working_copy
        .parent()
        .context("fast-img working copy has no parent directory")?;
    let base_name = working_copy
        .file_name()
        .context("fast-img working copy has no final path component")?
        .to_string_lossy();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();

    for attempt in 0..1000 {
        let suffix = if attempt == 0 {
            format!("{base_name}.stale-{timestamp}")
        } else {
            format!("{base_name}.stale-{timestamp}-{attempt}")
        };
        let archived = parent.join(suffix);
        match std::fs::symlink_metadata(&archived) {
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "inspect stale fast-img archive destination {}",
                        archived.display()
                    )
                });
            }
        }
        match std::fs::rename(working_copy, &archived) {
            Ok(()) => return Ok(Some(archived)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "archive stale fast-img working copy {} at {}",
                        working_copy.display(),
                        archived.display()
                    )
                });
            }
        }
    }

    anyhow::bail!(
        "could not choose an archive path for stale fast-img working copy {}",
        working_copy.display()
    )
}

fn fast_img_recover_non_directory_working_copy(
    working_copy: &Path,
    dry_run: DryRunFlag,
) -> anyhow::Result<()> {
    let metadata = match std::fs::symlink_metadata(working_copy) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "inspect fast-img working copy before opening marker {}",
                    working_copy.display()
                )
            });
        }
    };
    if metadata.file_type().is_dir() {
        return Ok(());
    }
    if dry_run.0 {
        println!(
            "[DRY-RUN ] stale fast-img output is not a directory; would archive {}",
            working_copy.display()
        );
        return Ok(());
    }
    if let Some(archived) = fast_img_archive_stale_working_copy(working_copy)? {
        println!(
            "[RECOVER ] stale fast-img output was not a directory; archived it at {}",
            archived.display()
        );
    }
    Ok(())
}

/// Prefer the conventional output directory after recovering a stale plain file.
///
/// `resolve_working_copy_dir` intentionally skips any occupied path without a
/// working-copy marker. For `FastImg`, though, that path can be the interrupted
/// plain-file placeholder that we know how to archive safely. Recover it first
/// so a normal run recreates the expected `<source>_optimized` directory.
fn fast_img_resolve_working_copy_for_run(
    src_dir: &Path,
    dry_run: DryRunFlag,
) -> anyhow::Result<PathBuf> {
    let preferred_working_copy = working_copy_dir(src_dir);
    match std::fs::symlink_metadata(&preferred_working_copy) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            fast_img_recover_non_directory_working_copy(&preferred_working_copy, dry_run)?;
            if !dry_run.0 {
                return Ok(preferred_working_copy);
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "inspect preferred fast-img working copy {}",
                    preferred_working_copy.display()
                )
            });
        }
    }

    let working_copy = resolve_working_copy_dir(src_dir);
    fast_img_recover_non_directory_working_copy(&working_copy, dry_run)?;
    Ok(working_copy)
}

fn fast_img_strip_non_target_files(working_copy: &Path, strategy: &str) -> anyhow::Result<()> {
    let mut pending_dirs = vec![working_copy.to_path_buf()];
    let mut files_to_delete = Vec::new();
    let target_format = foundation::delivery_codec_strategy::strategy_to_format_kind(strategy)
        .ok_or_else(|| anyhow::anyhow!("unsupported fast-img strategy {strategy:?}"))?;
    let target_ext = target_format
        .canonical_extension()
        .ok_or_else(|| anyhow::anyhow!("fast-img target format has no canonical extension"))?;
    while let Some(dir) = pending_dirs.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("read fast-img working-copy dir {}", dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("read fast-img working-copy entry in {}", dir.display())
            })?;
            let path = entry.path();
            let file_type = entry.file_type().with_context(|| {
                format!("read fast-img working-copy file type {}", path.display())
            })?;
            if file_type.is_dir() {
                pending_dirs.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let is_marker = path.file_name().is_some_and(|name| name == ".mfb_wc");
            if is_marker {
                continue;
            }
            let is_target =
                foundation::image::format_detect::detect_true_format(&path).with_context(|| {
                    format!(
                        "detect true format before fast-img working-copy cleanup {}",
                        path.display()
                    )
                })? == target_format;
            if is_target {
                continue;
            }
            files_to_delete.push(path);
        }
    }
    files_to_delete.sort_by(|left, right| {
        let left_is_marker = left.file_name().is_some_and(|name| name == ".mfb_wc");
        let right_is_marker = right.file_name().is_some_and(|name| name == ".mfb_wc");
        left_is_marker
            .cmp(&right_is_marker)
            .then_with(|| left.cmp(right))
    });
    for path in files_to_delete {
        std::fs::remove_file(&path).with_context(|| {
            format!(
                "delete non-{} fast-img working-copy file {}",
                target_ext.to_uppercase(),
                path.display()
            )
        })?;
        tracing::info!(
            target: "fast_img",
            path = %path.display(),
            "deleted non-{} working-copy file after Gate 1",
            target_ext.to_uppercase()
        );
    }
    Ok(())
}

struct FastImgInputPlan {
    src_root: PathBuf,
    candidates: Vec<PathBuf>,
}

impl FastImgInputPlan {
    fn from_input(input: &Path, recursive: bool) -> anyhow::Result<Self> {
        let canonical = foundation::media_conversion_gate::canonicalize_for_tool_input(input);
        if canonical.is_file() {
            let parent = canonical.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "fast-img single-file input has no parent: {}",
                    canonical.display()
                )
            })?;
            return Ok(Self {
                src_root: parent.to_path_buf(),
                candidates: vec![canonical],
            });
        }
        let candidates = fast_img_scan_regular_files(&canonical, recursive)?;
        Ok(Self {
            src_root: canonical,
            candidates,
        })
    }
}

fn fast_img_scan_regular_files(root: &Path, recursive: bool) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending_dirs = vec![root.to_path_buf()];
    while let Some(dir) = pending_dirs.pop() {
        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("fast-img scan read dir {}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.with_context(|| format!("fast-img scan entry under {}", dir.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("fast-img scan file type {}", path.display()))?;
            if file_type.is_dir() {
                if recursive {
                    pending_dirs.push(path);
                }
                continue;
            }
            if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn restore_jpeg_default_output_dir(input: &Path) -> anyhow::Result<PathBuf> {
    let naming_path = if input.is_file() {
        input.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "restore-jpeg single-file input has no parent: {}",
                input.display()
            )
        })?
    } else {
        input
    };
    Ok(naming_path.with_file_name(format!(
        "{}_restored_jpeg",
        naming_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!(
                "restore-jpeg input has no valid directory name: {}",
                naming_path.display()
            ))?
    )))
}

fn restore_jpeg_output_path_for(
    input: &Path,
    input_root: &Path,
    output_root: &Path,
) -> anyhow::Result<PathBuf> {
    let relative = input.strip_prefix(input_root).with_context(|| {
        format!(
            "restore-jpeg input {} is outside root {}",
            input.display(),
            input_root.display()
        )
    })?;
    let mut output = output_root.join(relative);
    output.set_extension("jpg");
    Ok(output)
}

fn restore_jpeg_candidate_files(input: &Path, recursive: bool) -> anyhow::Result<Vec<PathBuf>> {
    if input.is_file() {
        let format = foundation::image::format_detect::detect_true_format(input)
            .with_context(|| format!("restore-jpeg failed to probe {}", input.display()))?;
        if format != FormatKind::Jxl {
            anyhow::bail!(
                "restore-jpeg input is not a true JXL file: {} (detected {:?})",
                input.display(),
                format
            );
        }
        return Ok(vec![input.to_path_buf()]);
    }

    let mut jxl_files = Vec::new();
    for path in fast_img_scan_regular_files(input, recursive)? {
        let format = foundation::image::format_detect::detect_true_format(&path)
            .with_context(|| format!("restore-jpeg failed to probe {}", path.display()))?;
        if format == FormatKind::Jxl {
            jxl_files.push(path);
        }
    }
    Ok(jxl_files)
}

fn restore_jpeg_input_root(input: &Path) -> anyhow::Result<PathBuf> {
    if input.is_file() {
        let parent = input.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "restore-jpeg single-file input has no parent: {}",
                input.display()
            )
        })?;
        return Ok(parent.to_path_buf());
    }
    Ok(input.to_path_buf())
}

const RESTORE_JPEG_MANIFEST_NAME: &str = ".mfb_restore_jpeg_manifest.tsv";

#[derive(Debug, Clone)]
struct RestoreJpegCommitProof {
    source: PathBuf,
    output: PathBuf,
    source_rel: String,
    output_rel: String,
    source_hash: String,
    output_hash: String,
}

#[derive(Debug, Clone)]
struct RestoreJpegResult {
    committed: bool,
    proof: RestoreJpegCommitProof,
}

#[derive(Debug, Clone)]
struct RestoreJpegManifestRecord {
    proof: RestoreJpegCommitProof,
    source_deleted: bool,
}

fn restore_jpeg_relative_string(path: &Path, root: &Path) -> anyhow::Result<String> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "restore-jpeg path {} is outside root {}",
            path.display(),
            root.display()
        )
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn restore_jpeg_hex_encode(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(text.len() * 2);
    for &byte in text.as_bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn write_restore_jpeg_manifest(
    output_root: &Path,
    records: &[RestoreJpegManifestRecord],
) -> anyhow::Result<()> {
    std::fs::create_dir_all(output_root).with_context(|| {
        format!(
            "restore-jpeg failed to create manifest directory {}",
            output_root.display()
        )
    })?;
    let manifest = output_root.join(RESTORE_JPEG_MANIFEST_NAME);
    let temp_manifest = manifest.with_extension("tsv.tmp");
    let mut content = String::from(
        "# MFB_RESTORE_JPEG_MANIFEST_V1\nsource_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted\n",
    );
    for record in records {
        content.push_str(&restore_jpeg_hex_encode(&record.proof.source_rel));
        content.push('\t');
        content.push_str(&restore_jpeg_hex_encode(&record.proof.output_rel));
        content.push('\t');
        content.push_str(&record.proof.source_hash);
        content.push('\t');
        content.push_str(&record.proof.output_hash);
        content.push('\t');
        content.push_str(if record.source_deleted {
            "true\n"
        } else {
            "false\n"
        });
    }
    std::fs::write(&temp_manifest, content).with_context(|| {
        format!(
            "restore-jpeg failed to write manifest temp {}",
            temp_manifest.display()
        )
    })?;
    std::fs::rename(&temp_manifest, &manifest).with_context(|| {
        format!(
            "restore-jpeg failed to commit manifest {}",
            manifest.display()
        )
    })?;
    Ok(())
}

fn record_and_delete_restored_jpeg_source(
    output_root: &Path,
    restore_records: &mut Vec<RestoreJpegManifestRecord>,
    proof: &RestoreJpegCommitProof,
) -> anyhow::Result<bool> {
    restore_records.push(RestoreJpegManifestRecord {
        proof: proof.clone(),
        source_deleted: false,
    });
    write_restore_jpeg_manifest(output_root, restore_records).with_context(|| {
        format!(
            "restore-jpeg failed to persist recovery manifest before removing {}",
            proof.source.display()
        )
    })?;
    let deleted = restore_jpeg_delete_verified_source(proof)?;
    if deleted {
        let record = restore_records
            .last_mut()
            .context("restore-jpeg recovery manifest record disappeared")?;
        record.source_deleted = true;
        write_restore_jpeg_manifest(output_root, restore_records).with_context(|| {
            format!(
                "restore-jpeg failed to persist completed deletion for {}",
                proof.source.display()
            )
        })?;
    }
    Ok(deleted)
}

fn record_retained_restored_jpeg_source(
    restore_records: &mut Vec<RestoreJpegManifestRecord>,
    proof: &RestoreJpegCommitProof,
) {
    restore_records.push(RestoreJpegManifestRecord {
        proof: proof.clone(),
        source_deleted: false,
    });
}

fn restore_jpeg_remove_temp(temp: &Path, context: &str) -> anyhow::Result<()> {
    foundation::io_utils::safe_remove_file(temp).with_context(|| {
        format!(
            "restore-jpeg failed to remove temp file {} after {context}",
            temp.display()
        )
    })
}

fn run_restore_image_command(
    mut command: std::process::Command,
    context: &str,
) -> anyhow::Result<std::process::Output> {
    foundation::process_runner::run_command_with_liveness_timeout(
        &mut command,
        std::time::Duration::from_secs(120),
        foundation::process_runner::image_process_hard_timeout(),
        context,
    )
    .with_context(|| format!("{context} failed to run"))
}

fn restore_jpeg_decode_to_temp(input: &Path, temp_output: &Path) -> anyhow::Result<()> {
    let command = foundation::DjxlBuilder::new()
        .input(input)
        .output(temp_output)
        .build();
    let decode = run_restore_image_command(command, "restore-jpeg djxl decode")
        .with_context(|| format!("restore-jpeg failed to launch djxl for {}", input.display()))?;
    if !decode.status.success() {
        let stderr = String::from_utf8_lossy(&decode.stderr);
        if let Err(cleanup_err) = restore_jpeg_remove_temp(temp_output, "djxl failure") {
            anyhow::bail!(
                "restore-jpeg djxl failed for {}: {}; additionally cleanup failed: {cleanup_err}",
                input.display(),
                stderr.trim()
            );
        }
        anyhow::bail!(
            "restore-jpeg djxl failed for {}: {}",
            input.display(),
            stderr.trim()
        );
    }
    Ok(())
}

fn restore_jpeg_decoded_pixels_match(left: &Path, right: &Path) -> anyhow::Result<bool> {
    let left_img = load_image_safe(left).with_context(|| {
        format!(
            "restore-jpeg proof gate failed to decode {}",
            left.display()
        )
    })?;
    let right_img = load_image_safe(right).with_context(|| {
        format!(
            "restore-jpeg proof gate failed to decode restored output {}",
            right.display()
        )
    })?;
    if left_img.width() != right_img.width() || left_img.height() != right_img.height() {
        return Ok(false);
    }
    Ok(left_img.to_rgba8().as_raw() == right_img.to_rgba8().as_raw())
}

fn restore_jpeg_build_current_proof_with_decoder<F>(
    input: &Path,
    input_root: &Path,
    output: &Path,
    output_root: &Path,
    decode_to_temp: F,
) -> anyhow::Result<RestoreJpegCommitProof>
where
    F: FnOnce(&Path, &Path) -> anyhow::Result<()>,
{
    let source_format =
        foundation::image::format_detect::detect_true_format(input).with_context(|| {
            format!(
                "restore-jpeg proof gate failed to probe {}",
                input.display()
            )
        })?;
    if source_format != FormatKind::Jxl {
        anyhow::bail!(
            "restore-jpeg proof gate refused non-JXL source {} (detected {:?})",
            input.display(),
            source_format
        );
    }

    let output_meta = std::fs::metadata(output).with_context(|| {
        format!(
            "restore-jpeg proof gate: restored output missing for {} -> {}",
            input.display(),
            output.display()
        )
    })?;
    if !output_meta.is_file() || output_meta.len() == 0 {
        anyhow::bail!(
            "restore-jpeg proof gate: restored output is not a non-empty file for {} -> {}",
            input.display(),
            output.display()
        );
    }

    let output_format =
        foundation::image::format_detect::detect_true_format(output).with_context(|| {
            format!(
                "restore-jpeg proof gate failed to probe restored output {}",
                output.display()
            )
        })?;
    if output_format != FormatKind::Jpeg {
        anyhow::bail!(
            "restore-jpeg proof gate: restored output is not a true JPEG: {} (detected {:?})",
            output.display(),
            output_format
        );
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(output)
        .map_err(|err| anyhow::anyhow!("restore-jpeg proof temp path failed: {err}"))?;
    if let Err(err) = decode_to_temp(input, &temp_output).with_context(|| {
        format!(
            "restore-jpeg proof gate failed to fresh-decode {}",
            input.display()
        )
    }) {
        if let Err(cleanup_err) = restore_jpeg_remove_temp(&temp_output, "failed fresh decode") {
            return Err(err.context(format!(
                "restore-jpeg proof temp cleanup also failed: {cleanup_err}"
            )));
        }
        return Err(err);
    }

    let proof_result = (|| {
        let decoded_format = foundation::image::format_detect::detect_true_format(&temp_output)
            .with_context(|| {
                format!(
                    "restore-jpeg proof gate failed to probe fresh djxl output {}",
                    temp_output.display()
                )
            })?;
        if decoded_format != FormatKind::Jpeg {
            anyhow::bail!(
                "restore-jpeg proof gate: fresh djxl output is not a true JPEG for {} (detected {:?})",
                input.display(),
                decoded_format
            );
        }

        let output_hash = calculate_blake3_hash(output).with_context(|| {
            format!(
                "restore-jpeg proof gate failed to hash restored output {}",
                output.display()
            )
        })?;
        if !restore_jpeg_decoded_pixels_match(&temp_output, output)? {
            anyhow::bail!(
                "restore-jpeg proof gate: restored JPEG pixels do not match fresh djxl decode for {} -> {}",
                input.display(),
                output.display()
            );
        }

        Ok(RestoreJpegCommitProof {
            source: input.to_path_buf(),
            output: output.to_path_buf(),
            source_rel: restore_jpeg_relative_string(input, input_root)?,
            output_rel: restore_jpeg_relative_string(output, output_root)?,
            source_hash: calculate_blake3_hash(input).with_context(|| {
                format!(
                    "restore-jpeg proof gate failed to hash source JXL {}",
                    input.display()
                )
            })?,
            output_hash,
        })
    })();
    let cleanup_result = restore_jpeg_remove_temp(&temp_output, "fresh decode proof");
    match (proof_result, cleanup_result) {
        (Ok(proof), Ok(())) => Ok(proof),
        (Ok(_), Err(cleanup_err)) => Err(cleanup_err),
        (Err(err), Ok(())) => Err(err),
        (Err(err), Err(cleanup_err)) => Err(err.context(format!(
            "restore-jpeg proof temp cleanup also failed: {cleanup_err}"
        ))),
    }
}

fn restore_jpeg_build_current_proof(
    input: &Path,
    input_root: &Path,
    output: &Path,
    output_root: &Path,
) -> anyhow::Result<RestoreJpegCommitProof> {
    restore_jpeg_build_current_proof_with_decoder(
        input,
        input_root,
        output,
        output_root,
        restore_jpeg_decode_to_temp,
    )
}

fn restore_single_jpeg(
    input: &Path,
    input_root: &Path,
    output_root: &Path,
    force: bool,
) -> anyhow::Result<RestoreJpegResult> {
    let output = restore_jpeg_output_path_for(input, input_root, output_root)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("restore-jpeg failed to create {}", parent.display()))?;
    }

    if !force && output.exists() {
        let proof = restore_jpeg_build_current_proof(input, input_root, &output, output_root)
            .with_context(|| {
                format!(
                    "restore-jpeg failed to re-verify existing output {} -> {}",
                    input.display(),
                    output.display()
                )
            })?;
        return Ok(RestoreJpegResult {
            committed: false,
            proof,
        });
    }

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|err| anyhow::anyhow!("restore-jpeg temp path failed: {err}"))?;
    let command = foundation::DjxlBuilder::new()
        .input(input)
        .output(&temp_output)
        .build();
    let decode = run_restore_image_command(command, "restore-jpeg djxl decode")
        .with_context(|| format!("restore-jpeg failed to launch djxl for {}", input.display()))?;
    if !decode.status.success() {
        let stderr = String::from_utf8_lossy(&decode.stderr);
        if let Err(cleanup_err) = restore_jpeg_remove_temp(&temp_output, "djxl failure") {
            anyhow::bail!(
                "restore-jpeg djxl failed for {}: {}; additionally cleanup failed: {cleanup_err}",
                input.display(),
                stderr.trim()
            );
        }
        anyhow::bail!(
            "restore-jpeg djxl failed for {}: {}",
            input.display(),
            stderr.trim()
        );
    }

    // Keep the exact official djxl reconstruction for the post-metadata pixel
    // proof. This avoids launching djxl a second time for every committed file.
    let proof_snapshot = temp_output.with_extension("mfb-restore-proof.jpg");
    std::fs::copy(&temp_output, &proof_snapshot).with_context(|| {
        format!(
            "restore-jpeg failed to snapshot official djxl output {}",
            temp_output.display()
        )
    })?;

    let commit_result =
        foundation::conversion::commit_temp_to_output_with_metadata_pixel_already_verified(
            &temp_output,
            &output,
            force,
            Some(input),
        )
        .with_context(|| {
            format!(
                "restore-jpeg failed to commit metadata-preserving output {}",
                output.display()
            )
        });
    let committed = match commit_result {
        Ok(committed) => committed,
        Err(err) => {
            let _ = restore_jpeg_remove_temp(&proof_snapshot, "failed metadata commit");
            return Err(err);
        }
    };

    let proof_result = restore_jpeg_build_current_proof_with_decoder(
        input,
        input_root,
        &output,
        output_root,
        |_input, fresh_decode| {
            std::fs::copy(&proof_snapshot, fresh_decode)
                .map(|_| ())
                .with_context(|| {
                    format!(
                        "restore-jpeg failed to stage djxl proof snapshot {}",
                        proof_snapshot.display()
                    )
                })
        },
    )
    .with_context(|| {
        format!(
            "restore-jpeg failed to build deletion proof for {} -> {}",
            input.display(),
            output.display()
        )
    });
    let cleanup_result = restore_jpeg_remove_temp(&proof_snapshot, "completed restore proof");
    let proof = match (proof_result, cleanup_result) {
        (Ok(proof), Ok(())) => proof,
        (Ok(_), Err(cleanup_err)) => return Err(cleanup_err),
        (Err(err), Ok(())) => return Err(err),
        (Err(err), Err(cleanup_err)) => {
            return Err(err.context(format!(
                "restore-jpeg proof snapshot cleanup also failed: {cleanup_err}"
            )));
        }
    };

    Ok(RestoreJpegResult { committed, proof })
}

fn restore_jpeg_delete_verified_source(proof: &RestoreJpegCommitProof) -> anyhow::Result<bool> {
    let input = &proof.source;
    let output = &proof.output;
    if !input.exists() {
        tracing::info!(
            target: "restore_jpeg_delete",
            source = %input.display(),
            output = %output.display(),
            "restore-jpeg delete skipped: source already absent"
        );
        return Ok(false);
    }

    let source_format =
        foundation::image::format_detect::detect_true_format(input).with_context(|| {
            format!(
                "restore-jpeg delete gate failed to probe {}",
                input.display()
            )
        })?;
    if source_format != FormatKind::Jxl {
        anyhow::bail!(
            "restore-jpeg delete gate refused non-JXL source {} (detected {:?})",
            input.display(),
            source_format
        );
    }

    let output_meta = std::fs::metadata(output).with_context(|| {
        format!(
            "restore-jpeg delete gate: restored output missing for {} -> {}",
            input.display(),
            output.display()
        )
    })?;
    if !output_meta.is_file() || output_meta.len() == 0 {
        anyhow::bail!(
            "restore-jpeg delete gate: restored output is not a non-empty file for {} -> {}",
            input.display(),
            output.display()
        );
    }

    let output_format =
        foundation::image::format_detect::detect_true_format(output).with_context(|| {
            format!(
                "restore-jpeg delete gate failed to probe restored output {}",
                output.display()
            )
        })?;
    if output_format != FormatKind::Jpeg {
        anyhow::bail!(
            "restore-jpeg delete gate: restored output is not a true JPEG: {} (detected {:?})",
            output.display(),
            output_format
        );
    }

    let source_hash = calculate_blake3_hash(input).with_context(|| {
        format!(
            "restore-jpeg delete gate failed to hash source {}",
            input.display()
        )
    })?;
    let output_hash = calculate_blake3_hash(output).with_context(|| {
        format!(
            "restore-jpeg delete gate failed to hash restored output {}",
            output.display()
        )
    })?;
    if source_hash != proof.source_hash || output_hash != proof.output_hash {
        anyhow::bail!(
            "restore-jpeg delete gate: stale restore proof for {} -> {}",
            input.display(),
            output.display()
        );
    }
    tracing::info!(
        target: "restore_jpeg_delete",
        source = %input.display(),
        source_blake3 = %source_hash,
        output = %output.display(),
        output_blake3 = %output_hash,
        "restore-jpeg delete-gate PASS: removing verified source JXL"
    );

    foundation::io_utils::safe_remove_file(input).with_context(|| {
        format!(
            "restore-jpeg delete gate failed to delete source JXL {} after verified restore {}",
            input.display(),
            output.display()
        )
    })?;
    safe_delete_matching_xmp_sidecar(input, output).map_err(|err| {
        anyhow::anyhow!(
            "restore-jpeg delete gate failed to delete matching XMP sidecar for {} using restored output {}: {err}",
            input.display(),
            output.display()
        )
    })?;
    Ok(true)
}

fn restore_jpeg_dir_entry_partition(dir: &Path) -> std::io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut substantive_entries = Vec::new();
    let mut ds_store_files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_name() == ".DS_Store" && entry.file_type()?.is_file() {
            ds_store_files.push(path);
        } else {
            substantive_entries.push(path);
        }
    }
    Ok((substantive_entries, ds_store_files))
}

fn restore_jpeg_remove_empty_dir_once(dir: &Path) -> bool {
    const DS_STORE_REMOVAL_ATTEMPTS: usize = 2;
    if !dir.exists() {
        return false;
    }
    for _ in 0..DS_STORE_REMOVAL_ATTEMPTS {
        let (substantive_entries, ds_store_files) = match restore_jpeg_dir_entry_partition(dir) {
            Ok(entries) => entries,
            Err(err) => {
                println!(
                    "[WARN] Could not remove {}: failed to inspect directory: {err}",
                    dir.display()
                );
                return false;
            }
        };
        if !substantive_entries.is_empty() {
            return false;
        }
        if ds_store_files.is_empty() {
            match std::fs::remove_dir(dir) {
                Ok(()) => {
                    println!("[CLEANUP] Removed empty dir: {}", dir.display());
                    return true;
                }
                Err(err) => {
                    println!("[WARN] Could not remove {}: {err}", dir.display());
                    return false;
                }
            }
        }
        for ds_store in ds_store_files {
            foundation::media_conversion_gate::delivery_remove_file_or_audit(
                "restore-jpeg cleanup .DS_Store removal",
                &ds_store,
            );
        }
    }

    let (substantive_entries, ds_store_files) = match restore_jpeg_dir_entry_partition(dir) {
        Ok(entries) => entries,
        Err(err) => {
            println!(
                "[WARN] Could not remove {}: failed to inspect directory: {err}",
                dir.display()
            );
            return false;
        }
    };
    if !substantive_entries.is_empty() || !ds_store_files.is_empty() {
        return false;
    }
    match std::fs::remove_dir(dir) {
        Ok(()) => {
            println!("[CLEANUP] Removed empty dir: {}", dir.display());
            true
        }
        Err(err) => {
            println!("[WARN] Could not remove {}: {err}", dir.display());
            false
        }
    }
}

fn restore_jpeg_prune_empty_source_dirs(input_root: &Path, candidate_dirs: &[PathBuf]) -> usize {
    let mut candidates = BTreeSet::new();
    for dir in candidate_dirs {
        if dir != input_root && dir.starts_with(input_root) {
            candidates.insert(dir.clone());
        }
    }
    let mut candidates: Vec<PathBuf> = candidates.into_iter().collect();
    candidates.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));

    let mut removed = 0usize;
    for start_dir in candidates {
        let mut current = start_dir;
        loop {
            if current == input_root || !current.starts_with(input_root) {
                break;
            }
            let Some(parent) = current.parent().map(Path::to_path_buf) else {
                break;
            };
            if !restore_jpeg_remove_empty_dir_once(&current) {
                break;
            }
            removed += 1;
            current = parent;
        }
    }
    removed
}

fn restore_jpeg_canonical_output_root(output_root: &Path) -> anyhow::Result<PathBuf> {
    if output_root.exists() {
        return std::fs::canonicalize(output_root).with_context(|| {
            format!(
                "restore-jpeg failed to canonicalize output root {}",
                output_root.display()
            )
        });
    }

    let mut missing = Vec::new();
    let mut existing = output_root;
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            anyhow::anyhow!(
                "restore-jpeg output root has no existing ancestor: {}",
                output_root.display()
            )
        })?;
        missing.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "restore-jpeg output root has no existing ancestor: {}",
                output_root.display()
            )
        })?;
    }
    let mut canonical = std::fs::canonicalize(existing).with_context(|| {
        format!(
            "restore-jpeg failed to canonicalize output ancestor {}",
            existing.display()
        )
    })?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

fn restore_jpeg_validate_disjoint_roots(
    input_root: &Path,
    output_root: &Path,
) -> anyhow::Result<()> {
    let canonical_input = std::fs::canonicalize(input_root).with_context(|| {
        format!(
            "restore-jpeg failed to canonicalize input root {}",
            input_root.display()
        )
    })?;
    let canonical_output = restore_jpeg_canonical_output_root(output_root)?;
    if canonical_input == canonical_output || canonical_output.starts_with(&canonical_input) {
        anyhow::bail!(
            "restore-jpeg requires disjoint input and output roots: input={} output={}",
            canonical_input.display(),
            canonical_output.display()
        );
    }
    Ok(())
}

fn restore_jpeg_jxlinfo_has_jpeg_reconstruction(output: &[u8]) -> bool {
    String::from_utf8_lossy(output)
        .to_ascii_lowercase()
        .contains("jpeg bitstream reconstruction data available")
}

fn restore_jpeg_preflight(
    input_root: &Path,
    output_root: &Path,
    files: &[PathBuf],
) -> anyhow::Result<()> {
    restore_jpeg_validate_disjoint_roots(input_root, output_root)?;
    if files.is_empty() {
        anyhow::bail!(
            "restore-jpeg found no true JXL files in {}",
            input_root.display()
        );
    }

    let mut output_owners = BTreeMap::new();
    for source in files {
        let output = restore_jpeg_output_path_for(source, input_root, output_root)?;
        if let Some(previous) = output_owners.insert(output.clone(), source.clone()) {
            anyhow::bail!(
                "restore-jpeg output collision: {} and {} both map to {}",
                previous.display(),
                source.display(),
                output.display()
            );
        }
    }

    let jxlinfo = foundation::common_utils::resolve_tool_path("jxlinfo")
        .context("restore-jpeg requires official jxlinfo")?;
    let available_workers = match std::thread::available_parallelism() {
        Ok(worker_count) => worker_count.get(),
        Err(error) => {
            eprintln!(
                "[PREFLIGHT] available parallelism could not be detected ({error}); using one worker"
            );
            1
        }
    };
    let worker_count = available_workers.clamp(1, 8);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .thread_name(|index| format!("mfb-jxlinfo-{index}"))
        .build()
        .context("restore-jpeg failed to create jxlinfo preflight worker pool")?;
    let verified = std::sync::atomic::AtomicUsize::new(0);
    pool.install(|| {
        files.par_iter().try_for_each(|source| {
            let mut command = std::process::Command::new(&jxlinfo);
            command.arg("-v").arg(source);
            let probe = run_restore_image_command(command, "restore-jpeg jxlinfo preflight")
                .with_context(|| {
                    format!(
                        "restore-jpeg preflight failed to launch jxlinfo for {}",
                        source.display()
                    )
                })?;
            let mut diagnostic = probe.stdout;
            diagnostic.extend_from_slice(&probe.stderr);
            if !probe.status.success() {
                anyhow::bail!(
                    "restore-jpeg preflight jxlinfo failed for {}: {}",
                    source.display(),
                    String::from_utf8_lossy(&diagnostic).trim()
                );
            }
            if !restore_jpeg_jxlinfo_has_jpeg_reconstruction(&diagnostic) {
                anyhow::bail!(
                    "restore-jpeg preflight refused {}: JPEG bitstream reconstruction data is unavailable",
                    source.display()
                );
            }
            let completed =
                verified.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if completed.is_multiple_of(250) || completed == files.len() {
                println!(
                    "[PREFLIGHT] verified {completed}/{} reversible JXL source files",
                    files.len()
                );
            }
            Ok::<(), anyhow::Error>(())
        })
    })?;
    Ok(())
}

fn restore_jpeg_keep_source_parallel(
    files: &[PathBuf],
    input_root: &Path,
    output_root: &Path,
    force: bool,
) -> anyhow::Result<(usize, usize, Vec<RestoreJpegManifestRecord>)> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let available_workers = match std::thread::available_parallelism() {
        Ok(worker_count) => worker_count.get(),
        Err(error) => {
            eprintln!(
                "[RESTORE ] available parallelism could not be detected ({error}); using one worker"
            );
            1
        }
    };
    let worker_count = available_workers.clamp(1, 4);
    println!("[RESTORE ] using {worker_count} bounded workers; source JXL deletion is disabled");

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .thread_name(|index| format!("mfb-restore-jpeg-{index}"))
        .build()
        .context("restore-jpeg failed to create bounded worker pool")?;
    let processed = AtomicUsize::new(0);
    let restored = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let file_count = files.len();

    let pending = pool.install(|| {
        files
            .par_iter()
            .map(|file| {
                let result = restore_single_jpeg(file, input_root, output_root, force)?;
                if result.committed {
                    restored.fetch_add(1, Ordering::Relaxed);
                } else {
                    skipped.fetch_add(1, Ordering::Relaxed);
                }
                let completed = processed.fetch_add(1, Ordering::Relaxed) + 1;
                if completed.is_multiple_of(250) || completed == file_count {
                    println!(
                        "[RESTORE ] verified {completed}/{file_count} JPEG outputs (new={} existing={})",
                        restored.load(Ordering::Relaxed),
                        skipped.load(Ordering::Relaxed)
                    );
                }
                Ok(result)
            })
            .collect::<Vec<anyhow::Result<RestoreJpegResult>>>()
    });

    let mut records = Vec::with_capacity(file_count);
    for result in pending {
        let result = result?;
        record_retained_restored_jpeg_source(&mut records, &result.proof);
    }
    Ok((
        restored.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed),
        records,
    ))
}

fn run_restore_jpeg(
    input: &Path,
    output_dir: Option<&Path>,
    recursive: bool,
    force: bool,
    keep_source: bool,
) -> anyhow::Result<()> {
    if let Err(err) = foundation::tools::require(&["jxlinfo", "djxl", "exiftool"]) {
        log_fatal!(foundation::infra::static_logs::messages::LABEL_TOOLS, &err);
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    let input_root = restore_jpeg_input_root(input)?;
    let output_root = match output_dir {
        Some(path) => path.to_path_buf(),
        None => restore_jpeg_default_output_dir(input)?,
    };
    let files = restore_jpeg_candidate_files(input, recursive)?;
    let file_count = files.len();
    restore_jpeg_preflight(&input_root, &output_root, &files)?;
    println!(
        "[SCAN    ] Found {} reversible JXL files in {}",
        files.len(),
        input_root.display()
    );

    let mut restored = 0usize;
    let mut skipped = 0usize;
    let mut deleted_sources = 0usize;
    let mut deleted_source_dirs = Vec::new();
    let mut restore_records = Vec::new();
    write_restore_jpeg_manifest(&output_root, &restore_records)?;

    if keep_source {
        (restored, skipped, restore_records) =
            restore_jpeg_keep_source_parallel(&files, &input_root, &output_root, force)?;
        write_restore_jpeg_manifest(&output_root, &restore_records).with_context(|| {
            format!(
                "restore-jpeg failed to commit retained-source manifest after {file_count} files"
            )
        })?;
        foundation::preserve_directory_with_log(&input_root, &output_root).with_context(|| {
            format!(
                "restore-jpeg failed to preserve directory metadata {} -> {}",
                input_root.display(),
                output_root.display()
            )
        })?;
        println!(
            "[DONE    ] restored {restored} JPEGs to {} ({skipped} existing outputs skipped) source JXLs deleted=0 retained={file_count}",
            output_root.display()
        );
        return Ok(());
    }

    for file in files {
        let result = restore_single_jpeg(&file, &input_root, &output_root, force)?;
        if result.committed {
            restored += 1;
        } else {
            skipped += 1;
        }
        if record_and_delete_restored_jpeg_source(
            &output_root,
            &mut restore_records,
            &result.proof,
        )? {
            deleted_sources += 1;
            if let Some(parent) = file.parent() {
                deleted_source_dirs.push(parent.to_path_buf());
            }
        }

        let processed = restored + skipped;
        if processed.is_multiple_of(250) || processed == file_count {
            println!(
                "[RESTORE ] verified {processed}/{file_count} JPEG outputs (new={restored} existing={skipped})"
            );
        }
    }
    restore_jpeg_prune_empty_source_dirs(&input_root, &deleted_source_dirs);
    foundation::preserve_directory_with_log(&input_root, &output_root).with_context(|| {
        format!(
            "restore-jpeg failed to preserve directory metadata {} -> {}",
            input_root.display(),
            output_root.display()
        )
    })?;
    println!(
        "[DONE    ] restored {restored} JPEGs to {} ({skipped} existing outputs skipped) source JXLs deleted={deleted_sources} retained={}",
        output_root.display(),
        0
    );
    Ok(())
}

fn fast_img_source_hash_set(
    src_dir: &Path,
    source_jpegs: &[PathBuf],
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for source in source_jpegs {
        let rel = source.strip_prefix(src_dir).map_err(|err| {
            anyhow::anyhow!(
                "fast-img source hash path outside root: source={} root={} ({err})",
                source.display(),
                src_dir.display()
            )
        })?;
        hashes.insert(
            rel.to_string_lossy().to_string(),
            calculate_blake3_hash(source)?,
        );
    }
    Ok(hashes)
}

fn fast_img_marker_recorded_source_hashes(
    marker: &WorkingCopyMarker,
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for (rel, entry) in &marker.blake3_log {
        if hashes.insert(rel.clone(), entry.src.clone()).is_some() {
            anyhow::bail!("fast-img marker has duplicate recorded source path: {rel}");
        }
    }
    for (rel, entry) in &marker.skipped_sources {
        if hashes.insert(rel.clone(), entry.src.clone()).is_some() {
            anyhow::bail!("fast-img marker records {rel} as both converted and skipped");
        }
    }
    for (rel, entry) in &marker.failed_sources {
        if hashes.insert(rel.clone(), entry.src.clone()).is_some() {
            anyhow::bail!("fast-img marker records {rel} as both converted and failed");
        }
    }
    Ok(hashes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FastImgCleanupCompleteSourceState {
    DeletedConverted,
    RestoredOriginal,
    StaleCurrent,
}

fn fast_img_cleanup_complete_source_state(
    marker: &WorkingCopyMarker,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<FastImgCleanupCompleteSourceState> {
    if marker.recorded_source_count() != marker.src_jpeg_count {
        anyhow::bail!(
            "fast-img cleanup marker requires complete source disposition: marker_count={} converted={} skipped={} failed={}",
            marker.src_jpeg_count,
            marker.blake3_log.len(),
            marker.skipped_sources.len(),
            marker.failed_sources.len()
        );
    }
    let retained_hashes = marker
        .skipped_sources
        .iter()
        .chain(marker.failed_sources.iter())
        .map(|(rel, entry)| (rel.clone(), entry.src.clone()))
        .collect::<BTreeMap<_, _>>();
    let retained_source_count = marker
        .skipped_sources
        .len()
        .checked_add(marker.failed_sources.len())
        .context("fast-img retained source count overflowed usize")?;
    let marker_hashes = fast_img_marker_recorded_source_hashes(marker)?;
    if current_count == marker.src_jpeg_count && marker_hashes == *current_source_hashes {
        return Ok(FastImgCleanupCompleteSourceState::RestoredOriginal);
    }
    if current_count == retained_source_count {
        if retained_hashes != *current_source_hashes {
            anyhow::bail!("fast-img cleanup marker retained-source hash set changed");
        }
        return Ok(FastImgCleanupCompleteSourceState::DeletedConverted);
    }
    tracing::warn!(
        target: "fast_img",
        "fast-img cleanup marker source count mismatch after converted-source deletion: retained={} current={current_count}",
        retained_source_count
    );
    Ok(FastImgCleanupCompleteSourceState::StaleCurrent)
}

fn validate_cleanup_complete_marker(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    if marker.stage == FastImgStageName::CleanupComplete {
        validate_cleanup_complete_deleted_source_state(
            marker,
            src_dir,
            current_count,
            current_source_hashes,
        )?;
        return Ok(());
    }
    validate_fast_img_marker_source_state(marker, src_dir, current_count, current_source_hashes)?;
    Ok(())
}

fn validate_cleanup_retry_marker_source_state(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    if marker.src_dir != src_dir {
        anyhow::bail!(
            "fast-img marker belongs to {}, not {}; refusing failed-source retry",
            marker.src_dir.display(),
            src_dir.display()
        );
    }
    let retained_hashes = marker
        .skipped_sources
        .iter()
        .chain(marker.failed_sources.iter())
        .map(|(rel, entry)| (rel.clone(), entry.src.clone()))
        .collect::<BTreeMap<_, _>>();
    if retained_hashes.len() != current_count || retained_hashes != *current_source_hashes {
        anyhow::bail!("fast-img failed-source retry retained source set changed");
    }
    if marker.recorded_source_count() != marker.src_jpeg_count {
        anyhow::bail!(
            "fast-img failed-source retry marker requires complete source disposition: marker_count={} converted={} skipped={} failed={}",
            marker.src_jpeg_count,
            marker.blake3_log.len(),
            marker.skipped_sources.len(),
            marker.failed_sources.len()
        );
    }
    Ok(())
}

fn validate_cleanup_complete_deleted_source_state(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    match fast_img_cleanup_complete_source_state(marker, current_count, current_source_hashes)? {
        FastImgCleanupCompleteSourceState::DeletedConverted => {}
        FastImgCleanupCompleteSourceState::RestoredOriginal
        | FastImgCleanupCompleteSourceState::StaleCurrent => {
            anyhow::bail!(
                "fast-img cleanup marker source count mismatch after converted-source deletion: retained={} current={current_count}",
                marker.skipped_sources.len() + marker.failed_sources.len()
            );
        }
    }
    for (rel, entry) in &marker.blake3_log {
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        if source.exists() {
            anyhow::bail!(
                "fast-img cleanup marker expected deleted source JPEG to be absent: {}",
                source.display()
            );
        }
        if entry.out.is_empty() {
            anyhow::bail!("fast-img cleanup marker has empty output hash for {rel}");
        }
        let output = fast_img_marker_entry_output_path(marker, rel, entry)?;
        let local_output_optional = fast_img_cleanup_complete_has_shortest_path_proof(marker)
            && entry.library_asset.as_deref() == Some(entry.out.as_str());
        let output_metadata = match std::fs::metadata(&output) {
            Ok(metadata) => metadata,
            Err(err) if local_output_optional && err.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(
                    target: "fast_img",
                    path = %output.display(),
                    "fast-img shortest-path cleanup marker accepts deleted local JXL output after Photos/iCloud proof"
                );
                continue;
            }
            Err(err) => {
                let marker_file_path = marker_path_for_working_copy(&marker.working_copy);
                return Err(err).with_context(|| {
                    format!(
                        "fast-img cleanup marker cannot stat JXL output {}; source JPEGs are already absent, so restore this optimized JXL or restore the source backup before rerunning shortest-path fastmode (to force reset, delete the marker file at: {})",
                        output.display(),
                        marker_file_path.display()
                    )
                });
            }
        };
        if output_metadata.len() == 0 {
            anyhow::bail!(
                "fast-img cleanup marker found empty JXL output {}",
                output.display()
            );
        }
        let output_hash = calculate_blake3_hash(&output)?;
        if output_hash != entry.out {
            anyhow::bail!(
                "fast-img cleanup marker output hash drifted for {}",
                output.display()
            );
        }
    }
    for (rel, entry) in &marker.skipped_sources {
        if entry.src.is_empty() || entry.reason.trim().is_empty() {
            anyhow::bail!("fast-img cleanup marker has incomplete skipped-source proof for {rel}");
        }
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        if !source.exists() {
            anyhow::bail!(
                "fast-img cleanup marker expected skipped source JPEG to remain: {}",
                source.display()
            );
        }
    }
    for (rel, entry) in &marker.failed_sources {
        if entry.src.is_empty() || entry.reason.trim().is_empty() {
            anyhow::bail!("fast-img cleanup marker has incomplete failed-source proof for {rel}");
        }
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        if !source.exists() {
            anyhow::bail!(
                "fast-img cleanup marker expected failed source JPEG to remain: {}",
                source.display()
            );
        }
    }
    Ok(())
}

fn validate_fast_img_marker_source_state(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    if marker.src_dir != src_dir {
        anyhow::bail!(
            "fast-img marker belongs to {}, not {}; refusing stale resume/no-op",
            marker.src_dir.display(),
            src_dir.display()
        );
    }
    let cleanup_resume_subset =
        fast_img_cleanup_resume_source_subset_matches(marker, current_source_hashes)?;
    if marker.src_jpeg_count != current_count && !cleanup_resume_subset {
        anyhow::bail!(
            "fast-img marker source count changed: marker={} current={current_count}",
            marker.src_jpeg_count
        );
    }
    let marker_hashes = fast_img_marker_recorded_source_hashes(marker)?;
    if !marker_hashes.is_empty() {
        let partial_log_allowed = marker.stage == FastImgStageName::Gate1Failed
            || marker.stage == FastImgStageName::OutputPrepared;
        let marker_hashes_match = if cleanup_resume_subset {
            true
        } else if partial_log_allowed {
            marker_hashes.iter().all(|(rel, hash)| {
                current_source_hashes
                    .get(rel)
                    .is_some_and(|current| current == hash)
            })
        } else {
            marker_hashes == *current_source_hashes
        };
        if !marker_hashes_match {
            anyhow::bail!("fast-img marker source hash set changed; refusing stale resume/no-op");
        }
    }
    if marker_hashes.is_empty()
        && output_prepared_or_later(&marker.stage)
        && marker.stage != FastImgStageName::OutputPrepared
    {
        anyhow::bail!(
            "fast-img marker missing BLAKE3 source log for post-encode JXL-only output; refusing stale resume"
        );
    }
    Ok(())
}

fn fast_img_marker_input_state_is_stale(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
    strategy: &str,
) -> anyhow::Result<bool> {
    if marker.src_dir != src_dir || marker.strategy != strategy {
        return Ok(true);
    }

    let marker_hashes = fast_img_marker_recorded_source_hashes(marker)?;
    if marker.src_jpeg_count != current_count {
        return Ok(!fast_img_cleanup_resume_source_subset_matches(
            marker,
            current_source_hashes,
        )?);
    }
    if marker_hashes.is_empty() {
        return Ok(output_prepared_or_later(&marker.stage)
            && marker.stage != FastImgStageName::OutputPrepared);
    }
    let partial_log_allowed = marker.stage == FastImgStageName::Gate1Failed
        || marker.stage == FastImgStageName::OutputPrepared;
    if partial_log_allowed {
        return Ok(!marker_hashes.iter().all(|(rel, hash)| {
            current_source_hashes
                .get(rel)
                .is_some_and(|current| current == hash)
        }));
    }
    Ok(marker_hashes != *current_source_hashes)
}

fn fast_img_skip_hashes_match(src: &Path, out: &Path, entry: &Blake3Entry) -> anyhow::Result<bool> {
    if entry.src.is_empty() || entry.out.is_empty() {
        return Ok(false);
    }
    let src_hash = match calculate_blake3_hash(src) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!(
                target: "fast_img",
                source = %src.display(),
                error = %err,
                "fast-img resume source BLAKE3 read failed"
            );
            return Err(anyhow::anyhow!(
                "fast-img resume source BLAKE3 read failed for {}: {err}",
                src.display()
            ));
        }
    };
    if src_hash != entry.src {
        return Ok(false);
    }
    let out_hash = match calculate_blake3_hash(out) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!(
                target: "fast_img",
                output = %out.display(),
                error = %err,
                "fast-img resume output BLAKE3 read failed"
            );
            return Err(anyhow::anyhow!(
                "fast-img resume output BLAKE3 read failed for {}: {err}",
                out.display()
            ));
        }
    };
    Ok(out_hash == entry.out)
}

fn print_gate_result(name: &str, result: &foundation::pipeline::verification::GateResult) {
    let status = if result.passed { "PASS" } else { "FAIL" };
    let checks = result
        .checks
        .iter()
        .map(|check| format!("{}:{}", check.name, if check.passed { "✅" } else { "❌" }))
        .collect::<Vec<_>>()
        .join(" ");
    println!("[{name:<8}] {checks} → {status}");
    let show_pass_details = result.passed && matches!(name, "GATE 2" | "GATE 3");
    for check in &result.checks {
        if !check.passed {
            println!(
                "           {}: expected {}, got {}",
                check.name, check.expected, check.actual
            );
            for path in &check.affected_files {
                println!("           ❌ {}", path.display());
            }
        } else if show_pass_details {
            println!(
                "           {} proof: expected {}, actual {}",
                check.name, check.expected, check.actual
            );
        }
    }
}

fn print_photos_verifier_proof_summary(library: &LibraryHandle, expected_count: usize) {
    let uploaded = library
        .imported_assets
        .iter()
        .filter(|asset| asset.sync_status == "uploaded")
        .count();
    let photos_local = library
        .imported_assets
        .iter()
        .filter(|asset| asset.sync_status == "photos_local")
        .count();
    let other_sync = library
        .imported_assets
        .len()
        .saturating_sub(uploaded + photos_local);
    let quarantined = library
        .imported_assets
        .iter()
        .filter(|asset| asset.quarantined)
        .count();
    println!(
        "[PHOTOS ] verifier proof: imported={}/{} queried={} uploaded={} photos_local={} other_sync={} quarantined={} import_errors={}",
        library.imported_assets.len(),
        expected_count,
        library.imported_assets.len(),
        uploaded,
        photos_local,
        other_sync,
        quarantined,
        library.import_error_count
    );
    if other_sync > 0 {
        println!(
            "[PHOTOS ] verifier policy: unrecognized Photos sync state present; Gate 3 must fail unless every asset has accepted custody proof"
        );
    } else if photos_local > 0 {
        println!(
            "[PHOTOS ] verifier policy: local Photos custody accepted; set MFB_FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF=1 for slower iCloud-upload proof"
        );
    } else {
        println!(
            "[PHOTOS ] verifier policy: iCloud-upload proof established for all imported assets"
        );
    }
}

fn fast_img_record_controllable_media_import_failures(
    marker: &mut WorkingCopyMarker,
    candidates: &[PhotosImportCandidate],
    library: &mut LibraryHandle,
) -> anyhow::Result<usize> {
    if library.import_error_count == 0 {
        return Ok(0);
    }

    let imported = library
        .imported_assets
        .iter()
        .map(|asset| asset.rel_path.as_str())
        .collect::<BTreeSet<_>>();
    let rejected_outputs = candidates
        .iter()
        .filter(|candidate| !imported.contains(candidate.rel_path.as_str()))
        .map(|candidate| candidate.rel_path.clone())
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(
        rejected_outputs.len() == library.import_error_count,
        "Photos partial-import accounting mismatch: rejected_outputs={} import_errors={}",
        rejected_outputs.len(),
        library.import_error_count
    );

    let mut rejected_sources = Vec::new();
    for (source_rel, entry) in &marker.blake3_log {
        let output = fast_img_marker_entry_output_path(marker, source_rel, entry)?;
        let output_rel =
            fast_img_output_rel_key(&output, &marker.working_copy, "photos_rejected_output_rel")?;
        if rejected_outputs.contains(&output_rel) {
            rejected_sources.push((source_rel.clone(), output, output_rel));
        }
    }
    anyhow::ensure!(
        rejected_sources.len() == rejected_outputs.len(),
        "Photos rejected-output mapping mismatch: outputs={} sources={}",
        rejected_outputs.len(),
        rejected_sources.len()
    );

    for (source_rel, output, output_rel) in rejected_sources {
        let entry = marker.blake3_log.remove(&source_rel).with_context(|| {
            format!("missing marker entry for rejected Photos output {output_rel}")
        })?;
        match std::fs::remove_file(&output) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "remove Photos-rejected working-copy output {}",
                        output.display()
                    )
                });
            }
        }
        marker.skipped_sources.remove(&source_rel);
        marker.failed_sources.insert(
            source_rel.clone(),
            SkippedSourceEntry {
                src: entry.src,
                reason: format!(
                    "Photos returned no verified result for this media item during normal-mode batch import: {output_rel}"
                ),
            },
        );
        println!(
            "[FAIL    ] {source_rel}: Photos batch import returned no proof; source retained and remaining media continue"
        );
    }

    let failure_count = library.import_error_count;
    library.import_error_count = 0;
    marker.encoded_count = marker.blake3_log.len();
    marker.gate2_checks = Gate2Checks::default();
    marker.gate3_checks = Gate3Checks::default();
    Ok(failure_count)
}

fn fast_img_commit_transcode_complete(marker: &mut WorkingCopyMarker, strategy: &str) {
    if strategy == "avif" {
        marker.metadata_policy_version = FAST_IMG_AVIF_CLEAN_POLICY_VERSION;
    }
    marker.stage = FastImgStageName::TranscodeComplete;
}

#[allow(clippy::too_many_arguments)]
fn fast_img_run_encode_phase(
    marker: &mut WorkingCopyMarker,
    source_jpegs: &[std::path::PathBuf],
    current_source_hashes: &std::collections::BTreeMap<String, String>,
    scan_failures: &std::collections::BTreeMap<String, String>,
    src_dir: &std::path::Path,
    working_copy: &std::path::Path,
    retry_failed_sources_from_cleanup: RetryFlag,
    archive: ArchiveFlag,
    allow_expert_options: ExpertOptionsFlag,
    strategy: &str,
) -> anyhow::Result<()> {
    let encode_label = if strategy == "avif" {
        "MEME MODE"
    } else {
        "ENCODE"
    };
    let source_kind = if strategy == "avif" {
        "static image"
    } else {
        "source JPEG"
    };
    let total = source_jpegs.len();
    let mut completed_from_resume = 0usize;
    let mut jobs = Vec::new();

    for source in source_jpegs {
        let rel = source.strip_prefix(src_dir).map_err(|err| {
            anyhow::anyhow!(
                "fast-img scan produced path outside source root: source={} root={} ({err})",
                source.display(),
                src_dir.display()
            )
        })?;
        let rel_key = rel.to_string_lossy().to_string();

        if let Some(reason) = scan_failures.get(&rel_key) {
            let src_hash = current_source_hashes
                .get(&rel_key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing scanned source hash for {rel_key}"))?;
            marker.blake3_log.remove(&rel_key);
            marker.skipped_sources.remove(&rel_key);
            marker.failed_sources.insert(
                rel_key.clone(),
                SkippedSourceEntry {
                    src: src_hash,
                    reason: reason.clone(),
                },
            );
            println!("[FAIL    ] {rel_key} {reason}");
            tracing::error!(
                target: "fast_img_failures",
                file = %rel_key,
                reason = %reason,
                "fast-img scan failure retained before encode"
            );
            continue;
        }

        if let Some(entry) = marker.skipped_sources.get(&rel_key) {
            fast_img_emit_explicit_skip(&rel_key, &entry.reason);
            continue;
        }
        if marker.failed_sources.contains_key(&rel_key) && !retry_failed_sources_from_cleanup.0 {
            if let Some(entry) = marker.failed_sources.get(&rel_key) {
                fast_img_emit_explicit_skip(
                    &rel_key,
                    &format!("failed (no retry): {}", entry.reason),
                );
            }
            continue;
        }

        let resume_out = marker
            .blake3_log
            .get(&rel_key)
            .map(|entry| fast_img_marker_entry_output_path(marker, &rel_key, entry))
            .transpose()?;
        let existing_output_current = match resume_out.as_ref() {
            Some(output) if output.exists() => match marker.blake3_log.get(&rel_key) {
                Some(entry) => fast_img_skip_hashes_match(source, output, entry)?,
                None => false,
            },
            _ => false,
        };
        if existing_output_current {
            let resume_out =
                resume_out.context("missing fast-img resume output after current proof")?;
            let resume_out_rel_key =
                fast_img_output_rel_key(&resume_out, working_copy, "fast_img_resume_output_rel")?;
            let expected_hash = marker
                .blake3_log
                .get(&rel_key)
                .map_or("", |e| e.out.as_str());
            match fast_img_check_reused_delivery(
                &resume_out,
                strategy,
                marker.metadata_policy_version,
                expected_hash,
            )? {
                ReuseDecision::Reusable { hash } => {
                    if let Some(entry) = marker.blake3_log.get_mut(&rel_key) {
                        if entry.out != hash {
                            entry.library_asset = None;
                        }
                        entry.out = hash;
                        entry.out_rel = Some(resume_out_rel_key.clone());
                    }
                    println!(
                        "[{encode_label}] reused verified output for {rel_key} -> {resume_out_rel_key}"
                    );
                    completed_from_resume += 1;
                    continue;
                }
                ReuseDecision::NeedsReencode { reason } => {
                    tracing::warn!(
                        target: "fast_img",
                        source_rel = %rel_key,
                        reason = %reason,
                        "reused output invalidated; scheduling clean re-encode"
                    );
                    if let Some(entry) = marker.blake3_log.get_mut(&rel_key) {
                        entry.out.clear();
                        entry.library_asset = None;
                        entry.out_rel = Some(resume_out_rel_key);
                    }
                    foundation::media_conversion_gate::delivery_remove_file_or_audit(
                        "fast_img invalidate obsolete reused output",
                        &resume_out,
                    );
                    write_marker_atomic(marker)?;
                }
            }
        }

        // If the marker already has a recorded out_rel for this source from a
        // prior run, pre-claim that exact path before calling
        // `fast_img_planned_output_rel`.  Without this, `reserve_output_path`
        // would see the on-disk JXL, find it unclaimed, and bump to a
        // collision suffix (`a (1).JXL`) — diverging from the marker record.
        //
        // `pre_claim_output_path` bypasses the `None if exists_on_disk => true`
        // arm and inserts the (output → input) pair directly.  We then call
        // `reserve_output_path` with the marker path (not the naive
        // `rel.with_extension("JXL")`) so that even a genuine prior-run
        // collision path like `a (1).JXL` is honoured.
        let (_reserved_out, out_rel_key) = if let Some(recorded_out_rel) = marker
            .blake3_log
            .get(&rel_key)
            .and_then(|e| e.out_rel.as_deref())
        {
            let recorded_out = working_copy.join(recorded_out_rel);
            foundation::conversion::pre_claim_output_path(source, &recorded_out);
            tracing::debug!(
                target: "fast_img",
                rel = %rel_key,
                recorded_out_rel = %recorded_out_rel,
                "pre-claimed marker output path for stale-proof reencode"
            );
            let reserved = foundation::conversion::reserve_output_path(source, &recorded_out);
            let out_rel_key =
                fast_img_output_rel_key(&reserved, working_copy, "fast_img_resume_reencode_rel")?;
            if reserved != recorded_out {
                tracing::warn!(
                    target: "fast_img",
                    rel = %rel_key,
                    recorded = %recorded_out_rel,
                    actual = %reserved.display(),
                    "stale-proof reencode: marker out_rel was already taken by another source; using new path"
                );
            }
            (reserved, out_rel_key)
        } else {
            fast_img_planned_output_rel(source, working_copy, rel, strategy)?
        };

        jobs.push(FastImgTranscodeJob {
            source: source.clone(),
            src_hash: current_source_hashes
                .get(&rel_key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing scanned source hash for {rel_key}"))?,
            rel_key,
            out_rel_key,
        });
    }

    let pending = jobs.len();
    if pending > 0 {
        let thread_config = foundation::thread_manager::get_balanced_thread_config(
            foundation::thread_manager::WorkloadType::Image,
        );
        let (parallel_tasks, child_threads) = fast_img_effective_encode_parallelism(
            pending,
            thread_config.parallel_tasks,
            thread_config.child_threads,
        );
        println!(
            "[{encode_label}] pending {pending}/{total} · skipped {completed_from_resume} · parallel {parallel_tasks} × {child_threads} threads"
        );
        tracing::info!(
            target: "fast_img",
            pending,
            skipped = completed_from_resume,
            total,
            parallel_tasks,
            child_threads,
            "fast-img parallel encode start"
        );
        let completed = AtomicUsize::new(0);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(parallel_tasks)
            .build()
            .map_err(|err| anyhow::anyhow!("fast-img encode thread pool init failed: {err}"))?;
        let results = pool.install(|| {
            jobs.par_iter()
                .map(|job| {
                    let result = fast_img_run_encode_job(
                        job,
                        src_dir,
                        working_copy,
                        child_threads,
                        archive.0,
                        allow_expert_options.0,
                        strategy,
                    );
                    if result.is_ok() {
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        println!("[{encode_label}] {done}/{pending} {}", job.source.display());
                    }
                    result
                })
                .collect::<Vec<_>>()
        });

        let mut encoded = completed_from_resume;
        let mut session_converted: u64 = 0;
        let mut session_source_bytes: u64 = 0;
        let mut session_output_bytes: u64 = 0;
        let mut session_failed = 0usize;
        let mut session_skipped = 0usize;
        for result in results {
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(err) => {
                    println!("[FAIL    ] {} {}", err.rel_key, err.reason);
                    fast_img_remove_failed_encode_output(working_copy, &err)?;
                    marker.blake3_log.remove(&err.rel_key);
                    marker.skipped_sources.remove(&err.rel_key);
                    marker.failed_sources.insert(
                        err.rel_key,
                        SkippedSourceEntry {
                            src: err.src_hash,
                            reason: err.reason,
                        },
                    );
                    session_failed += 1;
                    write_marker_atomic(marker)?;
                    continue;
                }
            };
            match outcome {
                FastImgTranscodeOutcome::Converted(proof) => {
                    // Session size summary counts ONLY files converted in this run:
                    // resume-reused, skipped, and failed items are excluded by
                    // construction (failed returns above; Skipped takes the other arm).
                    let src_abs = src_dir.join(&proof.rel_key);
                    let out_abs = working_copy.join(&proof.out_rel);
                    let src_len = std::fs::metadata(&src_abs)
                        .with_context(|| {
                            format!(
                                "stat converted source for size summary: {}",
                                src_abs.display()
                            )
                        })?
                        .len();
                    let out_len = std::fs::metadata(&out_abs)
                        .with_context(|| {
                            format!(
                                "stat converted JXL output for size summary: {}",
                                out_abs.display()
                            )
                        })?
                        .len();
                    session_converted += 1;
                    session_source_bytes = session_source_bytes
                        .checked_add(src_len)
                        .context("source byte accumulation overflowed u64")?;
                    session_output_bytes = session_output_bytes
                        .checked_add(out_len)
                        .context("output byte accumulation overflowed u64")?;
                    marker.skipped_sources.remove(&proof.rel_key);
                    marker.failed_sources.remove(&proof.rel_key);
                    marker.blake3_log.insert(
                        proof.rel_key,
                        Blake3Entry {
                            out_rel: Some(proof.out_rel),
                            src: proof.src_hash,
                            out: proof.out_hash,
                            library_asset: None,
                        },
                    );
                    encoded += 1;
                    marker.encoded_count = encoded;
                    write_marker_atomic(marker)?;
                }
                FastImgTranscodeOutcome::Skipped(proof) => {
                    fast_img_emit_explicit_skip(&proof.rel_key, &proof.reason);
                    session_skipped += 1;
                    marker.blake3_log.remove(&proof.rel_key);
                    marker.failed_sources.remove(&proof.rel_key);
                    marker.skipped_sources.insert(
                        proof.rel_key,
                        SkippedSourceEntry {
                            src: proof.src_hash,
                            reason: proof.reason,
                        },
                    );
                }
            }
            write_marker_atomic(marker)?;
        }
        print_fast_img_session_size_summary(
            session_converted,
            session_source_bytes,
            session_output_bytes,
            u64::try_from(completed_from_resume)
                .context("fast-img resume reuse count exceeds u64")?,
        )?;
        if session_skipped > 0 {
            println!(
                "[SKIP    ] {session_skipped} {source_kind}(s) explicitly skipped during encode"
            );
            for (rel, entry) in &marker.skipped_sources {
                let reason = &entry.reason;
                println!("[SKIP    ]   {rel}: {reason}  [SOURCE RETAINED]");
                tracing::warn!(
                    target: "fast_img_skips",
                    file = %rel,
                    reason = %reason,
                    "fast-img source skipped — original file retained"
                );
            }
        }
        if session_failed > 0 {
            println!("[FAIL    ] {session_failed} {source_kind}(s) failed and were left in place");
            // Enumerate per-file reasons so the user knows exactly which files
            // failed without grepping log shards.
            for (rel, entry) in &marker.failed_sources {
                let reason = &entry.reason;
                println!("[FAIL    ]   {rel}: {reason}");
                tracing::error!(
                    target: "fast_img_failures",
                    file = %rel,
                    reason = %reason,
                    "fast-img source failed"
                );
            }
        }
        if !marker.failed_sources.is_empty() || !marker.skipped_sources.is_empty() {
            println!(
                "[RETAIN  ] {} source file(s) retained in: {}",
                marker.failed_sources.len() + marker.skipped_sources.len(),
                src_dir.display()
            );
        }
    } else {
        marker.encoded_count = completed_from_resume;
        println!(
            "[{encode_label}] 0 pending · reused {completed_from_resume}/{total} verified outputs"
        );
        let (resume_source_bytes, resume_output_bytes) =
            fast_img_marker_delivery_byte_totals(marker, src_dir)?;
        print_fast_img_session_size_summary(
            0,
            resume_source_bytes,
            resume_output_bytes,
            u64::try_from(completed_from_resume)
                .context("fast-img resume reuse count exceeds u64")?,
        )?;
    }
    let reconciled = fast_img_reconcile_unrecorded_source_disposition(
        marker,
        src_dir,
        source_jpegs,
        current_source_hashes,
    )?;
    if reconciled > 0 {
        println!("[SKIP    ] {reconciled} {source_kind}(s) reconciled as explicit skips");
    }
    fast_img_commit_transcode_complete(marker, strategy);
    write_marker_atomic(marker)?;
    Ok(())
}

fn fast_img_deliver_modern_lossy_static_tier(
    marker: &mut WorkingCopyMarker,
    src_dir: &Path,
    candidates: &[ModernLossyStaticCandidate],
) -> anyhow::Result<(usize, usize, usize)> {
    if candidates.is_empty() && !marker.tier2_in_progress {
        return Ok((0, 0, 0));
    }
    marker.tier2_in_progress = true;
    write_marker_atomic(marker)?;
    println!(
        "[TIER 2  ] reconciling/importing {} modern lossy static source(s) with Photos",
        candidates.len()
    );
    let library_handle = if candidates.is_empty() {
        foundation::pipeline::verification::LibraryHandle::default()
    } else {
        import_modern_lossy_static_tier(src_dir, candidates)
            .map_err(|err| anyhow::anyhow!("fast-img modern lossy Photos delivery failed: {err}"))?
    };

    if library_handle.import_error_count != 0
        || library_handle.imported_assets.len() != candidates.len()
    {
        anyhow::bail!(
            "fast-img modern lossy Photos delivery incomplete: verified={} failed={} expected={}; all remaining sources retained",
            library_handle.imported_assets.len(),
            library_handle.import_error_count,
            candidates.len()
        );
    }
    apply_tier2_library_assets_to_marker(marker, &library_handle)
        .map_err(|err| anyhow::anyhow!("fast-img modern lossy marker proof failed: {err}"))?;
    write_marker_atomic(marker)?;
    if marker.tier2_imported_assets.is_empty() {
        anyhow::bail!(
            "fast-img tier-2 recovery has no persisted Photos custody proof; source files retained"
        );
    }
    let complete_library_handle = foundation::pipeline::verification::LibraryHandle {
        imported_assets: marker.tier2_imported_assets.clone(),
        import_error_count: 0,
    };
    let (deleted, already_deleted) =
        delete_verified_modern_lossy_static_sources(src_dir, &complete_library_handle).map_err(
            |err| anyhow::anyhow!("fast-img modern lossy source delete gate failed: {err}"),
        )?;
    let pruned =
        prune_empty_source_dirs_for_tier2_assets(src_dir, &complete_library_handle.imported_assets)
            .map_err(|err| {
                anyhow::anyhow!("fast-img modern lossy empty-directory cleanup failed: {err}")
            })?;
    marker.tier2_in_progress = false;
    write_marker_atomic(marker)?;
    Ok((deleted, already_deleted, pruned))
}

#[allow(clippy::too_many_arguments)]
fn fast_img_run_verification_and_delivery_pipeline(
    marker: &mut WorkingCopyMarker,
    source_jpegs: &[std::path::PathBuf],
    current_source_hashes: &std::collections::BTreeMap<String, String>,
    src_dir: &std::path::Path,
    working_copy: &std::path::Path,
    saved_dir_timestamps: &foundation::metadata::DirectoryTimestampsMap,
    retry_failed_sources_from_cleanup: bool,
    resume_local_delivery_for_shortest_path: bool,
    shortest_path: ShortestPathFlag,
    _auto_import: AutoImportFlag,
    reuse_marker_import_proof: bool,
    modern_lossy_candidates: &[ModernLossyStaticCandidate],
    strategy: &str,
) -> anyhow::Result<()> {
    let mode_name = if strategy == "avif" {
        "AVIF-only (Meme Mode)"
    } else {
        "JXL-only"
    };
    let ext_name = if strategy == "avif" { "AVIF" } else { "JXL" };
    let source_type = if strategy == "avif" {
        "images"
    } else {
        "JPEGs"
    };
    let source_kind = if strategy == "avif" {
        "static image"
    } else {
        "source JPEG"
    };
    let reconciled = fast_img_reconcile_unrecorded_source_disposition(
        marker,
        src_dir,
        source_jpegs,
        current_source_hashes,
    )?;
    if reconciled > 0 {
        write_marker_atomic(marker)?;
        println!(
            "[SKIP    ] {reconciled} {source_kind}(s) reconciled as explicit skips before delivery"
        );
    }
    let mut expected_count = fast_img_effective_expected_count(
        marker,
        source_jpegs.len(),
        resume_local_delivery_for_shortest_path,
    );

    // Fail early if all sources failed during encoding
    if expected_count == 0
        && !marker.failed_sources.is_empty()
        && modern_lossy_candidates.is_empty()
        && !marker.tier2_in_progress
    {
        anyhow::bail!(
            "All {} {source_kind}(s) failed during encoding; no outputs to verify. Check logs for per-file failure reasons.",
            marker.failed_sources.len(),
        );
    }

    let output_format = foundation::delivery_codec_strategy::strategy_to_format_kind(strategy);
    let ctx = fast_img_pipeline_ctx(marker, expected_count, None, output_format);
    if !gate1_complete_or_later(&marker.stage) {
        println!("[GATE 1  ] verifying local outputs");
        let gate1 = Gate1Local.run(&ctx);
        print_gate_result("GATE 1", &gate1);
        marker.apply_gate1(&gate1);
        write_marker_atomic(marker)?;
        if !gate1.passed {
            // @ANCHOR:no-silent-gate-fail — every gate fail surfaces CheckDetail per check to UI/log
            anyhow::bail!(
                "Gate 1 failed: {}; wc preserved at {}",
                marker_checks_from_result(&gate1),
                working_copy.display()
            );
        }
    }

    if fast_img_post_gate1_policy(shortest_path) == FastImgPostGate1Policy::LocalOnlyDelivery {
        if retry_failed_sources_from_cleanup {
            fast_img_validate_cleanup_retry_jxl_only_delivery_exit(
                marker,
                source_jpegs.len(),
                current_source_hashes,
                strategy,
            )?;
        } else {
            fast_img_validate_jxl_only_delivery_exit(
                marker,
                source_jpegs.len(),
                current_source_hashes,
                strategy,
            )?;
        }
        let (tier2_deleted, tier2_already_deleted, tier2_dirs_pruned) = if strategy == "jxl" {
            fast_img_deliver_modern_lossy_static_tier(marker, src_dir, modern_lossy_candidates)?
        } else {
            (0, 0, 0)
        };
        let (source_deleted, source_already_deleted) =
            fast_img_delete_verified_source_jpegs(marker, src_dir, strategy)?;
        let source_dirs_pruned = fast_img_prune_empty_source_dirs(marker, src_dir)?;
        fast_img_strip_non_target_files(working_copy, strategy)?;
        foundation::restore_delivery_directory_metadata(
            saved_dir_timestamps,
            src_dir,
            working_copy,
        )
        .with_context(|| {
            format!(
                "restore fast-img directory metadata {} -> {} after local-only cleanup",
                src_dir.display(),
                working_copy.display()
            )
        })?;
        marker.stage = FastImgStageName::CleanupComplete;
        marker.error = None;
        write_marker_atomic(marker)?;
        println!(
            "[DELIVER ] Gate 1 passed; {mode_name} output at {}; source {source_type} deleted={} already_absent={} modern_lossy_deleted={} modern_lossy_already_absent={} empty_dirs_pruned={}",
            working_copy.display(),
            source_deleted,
            source_already_deleted,
            tier2_deleted,
            tier2_already_deleted,
            source_dirs_pruned + tier2_dirs_pruned
        );
        return Ok(());
    }

    let import_candidates = build_fast_img_output_import_candidates(marker)?;
    let mut library_handle = if import_complete_or_later(&marker.stage) {
        let library_handle = if fast_img_marker_has_complete_import_proof(marker) {
            let library_handle = reverify_media_outputs_with_library_verifier(
                &import_candidates,
                &marker.photos_imported_assets,
            )
            .map_err(|err| {
                anyhow::anyhow!(
                    "fast-img resume Photos UUID re-verification failed; source files retained: {err}"
                )
            })?;
            tracing::info!(
                target: "fast_img",
                imported = library_handle.imported_assets.len(),
                gate3_retry = reuse_marker_import_proof,
                "fast-img refreshed persisted Photos UUID proof for retry/resume"
            );
            library_handle
        } else {
            anyhow::bail!(
                "fast-img resume marker has no complete Photos UUID proof; refusing automatic re-import or source deletion"
            )
        };
        apply_library_assets_to_marker(marker, &library_handle)
            .map_err(|err| anyhow::anyhow!("fast-img marker/library verifier mismatch: {err}"))?;
        library_handle
    } else {
        let reconcile_interrupted_import = marker.stage == FastImgStageName::Importing;
        if !reconcile_interrupted_import {
            marker.stage = FastImgStageName::Importing;
            marker.error = None;
            write_marker_atomic(marker)?;
        }
        let library_handle = import_media_outputs_with_checkpointed_library_verifier(
            marker,
            reconcile_interrupted_import,
        )
        .map_err(|err| {
            anyhow::anyhow!("fast-img shortest-path AVIF import verifier failed: {err}")
        })?;
        apply_library_assets_to_marker(marker, &library_handle)
            .map_err(|err| anyhow::anyhow!("fast-img marker/library verifier mismatch: {err}"))?;
        marker.stage = FastImgStageName::ImportComplete;
        marker.error = None;
        library_handle
    };
    let controlled_import_failures = fast_img_record_controllable_media_import_failures(
        marker,
        &import_candidates,
        &mut library_handle,
    )?;
    if controlled_import_failures > 0 {
        expected_count = fast_img_effective_expected_count(
            marker,
            source_jpegs.len(),
            resume_local_delivery_for_shortest_path,
        );
        marker.stage = FastImgStageName::ImportComplete;
        marker.error = None;
        println!(
            "[PHOTOS ] normal mode retained {controlled_import_failures} rejected source(s); continuing Gates 2/3 for {expected_count} verified import(s)"
        );
    }
    write_marker_atomic(marker)?;

    if !gate2_complete_or_later(&marker.stage) {
        print_photos_verifier_proof_summary(&library_handle, expected_count);
        println!("[GATE 2  ] verifying Photos import");
        let gate2 = Gate2Import.run(&fast_img_pipeline_ctx(
            marker,
            expected_count,
            Some(library_handle.clone()),
            output_format,
        ));
        print_gate_result("GATE 2", &gate2);
        marker.apply_gate2(&gate2);
        write_marker_atomic(marker)?;
        if !gate2.passed {
            anyhow::bail!(
                "Gate 2 failed: {}; wc preserved at {}",
                marker_checks_from_result(&gate2),
                working_copy.display()
            );
        }
    }

    if !deep_scan_complete_or_later(&marker.stage) {
        println!("[DEEP    ] scanning Photos library consistency");
        marker.stage = FastImgStageName::DeepScanComplete;
        write_marker_atomic(marker)?;
    }

    if !gate3_complete_or_later(&marker.stage) {
        println!("[GATE 3  ] verifying before cleanup");
        let gate3 = Gate3Deep.run(&fast_img_pipeline_ctx(
            marker,
            expected_count,
            Some(library_handle),
            output_format,
        ));
        print_gate_result("GATE 3", &gate3);
        marker.apply_gate3(&gate3);
        write_marker_atomic(marker)?;
        if !gate3.passed {
            anyhow::bail!(
                "Gate 3 failed: {}; wc preserved at {}",
                marker_checks_from_result(&gate3),
                working_copy.display()
            );
        }
    }

    let (source_deleted, source_already_deleted) =
        fast_img_delete_verified_source_jpegs(marker, src_dir, strategy)?;
    let source_dirs_pruned = fast_img_prune_empty_source_dirs(marker, src_dir)?;
    fast_img_strip_non_target_files(working_copy, strategy)?;
    foundation::restore_delivery_directory_metadata(saved_dir_timestamps, src_dir, working_copy)
        .with_context(|| {
            format!(
                "restore fast-img directory metadata {} -> {} after shortest-path cleanup",
                src_dir.display(),
                working_copy.display()
            )
        })?;
    tracing::info!(
        target: "fast_img",
        deleted = source_deleted,
        already_absent = source_already_deleted,
        empty_dirs_pruned = source_dirs_pruned,
        src_dir = %src_dir.display(),
        "fast-img deleted verified source files after Gate 3"
    );

    marker.stage = FastImgStageName::CleanupComplete;
    marker.error = None;
    write_marker_atomic(marker)?;
    println!(
        "[DONE    ] {} {ext_name} files · {} source {source_type} deleted · {} empty source dirs pruned · {mode_name} output at {} · gates: ①②③ all ✅",
        expected_count,
        source_deleted,
        source_dirs_pruned,
        working_copy.display()
    );
    Ok(())
}

fn auto_convert_build_options(
    config: &AutoConvertConfig,
    analysis: &foundation::image_analyzer::ImageAnalysis,
    quality_label: String,
) -> img::lossless_converter::ConvertOptions {
    img::lossless_converter::ConvertOptions {
        output_dir: config.output_dir.clone(),
        base_dir: config.base_dir.clone(),
        flags: {
            img::lossless_converter::ConvertFlags::empty()
                | if config.force() {
                    img::lossless_converter::ConvertFlags::FORCE
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.delete_original() {
                    img::lossless_converter::ConvertFlags::DELETE_ORIGINAL
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.in_place() {
                    img::lossless_converter::ConvertFlags::IN_PLACE
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.explore() {
                    img::lossless_converter::ConvertFlags::EXPLORE
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.match_quality() {
                    img::lossless_converter::ConvertFlags::MATCH_QUALITY
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.compress() {
                    img::lossless_converter::ConvertFlags::COMPRESS
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.apple_compat() {
                    img::lossless_converter::ConvertFlags::APPLE_COMPAT
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.use_gpu() {
                    img::lossless_converter::ConvertFlags::USE_GPU
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.ultimate() {
                    img::lossless_converter::ConvertFlags::ULTIMATE
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.allow_size_tolerance() {
                    img::lossless_converter::ConvertFlags::ALLOW_SIZE_TOLERANCE
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.allow_expert_options() {
                    img::lossless_converter::ConvertFlags::ALLOW_EXPERT_OPTIONS
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.archive() {
                    img::lossless_converter::ConvertFlags::ARCHIVE
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
                | if config.verbose() {
                    img::lossless_converter::ConvertFlags::VERBOSE
                } else {
                    img::lossless_converter::ConvertFlags::empty()
                }
        },
        child_threads: if config.child_threads > 0 {
            config.child_threads
        } else {
            2
        },
        input_format: Some(analysis.format.clone()),
        quality_label: Some(quality_label),
        codec: foundation::conversion_types::SelectedCodec::Hevc,
    }
}

#[allow(clippy::too_many_arguments)]
fn auto_convert_directory_output_completeness_verification(
    config: &AutoConvertConfig,
    output_dir: &std::path::Path,
    recursive: bool,
    ignored_count: usize,
    failed_count: usize,
    result: &mut foundation::Summary,
    post_run_errors: &mut Vec<String>,
) {
    log_detail!("");
    foundation::log_static!(
        info,
        foundation::infra::static_logs::messages::OUTPUT_VERIFY
    );
    let verify = foundation::verify_output_completeness_for_domain(
        foundation::media_conversion_gate::base_dir_or_default(
            config.base_dir.as_deref(),
            "verify_output_base",
        ),
        output_dir,
        recursive,
        foundation::VerifyDomain::ImagesAndPassthrough,
    );
    let adjusted_expected = verify
        .expected
        .saturating_sub(ignored_count)
        .saturating_sub(failed_count);
    let adjusted_diff = foundation::numeric_cast::usize_to_i64_sat(adjusted_expected)
        - foundation::numeric_cast::usize_to_i64_sat(verify.actual);
    let (adjusted_passed, adjusted_message) = match adjusted_diff.cmp(&0) {
        core::cmp::Ordering::Equal => (
            true,
            format!(
                "{} Verification passed: {} files (ignored {} files, failed {} files excluded)",
                foundation::modern_ui::symbols::pick(
                    foundation::modern_ui::symbols::SUCCESS,
                    foundation::modern_ui::symbols::plain::SUCCESS
                ),
                verify.actual,
                ignored_count,
                failed_count
            ),
        ),
        core::cmp::Ordering::Greater => (
            false,
            format!(
                "{} Verification FAILED: missing {adjusted_diff} files after excluding {ignored_count} ignored and {failed_count} failed inputs (expected {adjusted_expected}, got {})",
                foundation::modern_ui::symbols::pick(
                    foundation::modern_ui::symbols::ERROR,
                    foundation::modern_ui::symbols::plain::ERROR
                ),
                verify.actual
            ),
        ),
        core::cmp::Ordering::Less => (
            true,
            format!(
                "{} Output has {} extra files after excluding {} ignored and {} failed inputs (expected {}, got {})",
                foundation::modern_ui::symbols::styled_warning_icon(),
                -adjusted_diff,
                ignored_count,
                failed_count,
                adjusted_expected,
                verify.actual
            ),
        ),
    };
    log_detail!(&adjusted_message);
    if !adjusted_passed {
        result.warn(&format!(
            "Output completeness check failed after excluding {ignored} ignored and {failed} failed inputs: expected {expected}, got {actual}",
            ignored = ignored_count,
            failed = failed_count,
            expected = adjusted_expected,
            actual = verify.actual
        ));
        foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
            "output_completeness_parity",
            "file count mismatch between input and output directories; some files may have been lost",
        );
        post_run_errors.push(format!(
            "Output completeness verification failed for {} -> {}: expected {adjusted_expected}, got {} after excluding {ignored_count} ignored and {failed_count} failed inputs",
            foundation::media_conversion_gate::base_dir_or_default(
                config.base_dir.as_deref(),
                "integrity_audit",
            )
            .display(),
            output_dir.display(),
            verify.actual
        ));
    }
}

fn auto_convert_directory_disk_space_precheck(
    input: &std::path::Path,
    config: &AutoConvertConfig,
    files: &[std::path::PathBuf],
) {
    if std::env::var("MFB_SKIP_DISK_PRECHECK").as_deref() != Ok("1") {
        let total_input_size: u64 = files
            .iter()
            .map(|f| match foundation::io_utils::metadata_with_retry(f) {
                Ok(metadata) => metadata.len(),
                Err(err) => {
                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                        "discovery_metadata",
                        f,
                        format!("failed to read file metadata for size sum: {err}"),
                    );
                    0
                }
            })
            .sum();
        let check_path = foundation::media_conversion_gate::disk_space_probe_path(
            config.output_dir.as_deref(),
            input,
        );
        if let Some(avail) = foundation::system_memory::get_available_disk_bytes(check_path) {
            // Reserve 1 GB headroom on top of total input size (temp files, partial encodes, etc.)
            let required = total_input_size.saturating_add(1024 * 1024 * 1024);
            if avail < required {
                let avail_gb =
                    foundation::numeric_cast::u64_to_f64(avail) / (1024.0 * 1024.0 * 1024.0);
                let required_gb =
                    foundation::numeric_cast::u64_to_f64(required) / (1024.0 * 1024.0 * 1024.0);
                foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                    "disk_space_insufficient",
                    format!(
                        "insufficient disk on output volume: available {avail_gb:.2} GB, required {required_gb:.2} GB (input + 1 GB headroom)"
                    ),
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
            if config.verbose() {
                log_detail!(&format!(
                    "{} Storage Audit: Output volume verified - {:.2} GB available, {:.2} GB required",
                    foundation::infra::static_logs::messages::LABEL_DISK,
                    foundation::numeric_cast::u64_to_f64(avail) / (1024.0 * 1024.0 * 1024.0),
                    foundation::numeric_cast::u64_to_f64(required) / (1024.0 * 1024.0 * 1024.0)
                ));
            }
        }
    }
}

#[cfg(test)]
mod fast_img_hardening_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::expect_fun_call,
        clippy::panic,
        clippy::assertions_on_constants
    )]
    use super::{
        ArchiveFlag, AutoImportFlag, Cli, Commands, DeleteSourceFlag, DryRunFlag,
        ExpertOptionsFlag, FastImgCleanupCompleteSourceState, FastImgInputPlan,
        FastImgPostGate1Policy, FastImgRunOptions, FastImgTranscodeError, FreshFlag, RecursiveFlag,
        RetryFlag, ShortestPathFlag, command_requires_database,
        fast_img_archive_stale_working_copy, fast_img_cleanup_complete_has_shortest_path_proof,
        fast_img_cleanup_complete_should_resume_shortest_path_import,
        fast_img_cleanup_complete_source_state, fast_img_cleanup_resume_source_subset_matches,
        fast_img_completed_marker_has_new_tier2_work, fast_img_container_is_static,
        fast_img_delete_notice_message, fast_img_delete_verified_source_jpegs_with,
        fast_img_effective_encode_parallelism, fast_img_effective_expected_count,
        fast_img_effective_verify_parallelism, fast_img_marker_entry_output_path,
        fast_img_marker_input_state_is_stale, fast_img_marker_outputs_current,
        fast_img_pipeline_ctx, fast_img_planned_output_rel, fast_img_post_gate1_policy,
        fast_img_prune_empty_source_dirs, fast_img_reconcile_unrecorded_source_disposition,
        fast_img_recover_non_directory_working_copy, fast_img_refresh_reused_jxl_delivery,
        fast_img_remove_failed_encode_output, fast_img_requires_resume_decision,
        fast_img_resolve_working_copy_for_run, fast_img_reuses_marker_import_proof_on_resume,
        fast_img_run_encode_phase, fast_img_skip_hashes_match, fast_img_source_hash_set,
        fast_img_strip_non_target_files, fast_img_validate_cleanup_retry_jxl_only_delivery_exit,
        fast_img_validate_jxl_only_delivery_exit, fast_img_validate_recorded_source_hashes_current,
        fast_img_verified_output_format, fast_img_verify_source_hash_unchanged,
        fast_static_modern_compression, restore_jpeg_build_current_proof_with_decoder,
        restore_jpeg_candidate_files, restore_jpeg_delete_verified_source,
        restore_jpeg_jxlinfo_has_jpeg_reconstruction, restore_jpeg_output_path_for,
        restore_jpeg_prune_empty_source_dirs, restore_jpeg_validate_disjoint_roots, run_fast_img,
        validate_cleanup_complete_marker, validate_fast_img_marker_source_state,
    };
    use anyhow::Context;
    use clap::Parser;
    use clap::error::ErrorKind;
    use foundation::ToolBuilder;
    use foundation::fast_img::{
        FastImgLibraryAssetProbe, IntegrityResult, apply_library_assets_to_marker, is_true_jpeg,
        library_handle_from_probes,
    };
    use foundation::image::format_detect::FormatKind;
    use foundation::pipeline::verification::{
        Blake3Entry, FastImgStageName, Gate1Checks, Gate2Checks, Gate2Import, Gate3Checks,
        Gate3Deep, LibraryAssetRecord, PipelineCtx, SkippedSourceEntry, VerificationGate,
        WorkingCopyMarker,
    };
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static FAST_IMG_TEST_ENV_LOCK: Mutex<()> = Mutex::new(());
    const MINIMAL_JXL_BYTES: &[u8] = &[0xFF, 0x0A, 0x00];

    struct TestEnvGuard {
        key: &'static str,
        old_value: Option<std::ffi::OsString>,
    }

    struct TestEnvPolicyGuard {
        _guards: Vec<TestEnvGuard>,
        // Fields drop in declaration order. Keep the lock alive until every
        // environment variable above has been restored.
        _lock: MutexGuard<'static, ()>,
    }

    impl TestEnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old_value = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, old_value }
        }

        fn set_os(key: &'static str, value: &std::ffi::OsStr) -> Self {
            let old_value = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, old_value }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            if let Some(old_value) = &self.old_value {
                unsafe { std::env::set_var(self.key, old_value) };
            } else {
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

    fn fast_img_test_policy(require_icloud_upload: bool) -> TestEnvPolicyGuard {
        let lock = foundation::media_conversion_gate::mutex_guard_or_recover(
            "fast_img_test_policy",
            FAST_IMG_TEST_ENV_LOCK.lock(),
        );
        let guards = vec![
            TestEnvGuard::set("MFB_FAST_IMG_ICLOUD_VERIFY_ATTEMPTS", "1"),
            TestEnvGuard::set("MFB_FAST_IMG_ICLOUD_VERIFY_DELAY_MS", "0"),
            TestEnvGuard::set(
                "MFB_FAST_IMG_REQUIRE_ICLOUD_UPLOAD_PROOF",
                if require_icloud_upload { "1" } else { "0" },
            ),
        ];
        TestEnvPolicyGuard {
            _guards: guards,
            _lock: lock,
        }
    }

    fn fast_img_single_query_icloud_test_policy() -> TestEnvPolicyGuard {
        fast_img_test_policy(true)
    }

    fn fast_img_single_query_local_test_policy() -> TestEnvPolicyGuard {
        fast_img_test_policy(false)
    }

    fn fast_img_marker_state_test_env(root: &std::path::Path) -> TestEnvPolicyGuard {
        let lock = foundation::media_conversion_gate::mutex_guard_or_recover(
            "fast_img_marker_state_test_env",
            FAST_IMG_TEST_ENV_LOCK.lock(),
        );
        let guards = vec![TestEnvGuard::set_os(
            foundation::constants::ENV_MFB_HOME_ROOT,
            root.as_os_str(),
        )];
        TestEnvPolicyGuard {
            _guards: guards,
            _lock: lock,
        }
    }

    fn write_jpeg(path: &std::path::Path, payload: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
        bytes.extend_from_slice(payload);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn write_avif_ftyp_stub(path: &std::path::Path) -> anyhow::Result<()> {
        let mut bytes = vec![
            0, 0, 0, 20, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f', 0, 0, 0, 0, b'a', b'v',
            b'i', b'f',
        ];
        bytes.resize(64, 0);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    #[test]
    fn fast_static_preflight_fails_closed_when_modern_compression_is_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("truncated.jpg");
        write_avif_ftyp_stub(&path).unwrap();
        let err = fast_static_modern_compression(
            &path,
            &foundation::image_detection::DetectedFormat::AVIF,
        )
        .expect_err("unknown modern compression must not fall through to re-encoding");
        assert!(
            err.to_string().contains("compression"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn fast_img_jpeg_selection_does_not_require_full_analysis() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("header-only.jpg");
        write_jpeg(&path, &[0x00])?;

        assert!(fast_img_container_is_static(
            &path,
            foundation::image::format_detect::FormatKind::Jpeg,
        )?);
        Ok(())
    }

    fn write_real_jpeg(path: &std::path::Path, rgb: [u8; 3]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let image = image::RgbImage::from_pixel(1, 1, image::Rgb(rgb));
        image.save_with_format(path, image::ImageFormat::Jpeg)?;
        Ok(())
    }

    fn write_jxl(path: &std::path::Path, payload: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = vec![
            0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ];
        bytes.extend_from_slice(payload);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn restore_jpeg_test_proof(
        source: &std::path::Path,
        input_root: &std::path::Path,
        output: &std::path::Path,
        output_root: &std::path::Path,
    ) -> anyhow::Result<super::RestoreJpegCommitProof> {
        restore_jpeg_build_current_proof_with_decoder(
            source,
            input_root,
            output,
            output_root,
            |_input, temp_output| {
                std::fs::copy(output, temp_output)?;
                Ok(())
            },
        )
    }

    #[test]
    fn restore_jpeg_command_accepts_output_and_recursive_flags() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from([
            "img",
            "restore-jpeg",
            "/photos/Album_optimized",
            "--output",
            "/photos/Album_restored_jpeg",
            "--recursive",
            "--keep-source",
        ])?;

        let Commands::RestoreJpeg {
            input,
            output,
            recursive,
            force,
            keep_source,
        } = parsed.command
        else {
            anyhow::bail!("expected restore-jpeg command");
        };
        assert_eq!(input, std::path::PathBuf::from("/photos/Album_optimized"));
        assert_eq!(
            output,
            Some(std::path::PathBuf::from("/photos/Album_restored_jpeg"))
        );
        assert!(recursive);
        assert!(!force);
        assert!(keep_source);
        Ok(())
    }

    #[test]
    fn restore_jpeg_command_does_not_require_database_preflight() {
        let command = Commands::RestoreJpeg {
            input: std::path::PathBuf::from("/photos/Album_optimized"),
            output: Some(std::path::PathBuf::from("/photos/Album_restored_jpeg")),
            recursive: true,
            force: false,
            keep_source: false,
        };

        assert!(!command_requires_database(&command));
    }

    #[test]
    fn restore_jpeg_jxlinfo_parser_requires_reconstruction_proof() {
        assert!(restore_jpeg_jxlinfo_has_jpeg_reconstruction(
            b"JPEG bitstream reconstruction data available\n"
        ));
        assert!(!restore_jpeg_jxlinfo_has_jpeg_reconstruction(
            b"JPEG bitstream reconstruction data not available\n"
        ));
    }

    #[test]
    fn restore_jpeg_output_cannot_be_inside_input_root() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let input_root = root.path().join("Album_optimized");
        std::fs::create_dir_all(&input_root)?;

        let Err(err) =
            restore_jpeg_validate_disjoint_roots(&input_root, &input_root.join("restored_jpeg"))
        else {
            anyhow::bail!("nested output root was accepted");
        };
        assert!(err.to_string().contains("disjoint input and output roots"));

        restore_jpeg_validate_disjoint_roots(
            &input_root,
            &root.path().join("Album_restored_jpeg"),
        )?;
        Ok(())
    }

    #[test]
    fn restore_jpeg_output_path_preserves_nested_folder_structure() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let input_root = root.path().join("Album_optimized");
        let output_root = root.path().join("Album_restored_jpeg");
        let source = input_root.join("nested/day1/photo.JXL");

        let output = restore_jpeg_output_path_for(&source, &input_root, &output_root)?;

        assert_eq!(output, output_root.join("nested/day1/photo.jpg"));
        Ok(())
    }

    #[test]
    fn restore_jpeg_file_input_requires_true_jxl_magic() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let disguised = root.path().join("fake.jxl");
        write_jpeg(&disguised, b"not jxl")?;

        let Err(err) = restore_jpeg_candidate_files(&disguised, false) else {
            anyhow::bail!("restore-jpeg accepted a disguised non-JXL file");
        };

        assert!(err.to_string().contains("not a true JXL"));
        Ok(())
    }

    #[test]
    fn restore_jpeg_directory_scan_uses_true_jxl_magic_not_extension() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let input_root = root.path().join("Album_optimized");
        let true_jxl = input_root.join("nested/no_extension");
        let fake_jxl = input_root.join("fake.jxl");
        write_jxl(&true_jxl, b"jxl")?;
        write_jpeg(&fake_jxl, b"jpeg")?;

        let files = restore_jpeg_candidate_files(&input_root, true)?;

        assert_eq!(files, vec![true_jxl]);
        Ok(())
    }

    #[test]
    fn restore_jpeg_cleanup_deletes_only_verified_source_jxl_and_xmp() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let input_root = root.path().join("Album_optimized");
        let output_root = root.path().join("Album_restored_jpeg");
        let source = input_root.join("nested/camera.JXL");
        let source_xmp = input_root.join("nested/camera.JXL.xmp");
        let unrelated_png = input_root.join("nested/keep.png");
        let unrelated_xmp = input_root.join("nested/keep.png.xmp");
        let output = output_root.join("nested/camera.jpg");
        write_jxl(&source, b"jxl-source")?;
        std::fs::write(&source_xmp, b"<x:xmpmeta/>")?;
        std::fs::write(&unrelated_png, b"\x89PNG\r\n\x1a\nnot-jxl")?;
        std::fs::write(&unrelated_xmp, b"<x:xmpmeta/>")?;
        write_real_jpeg(&output, [10, 20, 30])?;
        let proof = restore_jpeg_test_proof(&source, &input_root, &output, &output_root)?;

        let deleted = restore_jpeg_delete_verified_source(&proof)?;

        assert!(deleted);
        assert!(!source.exists());
        assert!(!source_xmp.exists());
        assert!(unrelated_png.exists());
        assert!(unrelated_xmp.exists());
        assert!(output.exists());
        Ok(())
    }

    #[test]
    fn restore_jpeg_cleanup_prunes_empty_source_dirs_but_keeps_root() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let input_root = root.path().join("redone");
        let output_root = root.path().join("redone_restored_jpeg");
        let source_dir = input_root.join("🌟来源/✨闲鱼");
        let source = source_dir.join("camera.jxl");
        let source_xmp = source_dir.join("camera.xmp");
        let ds_store = source_dir.join(".DS_Store");
        let output = output_root.join("🌟来源/✨闲鱼/camera.jpg");
        write_jxl(&source, b"jxl-source")?;
        std::fs::write(&source_xmp, b"<x:xmpmeta/>")?;
        std::fs::write(&ds_store, b"finder")?;
        write_real_jpeg(&output, [10, 20, 30])?;
        let proof = restore_jpeg_test_proof(&source, &input_root, &output, &output_root)?;

        assert!(restore_jpeg_delete_verified_source(&proof)?);
        let pruned = restore_jpeg_prune_empty_source_dirs(&input_root, &[source_dir]);

        assert_eq!(pruned, 2);
        assert!(input_root.exists());
        assert!(!input_root.join("🌟来源/✨闲鱼").exists());
        assert!(!input_root.join("🌟来源").exists());
        Ok(())
    }

    #[test]
    fn restore_jpeg_cleanup_refuses_missing_or_non_jpeg_output() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let input_root = root.path().join("Album_optimized");
        let output_root = root.path().join("Album_restored_jpeg");
        let source = input_root.join("camera.JXL");
        let source_xmp = input_root.join("camera.JXL.xmp");
        write_jxl(&source, b"jxl-source")?;
        std::fs::write(&source_xmp, b"<x:xmpmeta/>")?;
        let output = output_root.join("camera.jpg");
        write_real_jpeg(&output, [10, 20, 30])?;
        let proof = restore_jpeg_test_proof(&source, &input_root, &output, &output_root)?;
        std::fs::remove_file(&output)?;

        let Err(missing_err) = restore_jpeg_delete_verified_source(&proof) else {
            anyhow::bail!("missing restored JPEG did not block source deletion");
        };
        assert!(missing_err.to_string().contains("restored output missing"));
        assert!(source.exists());
        assert!(source_xmp.exists());

        std::fs::write(&output, b"\x89PNG\r\n\x1a\nnot-jpeg")?;

        let Err(format_err) = restore_jpeg_delete_verified_source(&proof) else {
            anyhow::bail!("non-JPEG restored output did not block source deletion");
        };
        assert!(
            format_err
                .to_string()
                .contains("restored output is not a true JPEG")
        );
        assert!(source.exists());
        assert!(source_xmp.exists());
        Ok(())
    }

    #[test]
    fn restore_jpeg_proof_accepts_metadata_rewritten_same_pixels() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let input_root = root.path().join("Album_optimized");
        let output_root = root.path().join("Album_restored_jpeg");
        let source = input_root.join("camera.JXL");
        let output = output_root.join("camera.jpg");
        write_jxl(&source, b"jxl-source")?;
        write_real_jpeg(&output, [10, 20, 30])?;

        let proof = restore_jpeg_build_current_proof_with_decoder(
            &source,
            &input_root,
            &output,
            &output_root,
            |_input, temp_output| {
                write_real_jpeg(temp_output, [10, 20, 30])?;
                Ok(())
            },
        )?;

        assert_eq!(
            proof.output_hash,
            foundation::common_utils::calculate_blake3_hash(&output)?
        );
        assert!(source.exists());
        assert!(output.exists());
        Ok(())
    }

    #[test]
    fn restore_jpeg_proof_refuses_output_that_differs_from_fresh_decode() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let input_root = root.path().join("Album_optimized");
        let output_root = root.path().join("Album_restored_jpeg");
        let source = input_root.join("camera.JXL");
        let output = output_root.join("camera.jpg");
        write_jxl(&source, b"jxl-source")?;
        write_real_jpeg(&output, [10, 20, 30])?;

        let Err(err) = restore_jpeg_build_current_proof_with_decoder(
            &source,
            &input_root,
            &output,
            &output_root,
            |_input, temp_output| {
                write_real_jpeg(temp_output, [200, 20, 30])?;
                Ok(())
            },
        ) else {
            anyhow::bail!("pixel-mismatched fresh decode did not block restore deletion proof");
        };

        assert!(
            err.to_string()
                .contains("restored JPEG pixels do not match fresh djxl decode"),
            "Expected 'restored JPEG pixels do not match fresh djxl decode', but got: {err:?}"
        );
        assert!(source.exists());
        assert!(output.exists());
        Ok(())
    }

    #[test]
    fn single_file_input_plan_uses_parent_root_and_filename_candidate() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src = root.path().join("one.jpg");
        write_jpeg(&src, b"one")?;

        let plan = FastImgInputPlan::from_input(&src, true)?;

        assert_eq!(plan.src_root, root.path().canonicalize()?);
        assert_eq!(plan.candidates, vec![src.canonicalize()?]);
        Ok(())
    }

    #[test]
    fn directory_input_plan_keeps_arbitrary_disguised_extensions_for_magic_filter()
    -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let jpg = root.path().join("one.jpg");
        let disguised_paths = [
            root.path().join("nested/disguised.mp4"),
            root.path().join("disguised.png"),
            root.path().join("disguised.heic"),
            root.path().join("disguised.txt"),
            root.path().join("no-extension"),
        ];
        let png = root.path().join("not-fastmode.png");
        write_real_jpeg(&jpg, [10, 20, 30])?;
        for disguised in &disguised_paths {
            write_real_jpeg(disguised, [40, 50, 60])?;
        }
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nnot jpeg")?;

        let plan = FastImgInputPlan::from_input(root.path(), true)?;
        let mut true_jpeg_rels = Vec::new();
        for path in &plan.candidates {
            if is_true_jpeg(path)? {
                true_jpeg_rels.push(
                    path.strip_prefix(&plan.src_root)?
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
        true_jpeg_rels.sort();

        assert_eq!(
            true_jpeg_rels,
            vec![
                "disguised.heic".to_string(),
                "disguised.png".to_string(),
                "disguised.txt".to_string(),
                "nested/disguised.mp4".to_string(),
                "no-extension".to_string(),
                "one.jpg".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn cleanup_marker_rejects_same_count_source_drift() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"old")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: "out".to_string(),
                library_asset: None,
            },
        );
        write_jpeg(&src, b"new")?;
        let current_hashes = fast_img_source_hash_set(&src_root, &[src])?;

        let state = fast_img_cleanup_complete_source_state(&marker, 1, &current_hashes)?;
        assert_eq!(state, FastImgCleanupCompleteSourceState::StaleCurrent);

        let Err(err) = validate_cleanup_complete_marker(&marker, &src_root, 1, &current_hashes)
        else {
            anyhow::bail!("cleanup marker unexpectedly accepted drifted source");
        };

        assert!(
            err.to_string()
                .contains("source count mismatch after converted-source deletion"),
            "unexpected: {err}"
        );
        Ok(())
    }

    #[test]
    fn cleanup_marker_with_restored_sources_restarts_dry_run() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_root_raw = root.path().join("Photos");
        let src = src_root_raw.join("a.jpg");
        write_jpeg(&src, b"restored original")?;
        let src_root = src_root_raw.canonicalize()?;
        let src = src_root.join("a.jpg");
        let wc = foundation::pipeline::verification::resolve_working_copy_dir(&src_root);
        std::fs::create_dir_all(&wc)?;
        let out = wc.join("a.JXL");
        std::fs::write(&out, b"old jxl output")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.encoded_count = 1;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );
        foundation::pipeline::verification::write_marker_atomic(&marker)?;

        run_fast_img(FastImgRunOptions {
            input: &src_root,
            output_dir: None,
            delete_source: DeleteSourceFlag(false),
            dry_run: DryRunFlag(true),
            recursive: RecursiveFlag(true),
            auto_import: AutoImportFlag(false),
            shortest_path: ShortestPathFlag(false),
            retry: RetryFlag(false),
            fresh: FreshFlag(false),
            archive: false,
            allow_expert_options: false,
            strategy: "jxl",
            extreme_precision: false,
        })?;

        Ok(())
    }

    #[test]
    fn failed_fast_img_job_can_be_recorded_as_failed_source() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let src = src_root.join("bad.jpg");
        write_jpeg(&src, b"bad")?;
        let src_hash = foundation::common_utils::calculate_blake3_hash(&src)?;
        let err = FastImgTranscodeError {
            rel_key: "bad.jpg".to_string(),
            out_rel_key: "bad.JXL".to_string(),
            src_hash: src_hash.clone(),
            reason: "pixel-diff: djxl exited non-zero decoding output.JXL".to_string(),
        };
        let mut marker = WorkingCopyMarker::new(src_root, root.path().join("Photos_optimized"), 1);

        marker.blake3_log.remove(&err.rel_key);
        marker.failed_sources.insert(
            err.rel_key,
            SkippedSourceEntry {
                src: err.src_hash,
                reason: err.reason,
            },
        );

        let skipped = marker
            .failed_sources
            .get("bad.jpg")
            .context("failed source was not recorded")?;
        assert_eq!(skipped.src, src_hash);
        assert!(skipped.reason.contains("djxl exited non-zero"));
        assert!(marker.blake3_log.is_empty());
        Ok(())
    }

    #[test]
    fn failed_fast_img_job_removes_partial_jxl_output() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("Photos_optimized");
        std::fs::create_dir_all(&wc)?;
        let out = wc.join("bad.JXL");
        std::fs::write(&out, b"partial-jxl")?;
        let err = FastImgTranscodeError {
            rel_key: "bad.jpg".to_string(),
            out_rel_key: "bad.JXL".to_string(),
            src_hash: "source-hash".to_string(),
            reason: "pixel-diff: djxl exited non-zero decoding bad.JXL".to_string(),
        };

        fast_img_remove_failed_encode_output(&wc, &err)?;

        assert!(!out.exists());
        Ok(())
    }

    #[test]
    fn failed_fast_img_job_propagates_output_cleanup_failure() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("Photos_optimized");
        let out = wc.join("bad.JXL");
        std::fs::create_dir_all(&out)?;
        let err = FastImgTranscodeError {
            rel_key: "bad.jpg".to_string(),
            out_rel_key: "bad.JXL".to_string(),
            src_hash: "source-hash".to_string(),
            reason: "pixel-diff: djxl exited non-zero decoding bad.JXL".to_string(),
        };

        let cleanup_error = fast_img_remove_failed_encode_output(&wc, &err)
            .expect_err("an undeletable failed output must stop before Gate 1");

        assert!(cleanup_error.to_string().contains("bad.JXL"));
        assert!(out.is_dir(), "failed cleanup target must remain observable");
        Ok(())
    }

    #[test]
    fn cleanup_retry_delivery_accepts_previous_successes_plus_retried_sources() -> anyhow::Result<()>
    {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        std::fs::create_dir_all(&src_root)?;
        std::fs::create_dir_all(&wc)?;
        let failed_src = src_root.join("failed.jpg");
        write_jpeg(&failed_src, b"failed source")?;
        let old_out = wc.join("old.JXL");
        std::fs::write(&old_out, b"old output")?;
        let retried_out = wc.join("failed.JXL");
        std::fs::write(&retried_out, b"retried output")?;

        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.blake3_log.insert(
            "old.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("old.JXL".to_string()),
                src: "old-source-hash".to_string(),
                out: foundation::common_utils::calculate_blake3_hash(&old_out)?,
                library_asset: None,
            },
        );
        marker.blake3_log.insert(
            "failed.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("failed.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&failed_src)?,
                out: foundation::common_utils::calculate_blake3_hash(&retried_out)?,
                library_asset: None,
            },
        );
        let current_hashes = fast_img_source_hash_set(&src_root, &[failed_src])?;

        fast_img_validate_cleanup_retry_jxl_only_delivery_exit(&marker, 1, &current_hashes, "jxl")?;

        Ok(())
    }

    #[test]
    fn cleanup_marker_with_changed_restored_sources_restarts_dry_run() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_root_raw = root.path().join("Photos");
        let stale_src = src_root_raw.join("old.jpg");
        write_jpeg(&stale_src, b"old completed source")?;
        let src_root = src_root_raw.canonicalize()?;
        let stale_src = src_root.join("old.jpg");
        let wc = foundation::pipeline::verification::resolve_working_copy_dir(&src_root);
        std::fs::create_dir_all(&wc)?;
        let out = wc.join("old.JXL");
        std::fs::write(&out, b"old jxl output")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.encoded_count = 1;
        marker.blake3_log.insert(
            "old.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("old.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&stale_src)?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );
        foundation::pipeline::verification::write_marker_atomic(&marker)?;
        std::fs::remove_file(&stale_src)?;
        write_jpeg(&src_root.join("new_a.jpg"), b"new source a")?;
        write_jpeg(&src_root.join("new_b.jpg"), b"new source b")?;

        run_fast_img(FastImgRunOptions {
            input: &src_root,
            output_dir: None,
            delete_source: DeleteSourceFlag(false),
            dry_run: DryRunFlag(true),
            recursive: RecursiveFlag(true),
            auto_import: AutoImportFlag(false),
            shortest_path: ShortestPathFlag(false),
            retry: RetryFlag(false),
            fresh: FreshFlag(false),
            archive: false,
            allow_expert_options: false,
            strategy: "jxl",
            extreme_precision: false,
        })?;

        Ok(())
    }

    #[test]
    fn reused_fast_img_jxl_refresh_replays_metadata_and_returns_new_hash() -> anyhow::Result<()> {
        if !foundation::ExiftoolBuilder::check_available()
            || !foundation::common_utils::is_command_available(foundation::constants::TOOL_CJXL)
            || !foundation::common_utils::is_command_available(foundation::constants::TOOL_DJXL)
            || !foundation::MagickBuilder::check_available()
        {
            return Ok(());
        }

        let root = TempDir::new()?;
        let src = root.path().join("a.jpg");
        let out = root.path().join("a.JXL");
        let magick = std::process::Command::new(
            foundation::media_conversion_gate::delivery_imagemagick_cli_path_or_default(),
        )
        .arg("-size")
        .arg("3x2")
        .arg("gradient:")
        .arg(&src)
        .output()
        .context("create source JPEG")?;
        assert!(
            magick.status.success(),
            "create source JPEG failed: stdout={} stderr={}",
            String::from_utf8_lossy(&magick.stdout),
            String::from_utf8_lossy(&magick.stderr)
        );
        let orient = std::process::Command::new(foundation::constants::TOOL_EXIFTOOL)
            .arg("-overwrite_original")
            .arg("-Orientation#=6")
            .arg(&src)
            .output()
            .context("write source orientation")?;
        assert!(
            orient.status.success(),
            "write source orientation failed: stdout={} stderr={}",
            String::from_utf8_lossy(&orient.stdout),
            String::from_utf8_lossy(&orient.stderr)
        );
        let cjxl = std::process::Command::new(foundation::constants::TOOL_CJXL)
            .arg(&src)
            .arg(&out)
            .arg("--lossless_jpeg=1")
            .arg("--effort=7")
            .output()
            .context("encode source JXL")?;
        assert!(
            cjxl.status.success(),
            "encode source JXL failed: stdout={} stderr={}",
            String::from_utf8_lossy(&cjxl.stdout),
            String::from_utf8_lossy(&cjxl.stderr)
        );
        let stale_orientation = std::process::Command::new(foundation::constants::TOOL_EXIFTOOL)
            .arg("-overwrite_original")
            .arg("-IFD1:Orientation#=1")
            .arg(&out)
            .output()
            .context("write stale JXL thumbnail orientation")?;
        assert!(
            stale_orientation.status.success(),
            "write stale JXL orientation failed: stdout={} stderr={}",
            String::from_utf8_lossy(&stale_orientation.stdout),
            String::from_utf8_lossy(&stale_orientation.stderr)
        );
        let stale_hash = foundation::common_utils::calculate_blake3_hash(&out)?;

        let refreshed_hash = fast_img_refresh_reused_jxl_delivery(&src, &out)?;

        assert_ne!(
            stale_hash, refreshed_hash,
            "metadata refresh must update reused JXL hash proof after Orientation cleanup"
        );
        let has_orientation = std::process::Command::new(foundation::constants::TOOL_EXIFTOOL)
            .arg("-s3")
            .arg("-Orientation")
            .arg(&out)
            .output()
            .context("probe refreshed JXL orientation")?;
        assert!(
            has_orientation.status.success(),
            "orientation probe failed: {}",
            String::from_utf8_lossy(&has_orientation.stderr)
        );
        assert!(
            has_orientation.stdout.is_empty(),
            "reused JXL refresh must strip residual Orientation: {}",
            String::from_utf8_lossy(&has_orientation.stdout)
        );
        Ok(())
    }

    #[test]
    fn marker_refresh_updates_output_hash_and_clears_import_proof() -> anyhow::Result<()> {
        if !foundation::ExiftoolBuilder::check_available()
            || !foundation::common_utils::is_command_available(foundation::constants::TOOL_CJXL)
            || !foundation::common_utils::is_command_available(foundation::constants::TOOL_DJXL)
            || !foundation::MagickBuilder::check_available()
        {
            return Ok(());
        }
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        let out = wc.join("a.JXL");
        std::fs::create_dir_all(&src_root)?;
        std::fs::create_dir_all(&wc)?;
        let magick = std::process::Command::new(
            foundation::media_conversion_gate::delivery_imagemagick_cli_path_or_default(),
        )
        .arg("-size")
        .arg("3x2")
        .arg("gradient:")
        .arg(&src)
        .output()
        .context("create source JPEG")?;
        assert!(
            magick.status.success(),
            "create source JPEG failed: stdout={} stderr={}",
            String::from_utf8_lossy(&magick.stdout),
            String::from_utf8_lossy(&magick.stderr)
        );
        let cjxl = std::process::Command::new(foundation::constants::TOOL_CJXL)
            .arg(&src)
            .arg(&out)
            .arg("--lossless_jpeg=1")
            .arg("--effort=7")
            .output()
            .context("encode source JXL")?;
        assert!(
            cjxl.status.success(),
            "encode source JXL failed: stdout={} stderr={}",
            String::from_utf8_lossy(&cjxl.stdout),
            String::from_utf8_lossy(&cjxl.stderr)
        );
        let metadata = std::process::Command::new(foundation::constants::TOOL_EXIFTOOL)
            .arg("-overwrite_original")
            .arg("-EXIF:UserComment=marker refresh")
            .arg(&src)
            .output()
            .context("add source metadata after JXL encode")?;
        assert!(
            metadata.status.success(),
            "add source metadata failed: stdout={} stderr={}",
            String::from_utf8_lossy(&metadata.stdout),
            String::from_utf8_lossy(&metadata.stderr)
        );
        let old_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::TranscodeComplete;
        marker.gate1_checks.count = foundation::pipeline::verification::CheckPassed(true);
        marker.gate2_checks.count = foundation::pipeline::verification::CheckPassed(true);
        marker.gate3_checks.count_x3 = foundation::pipeline::verification::CheckPassed(true);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: old_hash.clone(),
                library_asset: Some(old_hash.clone()),
            },
        );

        let summary = super::fast_img_refresh_and_persist_marker_deliveries(
            &mut marker,
            &src_root,
            "hevc",
            true,
        )?;
        let persisted = super::read_marker(&marker.working_copy)?;
        let entry = persisted
            .blake3_log
            .get("a.jpg")
            .ok_or_else(|| anyhow::anyhow!("missing refreshed marker entry"))?;

        assert_eq!(summary.refreshed, 1);
        assert_eq!(summary.invalidated, 0);
        assert!(summary.marker_changed);
        assert_ne!(entry.out, old_hash);
        assert_eq!(entry.library_asset, None);
        assert_eq!(persisted.stage, FastImgStageName::TranscodeComplete);
        assert_eq!(persisted.gate1_checks, Gate1Checks::default());
        assert_eq!(persisted.gate2_checks, Gate2Checks::default());
        assert_eq!(persisted.gate3_checks, Gate3Checks::default());
        Ok(())
    }

    #[test]
    fn resume_reused_fast_img_output_keeps_recorded_collision_path() -> anyhow::Result<()> {
        if !foundation::ExiftoolBuilder::check_available()
            || !foundation::common_utils::is_command_available(foundation::constants::TOOL_CJXL)
        {
            return Ok(());
        }
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("a.jpg");
        let out = wc.join("a.JXL");
        std::fs::create_dir_all(&src_root)?;
        std::fs::create_dir_all(&wc)?;
        write_real_jpeg(&src, [10, 20, 30])?;
        let cjxl = std::process::Command::new(foundation::constants::TOOL_CJXL)
            .arg(&src)
            .arg(&out)
            .arg("--lossless_jpeg=1")
            .arg("--effort=7")
            .output()
            .context("encode source JXL")?;
        assert!(
            cjxl.status.success(),
            "encode source JXL failed: stdout={} stderr={}",
            String::from_utf8_lossy(&cjxl.stdout),
            String::from_utf8_lossy(&cjxl.stderr)
        );
        let current_source_hashes =
            fast_img_source_hash_set(&src_root, std::slice::from_ref(&src))?;
        let out_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc.clone(), 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: current_source_hashes
                    .get("a.jpg")
                    .cloned()
                    .context("missing source hash")?,
                out: out_hash.clone(),
                library_asset: Some(out_hash),
            },
        );

        fast_img_run_encode_phase(
            &mut marker,
            std::slice::from_ref(&src),
            &current_source_hashes,
            &BTreeMap::new(),
            &src_root,
            &wc,
            RetryFlag(false),
            ArchiveFlag(false),
            ExpertOptionsFlag(false),
            "jxl",
        )?;

        let entry = marker
            .blake3_log
            .get("a.jpg")
            .context("missing reused marker entry")?;
        assert_eq!(entry.out_rel.as_deref(), Some("a.JXL"));
        assert!(
            !wc.join("a (1).JXL").exists(),
            "resume reuse must not move proof to a fresh collision reservation"
        );
        Ok(())
    }

    /// Regression test: when a marker entry has `out_rel = "a.JXL"` but the
    /// source hash changed (stale proof), `fast_img_planned_output_rel` must
    /// honour the marker's recorded output path via `reserve_output_path`
    /// instead of treating the on-disk `a.JXL` as a foreign collision and
    /// producing `a (1).JXL`.
    #[test]
    fn stale_proof_reencode_keeps_marker_out_rel_path() -> anyhow::Result<()> {
        if !foundation::ExiftoolBuilder::check_available()
            || !foundation::common_utils::is_command_available(foundation::constants::TOOL_CJXL)
        {
            return Ok(());
        }

        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("a.jpg");
        let out = wc.join("a.JXL");
        std::fs::create_dir_all(&src_root)?;
        std::fs::create_dir_all(&wc)?;

        // Write a real JPEG so cjxl can encode it
        write_real_jpeg(&src, [10, 20, 30])?;

        // Produce the initial JXL (simulates a previous run that wrote a.JXL)
        let cjxl = std::process::Command::new(foundation::constants::TOOL_CJXL)
            .arg(&src)
            .arg(&out)
            .arg("--lossless_jpeg=1")
            .arg("--effort=7")
            .output()
            .context("encode source JXL")?;
        assert!(
            cjxl.status.success(),
            "encode source JXL failed: {}",
            String::from_utf8_lossy(&cjxl.stderr)
        );

        // Compute the *current* source hash (needed for the encode phase scan)
        let current_source_hashes =
            fast_img_source_hash_set(&src_root, std::slice::from_ref(&src))?;

        // Build a marker with a STALE source hash so hashes won't match,
        // forcing existing_output_current = false → re-encode branch
        let stale_src_hash =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        let stale_out_hash =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc.clone(), 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()), // marker already records a.JXL
                src: stale_src_hash,
                out: stale_out_hash,
                library_asset: None,
            },
        );

        // Run the encode phase — it will detect stale proof, re-encode,
        // and must write back to a.JXL (not a (1).JXL)
        fast_img_run_encode_phase(
            &mut marker,
            std::slice::from_ref(&src),
            &current_source_hashes,
            &BTreeMap::new(),
            &src_root,
            &wc,
            RetryFlag(false),
            ArchiveFlag(false),
            ExpertOptionsFlag(false),
            "jxl",
        )?;

        let entry = marker
            .blake3_log
            .get("a.jpg")
            .context("missing marker entry after reencode")?;

        assert_eq!(
            entry.out_rel.as_deref(),
            Some("a.JXL"),
            "stale-proof reencode must keep the marker's recorded out_rel"
        );
        assert!(
            !wc.join("a (1).JXL").exists(),
            "stale-proof reencode must not produce a spurious collision path"
        );
        Ok(())
    }

    #[test]
    fn fast_img_encode_options_force_overwrite_stale_outputs() -> anyhow::Result<()> {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
        )?;
        let options_pos = source
            .find("let options = LosslessConvertOptions")
            .ok_or_else(|| anyhow::anyhow!("fast-img encode options must exist"))?;
        let options_block = &source[options_pos..];
        let convert_pos = options_block
            .find("convert_jpeg_to_jxl")
            .ok_or_else(|| anyhow::anyhow!("fast-img JXL conversion call must exist"))?;
        let options_block = &options_block[..convert_pos];
        let force_pos = options_block
            .find("LosslessConvertFlags::FORCE")
            .ok_or_else(|| {
                anyhow::anyhow!("fast-img encode must force overwrite stale JXL outputs")
            })?;
        let require_output_delivery_pos = options_block
            .find("LosslessConvertFlags::REQUIRE_OUTPUT_DELIVERY")
            .ok_or_else(|| anyhow::anyhow!("fast-img encode must require output delivery"))?;
        let apple_compat_pos = options_block
            .find("LosslessConvertFlags::APPLE_COMPAT")
            .ok_or_else(|| {
                anyhow::anyhow!("fast-img JXL encode must enable Apple-compatible box layout")
            })?;

        assert!(
            force_pos < require_output_delivery_pos,
            "fast-img queued encodes must overwrite stale/corrupt JXL siblings before delivery checks"
        );
        assert!(
            apple_compat_pos < require_output_delivery_pos,
            "fast-img JXL encodes must enable Apple compatibility before delivery validation"
        );
        Ok(())
    }

    #[test]
    fn retry_marker_rejects_source_drift_before_resume() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"old")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::Gate1Failed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: "out".to_string(),
                library_asset: None,
            },
        );
        write_jpeg(&src, b"new")?;
        let current_hashes = fast_img_source_hash_set(&src_root, &[src])?;

        let Err(err) =
            validate_fast_img_marker_source_state(&marker, &src_root, 1, &current_hashes)
        else {
            anyhow::bail!("retry marker unexpectedly accepted drifted source");
        };

        assert!(err.to_string().contains("source hash set changed"));
        Ok(())
    }

    #[test]
    fn stale_fast_img_marker_is_archived_before_fresh_run() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_root_raw = root.path().join("Photos");
        std::fs::create_dir_all(&src_root_raw)?;
        let stale_src = src_root_raw.join("old.jpg");
        write_jpeg(&stale_src, b"old completed source")?;
        let src_root = src_root_raw.canonicalize()?;
        let stale_src = src_root.join("old.jpg");
        let wc = foundation::pipeline::verification::resolve_working_copy_dir(&src_root);
        std::fs::create_dir_all(&wc)?;
        let out = wc.join("old.JXL");
        std::fs::write(&out, b"old jxl output")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc.clone(), 2);
        marker.stage = FastImgStageName::Gate1Failed;
        marker.encoded_count = 1;
        marker.blake3_log.insert(
            "old.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("old.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&stale_src)?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );
        foundation::pipeline::verification::write_marker_atomic(&marker)?;
        std::fs::remove_file(&stale_src)?;
        write_jpeg(&src_root.join("new.jpg"), b"new source")?;

        let current_hashes = fast_img_source_hash_set(&src_root, &[src_root.join("new.jpg")])?;
        assert!(fast_img_marker_input_state_is_stale(
            &marker,
            &src_root,
            1,
            &current_hashes,
            "jxl",
        )?);

        let archived = fast_img_archive_stale_working_copy(&wc)?
            .ok_or_else(|| anyhow::anyhow!("test precondition: stale output exists"))?;
        assert!(!wc.exists());
        assert!(archived.join("old.JXL").is_file());

        Ok(())
    }

    #[test]
    fn stale_fast_img_file_is_archived_before_fresh_run() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let working_copy = root.path().join("Photos_optimized");
        std::fs::write(&working_copy, b"interrupted output placeholder")?;

        let archived = fast_img_archive_stale_working_copy(&working_copy)?
            .ok_or_else(|| anyhow::anyhow!("test precondition: stale output exists"))?;

        assert!(std::fs::symlink_metadata(&working_copy).is_err());
        assert_eq!(std::fs::read(&archived)?, b"interrupted output placeholder");
        Ok(())
    }

    #[test]
    fn non_directory_fast_img_working_copy_is_recovered_before_marker_read() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let working_copy = root.path().join("Photos_optimized");
        std::fs::write(&working_copy, b"interrupted output placeholder")?;

        fast_img_recover_non_directory_working_copy(&working_copy, DryRunFlag(false))?;

        assert!(std::fs::symlink_metadata(&working_copy).is_err());
        let archived = std::fs::read_dir(root.path())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|name| name.starts_with("Photos_optimized.stale-"))
            })
            .context("recovery did not archive the stale non-directory output")?;
        assert_eq!(std::fs::read(archived)?, b"interrupted output placeholder");
        Ok(())
    }

    #[test]
    fn non_directory_preferred_fast_img_output_is_archived_and_reused() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let source_dir = root.path().join("Photos");
        std::fs::create_dir(&source_dir)?;
        let working_copy = foundation::pipeline::verification::working_copy_dir(&source_dir);
        std::fs::write(&working_copy, b"interrupted output placeholder")?;

        let resolved = fast_img_resolve_working_copy_for_run(&source_dir, DryRunFlag(false))?;

        assert_eq!(resolved, working_copy);
        assert!(std::fs::symlink_metadata(&working_copy).is_err());
        let archived = std::fs::read_dir(root.path())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|name| name.starts_with("Photos_optimized.stale-"))
            })
            .context("resolver did not archive the stale preferred output")?;
        assert_eq!(std::fs::read(archived)?, b"interrupted output placeholder");
        Ok(())
    }

    #[test]
    fn non_directory_fast_img_working_copy_dry_run_does_not_mutate() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let working_copy = root.path().join("Photos_optimized");
        std::fs::write(&working_copy, b"interrupted output placeholder")?;

        fast_img_recover_non_directory_working_copy(&working_copy, DryRunFlag(true))?;

        assert_eq!(
            std::fs::read(&working_copy)?,
            b"interrupted output placeholder"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stale_fast_img_dangling_symlink_is_archived_before_fresh_run() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let working_copy = root.path().join("Photos_optimized");
        std::os::unix::fs::symlink(root.path().join("missing-output"), &working_copy)?;

        let archived = fast_img_archive_stale_working_copy(&working_copy)?
            .ok_or_else(|| anyhow::anyhow!("test precondition: stale output exists"))?;

        assert!(std::fs::symlink_metadata(&working_copy).is_err());
        assert!(
            std::fs::symlink_metadata(&archived)?
                .file_type()
                .is_symlink()
        );
        Ok(())
    }

    #[test]
    fn missing_stale_fast_img_working_copy_rebuilds_without_archive() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let working_copy = root.path().join("Photos_optimized");

        assert!(fast_img_archive_stale_working_copy(&working_copy)?.is_none());
        Ok(())
    }

    #[test]
    fn retry_marker_stale_source_count_is_detected() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"current")?;
        let marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        let current_hashes = fast_img_source_hash_set(&src_root, &[src])?;
        assert!(fast_img_marker_input_state_is_stale(
            &marker,
            &src_root,
            1,
            &current_hashes,
            "jxl",
        )?);
        Ok(())
    }

    #[test]
    fn retry_marker_same_count_hash_drift_still_fails_closed() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"old")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::Gate1Failed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: "out".to_string(),
                library_asset: None,
            },
        );
        write_jpeg(&src, b"new")?;
        let current_hashes = fast_img_source_hash_set(&src_root, &[src])?;

        assert!(fast_img_marker_input_state_is_stale(
            &marker,
            &src_root,
            1,
            &current_hashes,
            "jxl",
        )?);
        let Err(err) =
            validate_fast_img_marker_source_state(&marker, &src_root, 1, &current_hashes)
        else {
            anyhow::bail!("retry marker unexpectedly accepted same-count source drift");
        };
        assert!(err.to_string().contains("source hash set changed"));
        Ok(())
    }

    #[test]
    fn retry_marker_strategy_change_rebuilds_from_current_sources() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"current")?;
        let marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        let current_hashes = fast_img_source_hash_set(&src_root, &[src])?;

        assert!(fast_img_marker_input_state_is_stale(
            &marker,
            &src_root,
            1,
            &current_hashes,
            "avif",
        )?);
        Ok(())
    }

    #[test]
    fn output_prepared_partial_marker_accepts_unchanged_sources_for_resume() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let logged_src = src_root.join("a.jpg");
        let pending_src = src_root.join("b.jpg");
        write_jpeg(&logged_src, b"logged")?;
        write_jpeg(&pending_src, b"pending")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::OutputPrepared;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&logged_src)?,
                out: "partial-out".to_string(),
                library_asset: None,
            },
        );
        let current_hashes = fast_img_source_hash_set(&src_root, &[logged_src, pending_src])?;

        validate_fast_img_marker_source_state(&marker, &src_root, 2, &current_hashes)?;
        Ok(())
    }

    #[test]
    fn output_prepared_empty_log_accepts_pre_encode_resume() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let first = src_root.join("a.jpg");
        let second = src_root.join("b.jpg");
        write_jpeg(&first, b"first")?;
        write_jpeg(&second, b"second")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::OutputPrepared;
        let current_hashes = fast_img_source_hash_set(&src_root, &[first, second])?;

        validate_fast_img_marker_source_state(&marker, &src_root, 2, &current_hashes)?;
        Ok(())
    }

    #[test]
    fn cleanup_marker_accepts_deleted_sources_when_outputs_match() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&src, b"old")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::write(&out, b"jxl-output")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );
        std::fs::remove_file(&src)?;
        let current_hashes = BTreeMap::new();

        validate_cleanup_complete_marker(&marker, &src_root, 0, &current_hashes)?;
        Ok(())
    }

    #[test]
    fn verified_source_deletion_removes_matching_xmp_sidecar() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("discord/a.jpg");
        let xmp = src.with_extension("xmp");
        let out = wc.join("discord/a.JXL");
        write_jpeg(&src, b"source")?;
        std::fs::write(&xmp, b"<x:xmpmeta/>")?;
        std::fs::create_dir_all(
            out.parent()
                .ok_or_else(|| anyhow::anyhow!("missing parent"))?,
        )?;
        std::fs::write(&out, b"jxl-output")?;
        let source_hash = foundation::common_utils::calculate_blake3_hash(&src)?;
        let output_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "discord/a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("discord/a.JXL".to_string()),
                src: source_hash.clone(),
                out: output_hash.clone(),
                library_asset: None,
            },
        );

        let (deleted, already_deleted) =
            fast_img_delete_verified_source_jpegs_with(&marker, &src_root, |_source, _output| {
                Ok(IntegrityResult::FinalModernDelivery {
                    source_hash: source_hash.clone(),
                    output_hash: output_hash.clone(),
                })
            })?;

        assert_eq!((deleted, already_deleted), (1, 0));
        assert!(!src.exists());
        assert!(!xmp.exists());
        Ok(())
    }

    #[test]
    fn verified_source_deletion_prunes_empty_source_subdirs() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let empty_leaf = src_root.join("only_jpeg/day1");
        let keep_leaf = src_root.join("mixed/day2");
        let unrelated_empty = src_root.join("manual_empty/keep");
        std::fs::create_dir_all(&empty_leaf)?;
        std::fs::create_dir_all(&keep_leaf)?;
        std::fs::create_dir_all(&unrelated_empty)?;
        std::fs::write(keep_leaf.join("keep.png"), b"png")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), root.path().join("out"), 1);
        marker.blake3_log.insert(
            "only_jpeg/day1/a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("only_jpeg/day1/a.JXL".to_string()),
                src: "source-hash".to_string(),
                out: "output-hash".to_string(),
                library_asset: None,
            },
        );

        let pruned = fast_img_prune_empty_source_dirs(&marker, &src_root)?;

        assert_eq!(pruned, 2);
        assert!(!empty_leaf.exists());
        assert!(!src_root.join("only_jpeg").exists());
        assert!(unrelated_empty.exists());
        assert!(keep_leaf.exists());
        assert!(src_root.exists());
        Ok(())
    }

    #[test]
    fn fastmode_verify_parallelism_is_bounded_for_djxl_gate() {
        assert_eq!(fast_img_effective_verify_parallelism(0, 8), 1);
        assert_eq!(fast_img_effective_verify_parallelism(2, 8), 2);
        assert_eq!(fast_img_effective_verify_parallelism(99, 32), 4);
    }

    #[test]
    fn verified_absent_source_cleanup_removes_matching_xmp_sidecar() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("discord/a.jpg");
        let xmp = src.with_extension("xmp");
        let out = wc.join("discord/a.JXL");
        write_jpeg(&src, b"source")?;
        let source_hash = foundation::common_utils::calculate_blake3_hash(&src)?;
        std::fs::write(&xmp, b"<x:xmpmeta/>")?;
        std::fs::create_dir_all(
            out.parent()
                .ok_or_else(|| anyhow::anyhow!("missing parent"))?,
        )?;
        std::fs::write(&out, b"jxl-output")?;
        let output_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        std::fs::remove_file(&src)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "discord/a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("discord/a.JXL".to_string()),
                src: source_hash,
                out: output_hash,
                library_asset: None,
            },
        );

        let (deleted, already_deleted) =
            fast_img_delete_verified_source_jpegs_with(&marker, &src_root, |_source, _output| {
                anyhow::bail!("absent source must not request fresh source integrity")
            })?;

        assert_eq!((deleted, already_deleted), (0, 1));
        assert!(!xmp.exists());
        Ok(())
    }

    #[test]
    fn cleanup_marker_local_delivery_resumes_for_shortest_path_import() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let mut marker = WorkingCopyMarker::new(src_root, wc, 2);
        marker.stage = FastImgStageName::CleanupComplete;

        assert!(
            fast_img_cleanup_complete_should_resume_shortest_path_import(
                &marker,
                ShortestPathFlag(true)
            )
        );
        assert!(
            !fast_img_cleanup_complete_should_resume_shortest_path_import(
                &marker,
                ShortestPathFlag(false)
            )
        );
        assert_eq!(fast_img_effective_expected_count(&marker, 0, true), 2);
        Ok(())
    }

    #[test]
    fn cleanup_marker_completed_shortest_path_does_not_reimport() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.gate2_checks.count.0 = true;
        marker.gate2_checks.blake3_sample.0 = true;
        marker.gate2_checks.no_error.0 = true;
        marker.gate3_checks.count_x3.0 = true;
        marker.gate3_checks.sync.0 = true;
        marker.gate3_checks.quarantine.0 = true;
        marker.gate3_checks.chain.0 = true;

        assert!(fast_img_cleanup_complete_has_shortest_path_proof(&marker));
        assert!(
            !fast_img_cleanup_complete_should_resume_shortest_path_import(
                &marker,
                ShortestPathFlag(true)
            )
        );
        Ok(())
    }

    #[test]
    fn cleanup_marker_missing_output_reports_restore_action() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"old")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: "missing-output-hash".to_string(),
                library_asset: None,
            },
        );
        std::fs::remove_file(&src)?;
        let current_hashes = BTreeMap::new();

        let Err(err) = validate_cleanup_complete_marker(&marker, &src_root, 0, &current_hashes)
        else {
            anyhow::bail!("cleanup marker unexpectedly accepted a missing output");
        };

        let message = format!("{err:#}");
        assert!(message.contains("restore this optimized JXL"));
        Ok(())
    }

    #[test]
    fn cleanup_marker_shortest_path_proof_allows_deleted_output_dir() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"old")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::CleanupComplete;
        marker.gate2_checks.count.0 = true;
        marker.gate2_checks.blake3_sample.0 = true;
        marker.gate2_checks.no_error.0 = true;
        marker.gate3_checks.count_x3.0 = true;
        marker.gate3_checks.sync.0 = true;
        marker.gate3_checks.quarantine.0 = true;
        marker.gate3_checks.chain.0 = true;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: "already-imported-output-hash".to_string(),
                library_asset: Some("already-imported-output-hash".to_string()),
            },
        );
        std::fs::remove_file(&src)?;
        let current_hashes = BTreeMap::new();

        validate_cleanup_complete_marker(&marker, &src_root, 0, &current_hashes)?;
        Ok(())
    }

    #[test]
    fn gate_passed_marker_resumes_partially_completed_source_cleanup() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let deleted = src_root.join("deleted.jpg");
        let pending = src_root.join("pending.jpg");
        let retained = src_root.join("retained.jpg");
        write_jpeg(&deleted, b"deleted after gate")?;
        write_jpeg(&pending, b"pending cleanup")?;
        write_jpeg(&retained, b"explicitly retained")?;
        let deleted_hash = foundation::common_utils::calculate_blake3_hash(&deleted)?;
        let pending_hash = foundation::common_utils::calculate_blake3_hash(&pending)?;
        let retained_hash = foundation::common_utils::calculate_blake3_hash(&retained)?;

        std::fs::create_dir_all(&wc)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc.clone(), 3);
        marker.stage = FastImgStageName::Gate1Passed;
        for (rel, src_hash) in [("deleted.jpg", deleted_hash), ("pending.jpg", pending_hash)] {
            let out_rel = PathBuf::from(rel)
                .with_extension("JXL")
                .to_string_lossy()
                .to_string();
            let output = wc.join(&out_rel);
            std::fs::write(&output, format!("verified output for {rel}"))?;
            marker.blake3_log.insert(
                rel.to_string(),
                Blake3Entry {
                    out_rel: Some(out_rel),
                    src: src_hash,
                    out: foundation::common_utils::calculate_blake3_hash(&output)?,
                    library_asset: None,
                },
            );
        }
        marker.skipped_sources.insert(
            "retained.jpg".to_string(),
            SkippedSourceEntry {
                src: retained_hash,
                reason: "test retained source".to_string(),
            },
        );
        std::fs::remove_file(&deleted)?;
        let current_hashes = fast_img_source_hash_set(&src_root, &[pending, retained])?;

        for (stage, strategy) in [
            (FastImgStageName::Gate1Passed, "jxl"),
            (FastImgStageName::Gate1Passed, "avif"),
            (FastImgStageName::Gate3Passed, "avif"),
        ] {
            marker.stage = stage;
            marker.strategy = strategy.to_string();
            assert!(fast_img_cleanup_resume_source_subset_matches(
                &marker,
                &current_hashes
            )?);
            assert!(!fast_img_marker_input_state_is_stale(
                &marker,
                &src_root,
                2,
                &current_hashes,
                strategy
            )?);
            validate_fast_img_marker_source_state(&marker, &src_root, 2, &current_hashes)?;
            fast_img_validate_recorded_source_hashes_current(&marker, &current_hashes)?;
            fast_img_validate_jxl_only_delivery_exit(&marker, 2, &current_hashes, strategy)?;
        }
        Ok(())
    }

    #[test]
    fn gate1_failed_partial_marker_accepts_unchanged_sources_for_auto_retry() -> anyhow::Result<()>
    {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let logged_src = src_root.join("a.jpg");
        let pending_src = src_root.join("b.jpg");
        write_jpeg(&logged_src, b"logged")?;
        write_jpeg(&pending_src, b"pending")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::Gate1Failed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&logged_src)?,
                out: "partial-out".to_string(),
                library_asset: None,
            },
        );
        let current_hashes = fast_img_source_hash_set(&src_root, &[logged_src, pending_src])?;

        validate_fast_img_marker_source_state(&marker, &src_root, 2, &current_hashes)?;
        Ok(())
    }

    #[test]
    fn gate1_failed_partial_marker_rejects_logged_source_drift() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let logged_src = src_root.join("a.jpg");
        let pending_src = src_root.join("b.jpg");
        write_jpeg(&logged_src, b"old-logged")?;
        write_jpeg(&pending_src, b"pending")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::Gate1Failed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&logged_src)?,
                out: "partial-out".to_string(),
                library_asset: None,
            },
        );
        write_jpeg(&logged_src, b"new-logged")?;
        let current_hashes = fast_img_source_hash_set(&src_root, &[logged_src, pending_src])?;

        let Err(err) =
            validate_fast_img_marker_source_state(&marker, &src_root, 2, &current_hashes)
        else {
            anyhow::bail!("gate1_failed marker unexpectedly accepted drifted logged source");
        };

        assert!(err.to_string().contains("source hash set changed"));
        Ok(())
    }

    #[test]
    fn encode_complete_marker_without_log_rejects_resume() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"old")?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::TranscodeComplete;
        let current_hashes = fast_img_source_hash_set(&src_root, &[src])?;

        let Err(err) =
            validate_fast_img_marker_source_state(&marker, &src_root, 1, &current_hashes)
        else {
            anyhow::bail!("encode-complete marker unexpectedly accepted missing hash log");
        };

        assert!(err.to_string().contains("missing BLAKE3 source log"));
        Ok(())
    }

    #[test]
    fn default_policy_is_local_only_delivery() {
        assert_eq!(
            fast_img_post_gate1_policy(ShortestPathFlag(false)),
            FastImgPostGate1Policy::LocalOnlyDelivery
        );
    }

    #[test]
    fn shortest_path_policy_imports_and_verifies() {
        assert_eq!(
            fast_img_post_gate1_policy(ShortestPathFlag(true)),
            FastImgPostGate1Policy::ShortestPathImportAndVerify
        );
    }

    #[test]
    fn unfinished_fast_img_marker_requires_explicit_resume_decision() {
        let mut marker = WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("wc"), 1);
        assert!(fast_img_requires_resume_decision(
            &marker,
            ShortestPathFlag(false)
        ));
        marker.stage = FastImgStageName::CleanupComplete;
        assert!(!fast_img_requires_resume_decision(
            &marker,
            ShortestPathFlag(false)
        ));
        marker.tier2_in_progress = true;
        assert!(fast_img_requires_resume_decision(
            &marker,
            ShortestPathFlag(false)
        ));
    }

    #[test]
    fn completed_marker_requires_explicit_decision_for_reappeared_tier2_media() {
        let mut marker = WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("wc"), 0);
        marker.stage = FastImgStageName::CleanupComplete;

        assert!(!fast_img_completed_marker_has_new_tier2_work(&marker, 0));
        assert!(fast_img_completed_marker_has_new_tier2_work(&marker, 1));

        marker.tier2_in_progress = true;
        assert!(!fast_img_completed_marker_has_new_tier2_work(&marker, 1));
        assert!(fast_img_requires_resume_decision(
            &marker,
            ShortestPathFlag(false)
        ));
    }

    #[test]
    fn gate2_or_gate3_retry_reuses_complete_photos_uuid_proof() {
        let mut marker = WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("wc"), 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.AVIF".to_string()),
                src: "src".to_string(),
                out: "out".to_string(),
                library_asset: Some("out".to_string()),
            },
        );
        marker.photos_imported_assets.push(LibraryAssetRecord {
            rel_path: "a.AVIF".to_string(),
            blake3: "out".to_string(),
            sync_status: "photos_local".to_string(),
            quarantined: false,
            photos_uuid: Some("UUID-A/L0/001".to_string()),
            library_blake3: None,
        });

        marker.stage = FastImgStageName::Gate2Failed;
        assert!(fast_img_reuses_marker_import_proof_on_resume(&marker));
        marker.stage = FastImgStageName::Gate3Failed;
        assert!(fast_img_reuses_marker_import_proof_on_resume(&marker));
    }

    #[test]
    fn fast_img_command_does_not_require_database_preflight() {
        let command = Commands::FastImg {
            input: std::path::PathBuf::from("/photos"),
            output: None,
            delete_source: false,
            dry_run: true,
            recursive: true,
            auto_import: false,
            shortest_path: false,
            archive: false,
            retry: false,
            no_resume: false,
            allow_expert_options: false,
            strategy: "jxl".to_string(),
            extreme_precision: false,
        };

        assert!(!command_requires_database(&command));
    }

    #[test]
    fn run_command_accepts_underscore_expert_option_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["img", "run", "/photos", "--allow_expert_options"])?;

        let Commands::Run {
            allow_expert_options,
            ..
        } = parsed.command
        else {
            anyhow::bail!("expected run command");
        };
        assert!(allow_expert_options);
        Ok(())
    }

    #[test]
    fn fast_img_command_accepts_underscore_expert_option_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["img", "fast-img", "/photos", "--allow_expert_options"])?;

        let Commands::FastImg {
            allow_expert_options,
            ..
        } = parsed.command
        else {
            anyhow::bail!("expected fast-img command");
        };
        assert!(allow_expert_options);
        Ok(())
    }

    #[test]
    fn fast_img_command_accepts_strategy_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["img", "fast-img", "/photos", "--strategy", "avif"])?;

        let Commands::FastImg { strategy, .. } = parsed.command else {
            anyhow::bail!("expected fast-img command");
        };
        assert_eq!(strategy, "avif");
        Ok(())
    }

    #[test]
    fn fast_img_command_rejects_unknown_strategy_flag() {
        assert!(matches!(
            Cli::try_parse_from(["img", "fast-img", "/photos", "--strategy", "gif"]),
            Err(err) if err.kind() == ErrorKind::InvalidValue
        ));
    }

    #[test]
    fn run_command_accepts_archive_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["img", "run", "/photos", "--archive"])?;

        let Commands::Run { archive, .. } = parsed.command else {
            anyhow::bail!("expected run command");
        };
        assert!(archive);
        Ok(())
    }

    #[test]
    fn normal_img_run_rejects_manual_avif_before_database_preflight() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["img", "run", "/media/in", "--codec", "av1"])?;

        let error = super::validate_command_strategy(&parsed.command).unwrap_err();
        assert!(error.to_string().contains("img fast-img --strategy avif"));
        Ok(())
    }

    #[test]
    fn fast_img_command_accepts_archive_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["img", "fast-img", "/photos", "--archive"])?;

        let Commands::FastImg { archive, .. } = parsed.command else {
            anyhow::bail!("expected fast-img command");
        };
        assert!(archive);
        Ok(())
    }

    #[test]
    fn run_command_rejects_dash_expert_option_alias() -> anyhow::Result<()> {
        let err = match Cli::try_parse_from(["img", "run", "/photos", "--allow-expert-options"]) {
            Ok(_) => anyhow::bail!("run accepted --allow-expert-options"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("unexpected argument"),
            "dash expert flag should be rejected, got: {err}"
        );
        Ok(())
    }

    #[test]
    fn fast_img_command_rejects_dash_expert_option_alias() -> anyhow::Result<()> {
        let err =
            match Cli::try_parse_from(["img", "fast-img", "/photos", "--allow-expert-options"]) {
                Ok(_) => anyhow::bail!("fast-img accepted --allow-expert-options"),
                Err(err) => err,
            };

        assert!(
            err.to_string().contains("unexpected argument"),
            "dash expert flag should be rejected, got: {err}"
        );
        Ok(())
    }

    #[test]
    fn delete_notice_warns_source_jpegs_are_deleted_without_prompting() {
        let message = fast_img_delete_notice_message(3, 0, std::path::Path::new("/photos"), "jxl");
        assert!(message.contains("will directly delete original JPEG files"));
        assert!(message.contains("JXL-only delivery"));
    }

    #[test]
    fn delete_notice_does_not_claim_extra_source_scans() {
        let message = fast_img_delete_notice_message(2, 0, std::path::Path::new("/photos"), "avif");
        assert!(message.contains("AVIF-only (Meme Mode)"));
        assert!(!message.contains("tier-2"));
    }

    #[test]
    fn fastmode_parallelism_caps_to_pending_jobs_and_keeps_child_threads() {
        assert_eq!(fast_img_effective_encode_parallelism(3, 8, 2), (3, 2));
        assert_eq!(fast_img_effective_encode_parallelism(10, 4, 0), (4, 1));
        assert_eq!(fast_img_effective_encode_parallelism(0, 4, 2), (1, 2));
    }

    #[test]
    fn jxl_only_delivery_accepts_gate1_without_photos_verifier() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&src, b"jxl-only")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::write(&out, b"jxl")?;
        let current_hashes = fast_img_source_hash_set(&src_root, std::slice::from_ref(&src))?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: current_hashes
                    .get("a.jpg")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing test source hash"))?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );

        fast_img_validate_jxl_only_delivery_exit(&marker, 1, &current_hashes, "jxl")?;
        Ok(())
    }

    #[test]
    fn fast_img_expected_count_excludes_explicitly_skipped_sources() {
        let mut marker =
            WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("src_optimized"), 3);
        marker.skipped_sources.insert(
            "b.jpg".to_string(),
            SkippedSourceEntry {
                src: "skipped-source-hash".to_string(),
                reason: "lossless JPEG encode failed after strict cascade".to_string(),
            },
        );

        assert_eq!(fast_img_effective_expected_count(&marker, 3, false), 2);
        assert_eq!(fast_img_effective_expected_count(&marker, 0, true), 2);
    }

    #[test]
    fn fast_img_expected_count_excludes_explicitly_failed_sources() {
        let mut marker =
            WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("src_optimized"), 4);
        marker.failed_sources.insert(
            "b.jpg".to_string(),
            SkippedSourceEntry {
                src: "failed-source-hash".to_string(),
                reason: "pixel-diff: djxl exited non-zero decoding output.JXL".to_string(),
            },
        );

        assert_eq!(fast_img_effective_expected_count(&marker, 4, false), 3);
        assert_eq!(fast_img_effective_expected_count(&marker, 0, true), 3);
    }

    #[test]
    fn fast_img_marker_outputs_current_accepts_recorded_skipped_sources() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let converted_src = src_root.join("a.jpg");
        let skipped_src = src_root.join("b.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&converted_src, b"converted")?;
        write_jpeg(&skipped_src, b"skipped")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::write(&out, b"jxl-output")?;

        let mut marker = WorkingCopyMarker::new(src_root, wc, 2);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&converted_src)?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );
        marker.skipped_sources.insert(
            "b.jpg".to_string(),
            SkippedSourceEntry {
                src: foundation::common_utils::calculate_blake3_hash(&skipped_src)?,
                reason: "lossless JPEG encode failed after strict cascade".to_string(),
            },
        );

        assert!(fast_img_marker_outputs_current(&marker)?);
        Ok(())
    }

    #[test]
    fn jxl_only_delivery_accepts_recorded_skipped_source_remaining() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let converted_src = src_root.join("a.jpg");
        let skipped_src = src_root.join("b.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&converted_src, b"converted")?;
        write_jpeg(&skipped_src, b"skipped")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::write(&out, b"jxl-output")?;
        let current_hashes = fast_img_source_hash_set(&src_root, &[converted_src, skipped_src])?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 2);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: current_hashes
                    .get("a.jpg")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing converted source hash"))?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );
        marker.skipped_sources.insert(
            "b.jpg".to_string(),
            SkippedSourceEntry {
                src: current_hashes
                    .get("b.jpg")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing skipped source hash"))?,
                reason: "lossless JPEG encode failed after strict cascade".to_string(),
            },
        );

        fast_img_validate_jxl_only_delivery_exit(&marker, 2, &current_hashes, "jxl")?;
        Ok(())
    }

    #[test]
    fn verified_source_deletion_keeps_recorded_skipped_sources() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let converted_src = src_root.join("a.jpg");
        let skipped_src = src_root.join("b.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&converted_src, b"converted")?;
        write_jpeg(&skipped_src, b"skipped")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::write(&out, b"jxl-output")?;
        let source_hash = foundation::common_utils::calculate_blake3_hash(&converted_src)?;
        let output_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        let skipped_hash = foundation::common_utils::calculate_blake3_hash(&skipped_src)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: source_hash.clone(),
                out: output_hash.clone(),
                library_asset: None,
            },
        );
        marker.skipped_sources.insert(
            "b.jpg".to_string(),
            SkippedSourceEntry {
                src: skipped_hash,
                reason: "lossless JPEG encode failed after strict cascade".to_string(),
            },
        );

        let (deleted, already_deleted) =
            fast_img_delete_verified_source_jpegs_with(&marker, &src_root, |_source, _output| {
                Ok(IntegrityResult::FinalModernDelivery {
                    source_hash: source_hash.clone(),
                    output_hash: output_hash.clone(),
                })
            })?;

        assert_eq!((deleted, already_deleted), (1, 0));
        assert!(!converted_src.exists());
        assert!(skipped_src.exists());
        Ok(())
    }

    #[test]
    fn verified_source_deletion_proceeds_with_incomplete_disposition() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let converted_src = src_root.join("a.jpg");
        let unrecorded_src = src_root.join("b.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&converted_src, b"converted")?;
        write_jpeg(&unrecorded_src, b"unrecorded")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::write(&out, b"jxl-output")?;
        let source_hash = foundation::common_utils::calculate_blake3_hash(&converted_src)?;
        let output_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: source_hash.clone(),
                out: output_hash.clone(),
                library_asset: None,
            },
        );

        let (deleted, already_deleted) =
            fast_img_delete_verified_source_jpegs_with(&marker, &src_root, |_source, _output| {
                Ok(IntegrityResult::FinalModernDelivery {
                    source_hash: source_hash.clone(),
                    output_hash: output_hash.clone(),
                })
            })?;

        assert_eq!((deleted, already_deleted), (1, 0));
        assert!(!converted_src.exists());
        assert!(unrecorded_src.exists());
        Ok(())
    }

    #[test]
    fn verified_source_deletion_rejects_over_recorded_disposition() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "hash-a".to_string(),
                out: "out-a".to_string(),
                library_asset: None,
            },
        );
        marker.skipped_sources.insert(
            "b.jpg".to_string(),
            SkippedSourceEntry {
                src: "hash-b".to_string(),
                reason: "phantom skip".to_string(),
            },
        );

        let err = match fast_img_delete_verified_source_jpegs_with(
            &marker,
            &src_root,
            |_source, _output| {
                Ok(IntegrityResult::FinalModernDelivery {
                    source_hash: "hash-a".to_string(),
                    output_hash: "out-a".to_string(),
                })
            },
        ) {
            Ok(_) => anyhow::bail!("over-recorded disposition unexpectedly allowed deletion"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("over-recorded disposition"),
            "unexpected: {err}"
        );
        Ok(())
    }

    #[test]
    fn verified_source_deletion_rejects_duplicate_disposition() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "hash-a".to_string(),
                out: "out-a".to_string(),
                library_asset: None,
            },
        );
        marker.skipped_sources.insert(
            "a.jpg".to_string(),
            SkippedSourceEntry {
                src: "hash-a".to_string(),
                reason: "duplicate".to_string(),
            },
        );

        let err = match fast_img_delete_verified_source_jpegs_with(
            &marker,
            &src_root,
            |_source, _output| {
                Ok(IntegrityResult::FinalModernDelivery {
                    source_hash: "hash-a".to_string(),
                    output_hash: "out-a".to_string(),
                })
            },
        ) {
            Ok(_) => anyhow::bail!("duplicate disposition unexpectedly allowed deletion"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("disposition overlap"),
            "unexpected: {err}"
        );
        Ok(())
    }

    #[test]
    fn fast_img_planned_output_rel_disambiguates_extension_collisions() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        std::fs::create_dir_all(&src_root)?;
        std::fs::create_dir_all(&wc)?;
        let source_jpeg = src_root.join("photo.jpeg");
        let input_jpg_file = src_root.join("photo.jpg");
        write_jpeg(&source_jpeg, b"jpeg")?;
        write_jpeg(&input_jpg_file, b"jpg")?;

        let (_, out_path_jxl) =
            fast_img_planned_output_rel(&source_jpeg, &wc, Path::new("photo.jpeg"), "hevc")?;
        let (_, collision_jxl_rel) =
            fast_img_planned_output_rel(&input_jpg_file, &wc, Path::new("photo.jpg"), "hevc")?;

        assert_eq!(out_path_jxl, "photo.JXL");
        assert_ne!(out_path_jxl, collision_jxl_rel);
        assert!(
            collision_jxl_rel.contains("photo"),
            "disambiguated output should stay photo-derived: {collision_jxl_rel}"
        );
        Ok(())
    }

    #[test]
    fn fast_img_reconcile_unrecorded_sources_records_explicit_skips() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let converted_src = src_root.join("a.jpg");
        let missing_src = src_root.join("b.jpg");
        write_jpeg(&converted_src, b"converted")?;
        write_jpeg(&missing_src, b"missing")?;
        let current_hashes =
            fast_img_source_hash_set(&src_root, &[converted_src.clone(), missing_src.clone()])?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: current_hashes
                    .get("a.jpg")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing hash for a.jpg"))?,
                out: "out-a".to_string(),
                library_asset: None,
            },
        );

        let reconciled = fast_img_reconcile_unrecorded_source_disposition(
            &mut marker,
            &src_root,
            &[converted_src, missing_src.clone()],
            &current_hashes,
        )?;

        assert_eq!(reconciled, 1);
        assert!(marker.source_disposition_is_complete());
        let skipped = marker
            .skipped_sources
            .get("b.jpg")
            .ok_or_else(|| anyhow::anyhow!("missing reconciled skip for b.jpg"))?;
        assert_eq!(
            skipped.src,
            current_hashes
                .get("b.jpg")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing hash for b.jpg"))?
        );
        assert!(
            skipped.reason.contains("without disposition record"),
            "unexpected reason: {}",
            skipped.reason
        );
        assert!(missing_src.exists());
        Ok(())
    }

    #[test]
    fn jxl_only_delivery_accepts_after_reconcile_of_unrecorded_sources() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let converted_src = src_root.join("a.jpg");
        let missing_src = src_root.join("b.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&converted_src, b"converted")?;
        write_jpeg(&missing_src, b"missing")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::write(&out, b"jxl-output")?;
        let sources = vec![converted_src, missing_src];
        let current_hashes = fast_img_source_hash_set(&src_root, &sources)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: current_hashes
                    .get("a.jpg")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing hash for a.jpg"))?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );

        let reconciled = fast_img_reconcile_unrecorded_source_disposition(
            &mut marker,
            &src_root,
            &sources,
            &current_hashes,
        )?;
        assert_eq!(reconciled, 1);

        fast_img_validate_jxl_only_delivery_exit(&marker, 2, &current_hashes, "jxl")?;
        Ok(())
    }

    #[test]
    fn jxl_only_delivery_rejects_missing_jxl_output() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"jxl-only")?;
        let current_hashes = fast_img_source_hash_set(&src_root, std::slice::from_ref(&src))?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: current_hashes
                    .get("a.jpg")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing test source hash"))?,
                out: "missing-output-hash".to_string(),
                library_asset: None,
            },
        );

        let err = match fast_img_validate_jxl_only_delivery_exit(&marker, 1, &current_hashes, "jxl")
        {
            Ok(()) => anyhow::bail!("missing JXL output unexpectedly passed delivery exit"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("fast-img JXL output proof missing/drifted before delivery"),
            "unexpected: {err}"
        );
        Ok(())
    }

    #[test]
    fn gate1_marker_with_missing_jxl_output_is_not_current() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("a.jpg");
        write_jpeg(&src, b"source")?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: "missing-output-hash".to_string(),
                library_asset: None,
            },
        );

        assert!(!fast_img_marker_outputs_current(&marker)?);
        Ok(())
    }

    #[test]
    fn fast_img_skip_hashes_match_propagates_source_hash_errors() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src = root.path().join("a.jpg");
        let out = root.path().join("a.JXL");
        std::fs::create_dir_all(&src)?;
        std::fs::write(&out, b"jxl")?;
        let entry = Blake3Entry {
            out_rel: Some("a.JXL".to_string()),
            src: "logged-source-hash".to_string(),
            out: foundation::common_utils::calculate_blake3_hash(&out)?,
            library_asset: None,
        };

        let Err(err) = fast_img_skip_hashes_match(&src, &out, &entry) else {
            anyhow::bail!("expected fast-img resume source BLAKE3 read failure");
        };

        assert!(
            err.to_string()
                .contains("fast-img resume source BLAKE3 read failed"),
            "unexpected: {err}"
        );
        Ok(())
    }

    #[test]
    fn fast_img_rejects_source_replaced_during_encode() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src = root.path().join("source.jpg");
        std::fs::write(&src, b"before encode")?;
        let expected_hash = foundation::common_utils::calculate_blake3_hash(&src)?;
        std::fs::write(&src, b"replacement while encode was running")?;

        let err = fast_img_verify_source_hash_unchanged(&src, &expected_hash)
            .expect_err("changed source must invalidate the encoded output");
        assert!(
            err.to_string()
                .contains("source changed while it was being encoded"),
            "unexpected: {err}"
        );
        Ok(())
    }

    #[test]
    fn wc_contains_only_jxl_after_gate1() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("Photos_");
        std::fs::create_dir_all(&wc)?;
        let jpeg = wc.join("spoof.JXL");
        let jxl = wc.join("actual.bin");
        write_jpeg(&jpeg, b"source")?;
        std::fs::write(&jxl, b"\xff\x0a")?;

        fast_img_strip_non_target_files(&wc, "jxl")?;

        assert!(!jpeg.exists());
        assert!(jxl.exists());
        let remaining = std::fs::read_dir(&wc)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(remaining, vec![std::ffi::OsString::from("actual.bin")]);
        Ok(())
    }

    #[test]
    fn wc_contains_only_avif_after_gate1() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("Photos_");
        std::fs::create_dir_all(&wc)?;
        let jpeg = wc.join("spoof.AVIF");
        let avif = wc.join("actual.bin");
        write_jpeg(&jpeg, b"source")?;
        write_avif_ftyp_stub(&avif)?;

        fast_img_strip_non_target_files(&wc, "avif")?;

        assert!(!jpeg.exists());
        assert!(avif.exists());
        let remaining = std::fs::read_dir(&wc)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(remaining, vec![std::ffi::OsString::from("actual.bin")]);
        Ok(())
    }

    #[test]
    fn fast_img_delete_verifier_uses_content_format_not_extension() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let avif = root.path().join("actual.jxl");
        let spoof = root.path().join("spoof.avif");
        write_avif_ftyp_stub(&avif)?;
        write_jpeg(&spoof, b"not avif")?;

        assert_eq!(
            fast_img_verified_output_format(&avif, "avif")?,
            FormatKind::Avif
        );
        let err = fast_img_verified_output_format(&spoof, "avif")
            .expect_err("extension-spoofed AVIF must fail before source deletion");
        assert!(err.to_string().contains("content format mismatch"));
        Ok(())
    }

    #[test]
    fn fastmode_marker_preserves_nested_output_structure() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let src = src_root.join("trip/day1/a.jpg");
        let out = wc.join("trip/day1/a.JXL");
        write_jpeg(&src, b"nested")?;
        let out_parent = out
            .parent()
            .ok_or_else(|| anyhow::anyhow!("nested output missing parent"))?;
        std::fs::create_dir_all(out_parent)?;
        std::fs::write(&out, b"jxl")?;

        let mut marker = WorkingCopyMarker::new(src_root, wc.clone(), 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.blake3_log.insert(
            "trip/day1/a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("trip/day1/a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );

        let resolved = fast_img_marker_entry_output_path(
            &marker,
            "trip/day1/a.jpg",
            marker
                .blake3_log
                .get("trip/day1/a.jpg")
                .ok_or_else(|| anyhow::anyhow!("missing nested marker entry"))?,
        )?;

        assert_eq!(resolved, wc.join("trip/day1/a.JXL"));
        Ok(())
    }

    #[test]
    fn normal_mode_records_only_rejected_photos_item_as_failed() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_dir = root.path().join("src");
        let working_copy = root.path().join("working");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&working_copy)?;
        let mut marker = WorkingCopyMarker::new(src_dir, working_copy.clone(), 2)
            .with_strategy("avif".to_string());
        for name in ["a", "b"] {
            std::fs::write(working_copy.join(format!("{name}.AVIF")), name.as_bytes())?;
            marker.blake3_log.insert(
                format!("{name}.png"),
                Blake3Entry {
                    src: format!("src-{name}"),
                    out: format!("out-{name}"),
                    out_rel: Some(format!("{name}.AVIF")),
                    library_asset: None,
                },
            );
        }
        let candidates = ["a", "b"]
            .into_iter()
            .map(|name| foundation::fast_img::PhotosImportCandidate {
                path: working_copy.join(format!("{name}.AVIF")),
                rel_path: format!("{name}.AVIF"),
                blake3: format!("out-{name}"),
                album_name: "test".to_string(),
            })
            .collect::<Vec<_>>();
        let mut library = foundation::pipeline::verification::LibraryHandle {
            imported_assets: vec![foundation::pipeline::verification::LibraryAssetRecord {
                rel_path: "a.AVIF".to_string(),
                blake3: "out-a".to_string(),
                sync_status: "photos_local".to_string(),
                quarantined: false,
                photos_uuid: Some("UUID-A".to_string()),
                library_blake3: None,
            }],
            import_error_count: 1,
        };
        apply_library_assets_to_marker(&mut marker, &library)?;

        assert_eq!(
            super::fast_img_record_controllable_media_import_failures(
                &mut marker,
                &candidates,
                &mut library,
            )?,
            1
        );
        assert_eq!(library.import_error_count, 0);
        assert!(marker.blake3_log.contains_key("a.png"));
        assert_eq!(
            marker.blake3_log["a.png"].library_asset.as_deref(),
            Some("out-a")
        );
        assert!(marker.failed_sources.contains_key("b.png"));
        assert!(working_copy.join("a.AVIF").is_file());
        assert!(!working_copy.join("b.AVIF").exists());
        assert_eq!(fast_img_effective_expected_count(&marker, 2, false), 1);
        Ok(())
    }

    #[test]
    fn shortest_path_library_handle_passes_gate2_and_gate3_with_real_file_probe()
    -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let library_root = root.path().join("Photos Library.photoslibrary");
        let src = src_root.join("a.jpg");
        let out = wc.join("a.JXL");
        let library_asset = library_root.join("Masters/a.JXL");
        write_jpeg(&src, b"source")?;
        std::fs::create_dir_all(&wc)?;
        let library_asset_parent = library_asset
            .parent()
            .ok_or_else(|| anyhow::anyhow!("test library asset path has no parent"))?;
        std::fs::create_dir_all(library_asset_parent)?;
        std::fs::write(&out, MINIMAL_JXL_BYTES)?;
        std::fs::write(&library_asset, MINIMAL_JXL_BYTES)?;

        let src_hash = foundation::common_utils::calculate_blake3_hash(&src)?;
        let out_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc.clone(), 1);
        marker.stage = FastImgStageName::ImportComplete;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: src_hash,
                out: out_hash,
                library_asset: None,
            },
        );

        let handle = library_handle_from_probes(
            &marker,
            &[("a.JXL".to_string(), "UUID-A".to_string())],
            |uuid| {
                assert_eq!(uuid, "UUID-A");
                Ok(FastImgLibraryAssetProbe {
                    uuid: uuid.to_string(),
                    path: library_asset.clone(),
                    iscloudasset: true,
                    incloud: Some(true),
                    ismissing: false,
                })
            },
            |_path| Ok(false),
        )?;
        apply_library_assets_to_marker(&mut marker, &handle)?;

        let ctx = PipelineCtx {
            working_copy: wc,
            src_dir: src_root,
            blake3_log: marker.blake3_log,
            expected_count: 1,
            library_handle: Some(handle),
            output_format: None,
        };

        assert!(Gate2Import.run(&ctx).passed);
        assert!(Gate3Deep.run(&ctx).passed);
        Ok(())
    }

    #[test]
    fn shortest_path_library_handle_queries_osxphotos_uuid_from_photos_local_identifier()
    -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let library_asset = root.path().join("library/a.JXL");
        std::fs::create_dir_all(&wc)?;
        let library_asset_parent = library_asset
            .parent()
            .ok_or_else(|| anyhow::anyhow!("test library asset path has no parent"))?;
        std::fs::create_dir_all(library_asset_parent)?;
        std::fs::write(wc.join("a.JXL"), MINIMAL_JXL_BYTES)?;
        std::fs::write(&library_asset, MINIMAL_JXL_BYTES)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: foundation::common_utils::calculate_blake3_hash(&library_asset)?,
                library_asset: None,
            },
        );

        let handle = library_handle_from_probes(
            &marker,
            &[("a.JXL".to_string(), "UUID-A/L0/001".to_string())],
            |uuid| {
                assert_eq!(uuid, "UUID-A");
                Ok(FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: library_asset.clone(),
                    iscloudasset: true,
                    incloud: Some(true),
                    ismissing: false,
                })
            },
            |_path| Ok(false),
        )?;

        assert_eq!(handle.imported_assets.len(), 1);
        Ok(())
    }

    #[test]
    fn shortest_path_library_handle_accepts_photos_local_custody_by_default() -> anyhow::Result<()>
    {
        let _env = fast_img_single_query_local_test_policy();
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let library_asset = root.path().join("library/a.JXL");
        std::fs::create_dir_all(&wc)?;
        let library_asset_parent = library_asset
            .parent()
            .ok_or_else(|| anyhow::anyhow!("test library asset path has no parent"))?;
        std::fs::create_dir_all(library_asset_parent)?;
        std::fs::write(wc.join("a.JXL"), MINIMAL_JXL_BYTES)?;
        std::fs::write(&library_asset, MINIMAL_JXL_BYTES)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc.clone(), 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: foundation::common_utils::calculate_blake3_hash(&library_asset)?,
                library_asset: None,
            },
        );

        let handle = library_handle_from_probes(
            &marker,
            &[("a.JXL".to_string(), "UUID-A".to_string())],
            |_uuid| {
                Ok(FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: library_asset.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                })
            },
            |_path| Ok(false),
        )?;
        assert_eq!(handle.imported_assets[0].sync_status, "photos_local");
        apply_library_assets_to_marker(&mut marker, &handle)?;

        let ctx = PipelineCtx {
            working_copy: wc,
            src_dir: src_root,
            blake3_log: marker.blake3_log,
            expected_count: 1,
            library_handle: Some(handle),
            output_format: None,
        };

        assert!(Gate2Import.run(&ctx).passed);
        assert!(Gate3Deep.run(&ctx).passed);
        Ok(())
    }

    #[test]
    fn shortest_path_gate3_context_uses_marker_after_library_assets_applied() -> anyhow::Result<()>
    {
        let _env = fast_img_single_query_local_test_policy();
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let library_asset = root.path().join("library/a.JXL");
        let src = src_root.join("a.jpg");
        let out = wc.join("a.JXL");
        write_jpeg(&src, b"source")?;
        std::fs::create_dir_all(&wc)?;
        std::fs::create_dir_all(
            library_asset
                .parent()
                .ok_or_else(|| anyhow::anyhow!("test library asset path has no parent"))?,
        )?;
        std::fs::write(&out, MINIMAL_JXL_BYTES)?;
        std::fs::write(&library_asset, MINIMAL_JXL_BYTES)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: foundation::common_utils::calculate_blake3_hash(&out)?,
                library_asset: None,
            },
        );
        let handle = library_handle_from_probes(
            &marker,
            &[("a.JXL".to_string(), "UUID-A".to_string())],
            |_uuid| {
                Ok(FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: library_asset.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                })
            },
            |_path| Ok(false),
        )?;
        apply_library_assets_to_marker(&mut marker, &handle)?;

        let ctx = fast_img_pipeline_ctx(&marker, marker.src_jpeg_count, Some(handle), None);

        assert!(Gate3Deep.run(&ctx).passed);
        Ok(())
    }

    #[test]
    fn shortest_path_library_handle_fails_closed_without_icloud_upload_when_required()
    -> anyhow::Result<()> {
        let _env = fast_img_single_query_icloud_test_policy();
        let root = TempDir::new()?;
        let src_root = root.path().join("Photos");
        let wc = root.path().join("Photos_");
        let library_asset = root.path().join("library/a.JXL");
        std::fs::create_dir_all(&wc)?;
        let library_asset_parent = library_asset
            .parent()
            .ok_or_else(|| anyhow::anyhow!("test library asset path has no parent"))?;
        std::fs::create_dir_all(library_asset_parent)?;
        std::fs::write(wc.join("a.JXL"), MINIMAL_JXL_BYTES)?;
        std::fs::write(&library_asset, MINIMAL_JXL_BYTES)?;
        let mut marker = WorkingCopyMarker::new(src_root, wc, 1);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: foundation::common_utils::calculate_blake3_hash(&library_asset)?,
                library_asset: None,
            },
        );

        let Err(err) = library_handle_from_probes(
            &marker,
            &[("a.JXL".to_string(), "UUID-A".to_string())],
            |_uuid| {
                Ok(FastImgLibraryAssetProbe {
                    uuid: "UUID-A".to_string(),
                    path: library_asset.clone(),
                    iscloudasset: false,
                    incloud: Some(false),
                    ismissing: false,
                })
            },
            |_path| Ok(false),
        ) else {
            anyhow::bail!("shortest-path verifier accepted missing iCloud upload proof");
        };

        assert!(err.to_string().contains("without required proof"));
        Ok(())
    }

    #[test]
    fn fast_img_avif_meme_quality_uses_final_speed_zero_domain() {
        assert_eq!(super::AVIF_MEME_SPEED, 0);
        assert_eq!(
            foundation::exploration_policy::AvifSpeedDomain::MEME_QUALITY_LOCATOR.value(),
            6
        );
        assert_eq!(
            foundation::exploration_policy::EncoderDomain::avif(super::AVIF_MEME_SPEED),
            foundation::exploration_policy::EncoderDomain::avif(0),
        );
        assert_eq!(super::AVIF_MEME_MIN_QUALITY, 0);
        assert!(img::lossless_converter::AVIF_QUALITY_BINARY_PROBE_BUDGET >= 7);
    }

    #[test]
    fn fast_img_lossless_meme_candidate_uses_the_same_strict_size_policy() {
        assert!(super::avif_lossless_candidate_fits_source(
            999,
            1_000,
            foundation::exploration_policy::SizePolicy::StrictlySmaller
        ));
        assert!(!super::avif_lossless_candidate_fits_source(
            1_000,
            1_000,
            foundation::exploration_policy::SizePolicy::StrictlySmaller
        ));
        assert!(!super::avif_lossless_candidate_fits_source(
            1_001,
            1_000,
            foundation::exploration_policy::SizePolicy::StrictlySmaller
        ));
    }

    #[test]
    fn test_fast_img_meme_mode_forces_all_non_avif_static_formats_to_avif() {
        for format in [
            FormatKind::Jxl,
            FormatKind::WebP,
            FormatKind::Heic,
            FormatKind::Heif,
            FormatKind::Jpeg,
            FormatKind::Png,
            FormatKind::Gif,
            FormatKind::Bmp,
            FormatKind::Tiff,
        ] {
            assert!(
                !matches!(
                    format,
                    FormatKind::Mp4
                        | FormatKind::Mov
                        | FormatKind::Mkv
                        | FormatKind::Webm
                        | FormatKind::Unknown
                ),
                "Format {format:?} must be candidate for forced AVIF conversion in Meme Mode"
            );
        }
    }

    #[test]
    fn fast_img_avif_uses_direct_inputs_before_explicit_magick_adapters() -> anyhow::Result<()> {
        use super::AvifInputDecoder;

        for (format, decoder) in [
            (FormatKind::WebP, AvifInputDecoder::WebP),
            (FormatKind::Avif, AvifInputDecoder::Avif),
            (FormatKind::Heic, AvifInputDecoder::Heif),
            (FormatKind::Heif, AvifInputDecoder::Heif),
            (FormatKind::Jxl, AvifInputDecoder::Jxl),
            (FormatKind::Jp2, AvifInputDecoder::Jp2),
            (FormatKind::Gif, AvifInputDecoder::ImageMagick),
        ] {
            assert_eq!(super::avif_input_decoder(format), Some(decoder));
        }
        assert_eq!(super::avif_input_decoder(FormatKind::Jpeg), None);
        assert_eq!(super::avif_input_decoder(FormatKind::Png), None);
        assert_eq!(super::avif_input_decoder(FormatKind::Mp4), None);
        assert!(!super::avif_decoder_allows_imagemagick_fallback(
            AvifInputDecoder::WebP,
            false,
        ));
        assert!(super::avif_decoder_allows_imagemagick_fallback(
            AvifInputDecoder::WebP,
            true,
        ));

        let direct = super::prepare_fast_img_avif_encoder_input(
            Path::new("direct.jpg"),
            FormatKind::Jpeg,
            false,
        )?;
        assert_eq!(direct.path, Path::new("direct.jpg"));

        let adapter_error = match super::prepare_fast_img_avif_encoder_input(
            Path::new("static.bmp"),
            FormatKind::Bmp,
            false,
        ) {
            Ok(_) => anyhow::bail!("ImageMagick adapter must require explicit expert mode"),
            Err(err) => err,
        };
        assert!(adapter_error.to_string().contains("--allow-expert-options"));

        let (command, tool) = super::avif_input_decoder_command(
            AvifInputDecoder::ImageMagick,
            Path::new("static.gif"),
            Path::new("decoded.png"),
        )?;
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(tool, "ImageMagick");
        assert!(args.iter().any(|arg| arg.ends_with("static.gif[0]")));
        assert!(!args.iter().any(|arg| arg == "-flatten"));
        assert!(args.iter().any(|arg| arg == "-alpha"));
        assert!(args.iter().any(|arg| arg == "set"));
        assert!(args.iter().any(|arg| arg == "-strip"));
        Ok(())
    }

    #[test]
    fn meme_avif_failed_probes_do_not_move_verified_size_bounds() {
        let mut evidence = super::AvifMemeQualityEvidence::default();
        let oversize = super::AvifMemeCandidate {
            speed_domain: super::avif_meme_speed_domain(),
            quality: 100,
            temp_path: std::path::PathBuf::from("q100.avif"),
            output_size: 100,
            pure_media_size: 100,
            content_blake3: "q100".to_string(),
        };
        let fitting = super::AvifMemeCandidate {
            speed_domain: super::avif_meme_speed_domain(),
            quality: 90,
            temp_path: std::path::PathBuf::from("q90.avif"),
            output_size: 90,
            pure_media_size: 90,
            content_blake3: "q90".to_string(),
        };
        evidence.record_oversize(&oversize);
        evidence.record_failed(95);
        evidence.record_fit(&fitting);

        assert_eq!(evidence.verified_bracket(), Some((90, 100)));
        assert_eq!(evidence.next_refinement_quality(), Some(96));
        assert_eq!(evidence.lowest_oversize_quality, Some(100));
        assert!(evidence.failed_qualities.contains(&95));
    }

    #[test]
    fn meme_avif_strict_policy_uses_pure_media_payload() {
        let q100 = super::AvifMemeCandidate {
            speed_domain: super::avif_meme_speed_domain(),
            quality: 100,
            temp_path: std::path::PathBuf::from("q100.avif"),
            output_size: 1_300,
            pure_media_size: 900,
            content_blake3: "q100".to_string(),
        };
        assert!(super::avif_meme_candidate_fits_source(
            &q100,
            950,
            foundation::exploration_policy::SizePolicy::StrictlySmaller
        ));
        assert!(!super::avif_meme_candidate_fits_source(
            &q100,
            899,
            foundation::exploration_policy::SizePolicy::StrictlySmaller
        ));
        assert!(
            !super::avif_meme_candidate_fits_source(
                &q100,
                900,
                foundation::exploration_policy::SizePolicy::StrictlySmaller
            ),
            "strict Meme size policy must reject an equal pure-media payload"
        );
    }

    #[test]
    fn pure_media_parsers_exclude_jpeg_and_png_metadata() -> anyhow::Result<()> {
        fn append_png_chunk(
            bytes: &mut Vec<u8>,
            chunk_type: [u8; 4],
            payload: &[u8],
        ) -> anyhow::Result<()> {
            bytes.extend_from_slice(&u32::try_from(payload.len())?.to_be_bytes());
            bytes.extend_from_slice(&chunk_type);
            bytes.extend_from_slice(payload);
            bytes.extend_from_slice(&[0_u8; 4]);
            Ok(())
        }

        let root = TempDir::new()?;
        let jpeg_path = root.path().join("metadata.jpg");
        let jpeg = [
            &[0xFF, 0xD8][..],
            &[0xFF, 0xE1, 0x00, 0x06, 1, 2, 3, 4],
            &[0xFF, 0xFE, 0x00, 0x04, 5, 6],
            &[0xFF, 0xDB, 0x00, 0x04, 7, 8],
            &[0xFF, 0xDA, 0x00, 0x04, 9, 10, 11, 12],
            &[0xFF, 0xE2, 0x00, 0x04, 13, 14],
            &[0xFF, 0xDA, 0x00, 0x04, 15, 16, 17, 0xFF, 0xD9],
        ]
        .concat();
        std::fs::write(&jpeg_path, &jpeg)?;
        assert_eq!(
            super::jpeg_pure_media_size(&jpeg_path)?,
            u64::try_from(jpeg.len())? - 20
        );

        let png_path = root.path().join("metadata.png");
        let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        append_png_chunk(&mut png, *b"IHDR", &[0_u8; 13])?;
        append_png_chunk(&mut png, *b"tEXt", &[0_u8; 100])?;
        append_png_chunk(&mut png, *b"PLTE", &[0_u8; 3])?;
        append_png_chunk(&mut png, *b"tRNS", &[0_u8; 1])?;
        append_png_chunk(&mut png, *b"IDAT", &[0_u8; 5])?;
        append_png_chunk(&mut png, *b"IEND", &[])?;
        std::fs::write(&png_path, png)?;
        assert_eq!(super::png_pure_media_size(&png_path)?, 22);
        Ok(())
    }

    #[test]
    fn avif_pure_media_parser_counts_only_mdat_payloads() -> anyhow::Result<()> {
        fn append_box(
            bytes: &mut Vec<u8>,
            box_type: [u8; 4],
            payload: &[u8],
        ) -> anyhow::Result<()> {
            let size = u32::try_from(payload.len() + 8)?;
            bytes.extend_from_slice(&size.to_be_bytes());
            bytes.extend_from_slice(&box_type);
            bytes.extend_from_slice(payload);
            Ok(())
        }

        let root = TempDir::new()?;
        let avif_path = root.path().join("payload.avif");
        let mut avif = Vec::new();
        append_box(&mut avif, *b"ftyp", &[0_u8; 12])?;
        append_box(&mut avif, *b"meta", &[0_u8; 7])?;
        append_box(&mut avif, *b"mdat", &[0_u8; 11])?;
        avif.extend_from_slice(&1_u32.to_be_bytes());
        avif.extend_from_slice(b"mdat");
        avif.extend_from_slice(&19_u64.to_be_bytes());
        avif.extend_from_slice(&[0_u8; 3]);
        std::fs::write(&avif_path, avif)?;
        assert_eq!(super::avif_mdat_payload_size(&avif_path)?, 14);

        let truncated_path = root.path().join("truncated.avif");
        std::fs::write(
            &truncated_path,
            [20_u32.to_be_bytes().as_slice(), b"mdat", &[0_u8; 3]].concat(),
        )?;
        assert!(super::avif_mdat_payload_size(&truncated_path).is_err());
        Ok(())
    }

    #[test]
    fn gif_static_decodes_but_animation_and_corruption_fail_closed() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let static_gif = root.path().join("static.gif");
        let animated_gif = root.path().join("animated.gif");
        let corrupt_gif = root.path().join("corrupt.gif");

        {
            let mut encoder =
                image::codecs::gif::GifEncoder::new(std::fs::File::create(&static_gif)?);
            encoder.encode_frame(image::Frame::new(image::RgbaImage::new(1, 1)))?;
        }
        {
            let mut encoder =
                image::codecs::gif::GifEncoder::new(std::fs::File::create(&animated_gif)?);
            encoder.encode_frame(image::Frame::new(image::RgbaImage::new(1, 1)))?;
            encoder.encode_frame(image::Frame::new(image::RgbaImage::new(1, 1)))?;
        }
        std::fs::write(&corrupt_gif, b"GIF89a")?;

        assert!(super::fast_img_container_is_static(
            &static_gif,
            FormatKind::Gif
        )?);
        assert!(!super::fast_img_container_is_static(
            &animated_gif,
            FormatKind::Gif
        )?);
        assert!(
            super::fast_img_container_is_static(&corrupt_gif, FormatKind::Gif).is_err(),
            "damaged GIF must not reach ImageMagick's first-frame path"
        );
        Ok(())
    }

    #[test]
    fn malformed_gif_is_recorded_failed_before_fast_img_encode() -> anyhow::Result<()> {
        foundation::tools::require(&["avifenc", "avifdec"]).map_err(anyhow::Error::msg)?;
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_root = root.path().join("input");
        std::fs::create_dir_all(&src_root)?;
        let corrupt_gif = src_root.join("corrupt.gif");
        std::fs::write(&corrupt_gif, b"GIF89a")?;

        let run_error = run_fast_img(FastImgRunOptions {
            input: &src_root,
            output_dir: None,
            delete_source: DeleteSourceFlag(false),
            dry_run: DryRunFlag(false),
            recursive: RecursiveFlag(true),
            auto_import: AutoImportFlag(false),
            shortest_path: ShortestPathFlag(false),
            retry: RetryFlag(false),
            fresh: FreshFlag(false),
            archive: false,
            allow_expert_options: false,
            strategy: "avif",
            extreme_precision: false,
        })
        .expect_err("malformed GIF must remain an explicit failed source");

        assert!(
            run_error
                .to_string()
                .contains("All 1 static image(s) failed")
        );
        let working_copy = foundation::pipeline::verification::working_copy_dir(&src_root);
        let marker_dir = root.path().join("fast_img/markers");
        let marker_files = std::fs::read_dir(&marker_dir)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            marker_files.len(),
            1,
            "failed scan must leave one marker: {marker_files:?}"
        );
        let marker_json = std::fs::read_to_string(&marker_files[0])?;
        assert!(
            marker_json.contains("static-container metadata"),
            "scan failure must be recorded before any encoder fallback: {marker_json}"
        );
        assert!(corrupt_gif.is_file(), "malformed source must be retained");
        assert!(
            !working_copy.join("corrupt.AVIF").exists(),
            "malformed GIF must not produce an AVIF candidate"
        );
        Ok(())
    }

    #[test]
    fn mislabeled_png_completes_local_avif_encode_and_pixel_verification() -> anyhow::Result<()> {
        if foundation::tools::require(&["avifenc", "avifdec"]).is_err() {
            return Ok(());
        }
        let root = TempDir::new()?;
        let src_dir = root.path().join("src");
        let wc = root.path().join("wc");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&wc)?;
        let source = src_dir.join("qqcache.gif");
        image::DynamicImage::new_rgba8(16, 16)
            .save_with_format(&source, image::ImageFormat::Png)?;
        let job = super::FastImgTranscodeJob {
            source: source.clone(),
            src_hash: foundation::common_utils::calculate_blake3_hash(&source)?,
            rel_key: "qqcache.gif".to_string(),
            out_rel_key: "qqcache.AVIF".to_string(),
        };

        let outcome =
            super::fast_img_run_encode_job_inner(&job, &src_dir, &wc, 1, false, false, "avif")?;
        let super::FastImgTranscodeOutcome::Converted(proof) = outcome else {
            anyhow::bail!("mislabeled PNG should produce a verified AVIF output");
        };
        assert!(wc.join(proof.out_rel).is_file());
        Ok(())
    }

    #[test]
    fn test_fast_img_avif_stops_repeating_source_invariant_probe_errors() {
        assert!(super::avif_quality_probe_error_is_source_invariant(
            "AVIF pixel equivalence failed at q=90: pixel-diff: cannot open source image: malformed GIF header"
        ));
        assert!(super::avif_quality_probe_error_is_source_invariant(
            "avifenc failed at q=90: Unrecognized file format"
        ));
        assert!(super::avif_quality_probe_error_is_source_invariant(
            "avifenc failed at q=90: Unsupported file format AVIF"
        ));
        assert!(!super::avif_quality_probe_error_is_source_invariant(
            "official avifenc encode timed out after 120s"
        ));
        assert!(!super::avif_quality_probe_error_is_source_invariant(
            "official avifenc quality probe timed out after 120s"
        ));
        assert!(!super::avif_quality_probe_error_is_source_invariant(
            "AVIF health check failed at q=90: temporary I/O error"
        ));
    }

    #[test]
    fn fast_img_avif_only_explores_proven_source_semantics() {
        use foundation::image_detection::CompressionType;

        assert_eq!(
            super::fast_img_avif_source_is_lossless(CompressionType::Lossless),
            Some(true)
        );
        assert_eq!(
            super::fast_img_avif_source_is_lossless(CompressionType::Lossy),
            Some(false)
        );
        assert_eq!(
            super::fast_img_avif_source_is_lossless(CompressionType::Unknown),
            None
        );
        assert_eq!(
            super::fast_img_avif_source_is_lossless(CompressionType::JpegReconstruction),
            None
        );
    }

    #[test]
    fn fast_img_avif_terminal_error_keeps_last_verified_fitting_candidate() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let fitting_path = root.path().join("q95.AVIF");
        std::fs::write(&fitting_path, b"fitting")?;
        let mut fitting = Some(super::AvifMemeCandidate {
            speed_domain: super::avif_meme_speed_domain(),
            quality: 95,
            temp_path: fitting_path.clone(),
            output_size: 7,
            pure_media_size: 7,
            content_blake3: "fitting".to_string(),
        });

        let result = super::finish_avif_meme_after_terminal_probe_error(
            "avifenc failed at q=96: Unsupported file format AVIF",
            &mut fitting,
        )
        .expect("unsupported source format is terminal");
        let super::AvifQualityExploreResult::Found {
            quality,
            temp_path,
            selection,
            ..
        } = result
        else {
            anyhow::bail!("terminal source error must keep the last verified fitting candidate");
        };

        assert_eq!(quality, 95);
        assert_eq!(temp_path, fitting_path);
        assert_eq!(selection, "terminal_probe_fitting_fallback");
        Ok(())
    }

    #[test]
    fn fast_img_avif_terminal_error_without_verified_candidate_fails_closed() -> anyhow::Result<()>
    {
        let mut fitting = None;

        let result = super::finish_avif_meme_after_terminal_probe_error(
            "avifenc failed at q=100: Unsupported file format AVIF",
            &mut fitting,
        )
        .expect("unsupported source format is terminal");
        let super::AvifQualityExploreResult::SourceUnavailable { reason } = result else {
            anyhow::bail!("terminal error without verified candidate must fail closed");
        };

        assert!(reason.contains("Unsupported file format"));
        Ok(())
    }

    #[test]
    fn fast_img_avif_source_is_adopted_byte_exactly() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let source = root.path().join("source.AVIF");
        let output = root.path().join("working/nested/source.AVIF");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../foundation/tests/fixtures/metadata_clear_baseline.avif.fixture");
        std::fs::copy(&fixture, &source)?;
        let bytes = std::fs::read(&source)?;

        let result = super::fast_img_adopt_static_avif(&source, &output)?
            .expect("metadata-clean AVIF should be adopted");

        assert!(result.success);
        let output_display = output.display().to_string();
        assert_eq!(result.output_path.as_deref(), Some(output_display.as_str()));
        assert_eq!(std::fs::read(&output)?, bytes);
        Ok(())
    }

    #[test]
    fn fast_img_avif_source_with_metadata_is_reencoded_instead_of_adopted() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let source = root.path().join("source.AVIF");
        let output = root.path().join("working/source.AVIF");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../foundation/tests/fixtures/metadata_clear_baseline.avif.fixture");
        std::fs::copy(&fixture, &source)?;
        let metadata = foundation::ExiftoolBuilder::new()
            .arg("-XMP-dc:Description=private source metadata")
            .arg(foundation::path_safety::exiftool_path_arg(&source).as_ref())
            .build()
            .output()?;
        anyhow::ensure!(
            metadata.status.success(),
            "failed to write AVIF test metadata"
        );

        let result = super::fast_img_adopt_static_avif(&source, &output)?;

        assert!(result.is_none(), "metadata-bearing AVIF must be re-encoded");
        assert!(
            !output.exists(),
            "noncompliant AVIF must not be committed unchanged"
        );
        Ok(())
    }

    #[test]
    fn jxl_pure_bitstream_parser_excludes_container_metadata() -> anyhow::Result<()> {
        use foundation::infra::constants::{
            JXL_BOX_JXLP, JXL_CODESTREAM_MAGIC, JXL_CONTAINER_MAGIC,
        };
        fn append_jxl_box(
            bytes: &mut Vec<u8>,
            box_type: [u8; 4],
            payload: &[u8],
        ) -> anyhow::Result<()> {
            let size = u32::try_from(payload.len() + 8)?;
            bytes.extend_from_slice(&size.to_be_bytes());
            bytes.extend_from_slice(&box_type);
            bytes.extend_from_slice(payload);
            Ok(())
        }

        let root = TempDir::new()?;

        // Container JXL with ftyp, Exif, jxlp (codestream), and xml
        let container_jxl_path = root.path().join("container.jxl");
        let mut jxl_bytes = Vec::new();
        // Container signature box (12 bytes)
        jxl_bytes.extend_from_slice(JXL_CONTAINER_MAGIC);
        append_jxl_box(&mut jxl_bytes, *b"ftyp", b"jxl \x00\x00\x00\x00")?;
        append_jxl_box(&mut jxl_bytes, *b"Exif", &[0_u8; 50])?;
        // jxlp box: 4-byte sequence index + 100 bytes codestream fragment
        let mut jxlp_payload = Vec::new();
        jxlp_payload.extend_from_slice(&0_u32.to_be_bytes()); // index 0
        jxlp_payload.extend_from_slice(&[0xAB_u8; 100]); // 100 payload bytes
        append_jxl_box(&mut jxl_bytes, *JXL_BOX_JXLP, &jxlp_payload)?;
        append_jxl_box(&mut jxl_bytes, *b"xml ", &[0_u8; 30])?;
        std::fs::write(&container_jxl_path, &jxl_bytes)?;

        assert_eq!(
            super::jxl_pure_bitstream_size(&container_jxl_path)?,
            100,
            "JXL pure bitstream parser must return 100 (jxlp payload minus 4-byte sequence header)"
        );

        // Naked JXL codestream (starts with FF 0A)
        let naked_jxl_path = root.path().join("naked.jxl");
        let naked_bytes = [
            JXL_CODESTREAM_MAGIC[0],
            JXL_CODESTREAM_MAGIC[1],
            0x12,
            0x34,
            0x56,
            0x78,
        ];
        std::fs::write(&naked_jxl_path, naked_bytes)?;
        assert_eq!(
            super::jxl_pure_bitstream_size(&naked_jxl_path)?,
            u64::try_from(naked_bytes.len())?,
            "Naked JXL codestream size must equal file size"
        );

        Ok(())
    }

    #[test]
    fn jpeg_pure_media_size_handles_restart_markers_correctly() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let path = root.path().join("restart.jpg");
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xFF, 0xD8]);
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x07, b'J', b'F', b'I', b'F', 0x00]);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x03, 0x01, 0x00]);
        jpeg.extend_from_slice(&[0x11, 0x22, 0xFF, 0xD0, 0x33, 0x44, 0xFF, 0xD1, 0x55]);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);

        std::fs::write(&path, &jpeg)?;

        let pure_size = super::jpeg_pure_media_size(&path)?;
        assert_eq!(pure_size, 19);
        Ok(())
    }

    #[test]
    fn meme_avif_evidence_prefers_higher_fitting_quality_not_smaller_size() {
        let mut evidence = super::AvifMemeQualityEvidence::default();
        let candidate = |quality| super::AvifMemeCandidate {
            speed_domain: super::avif_meme_speed_domain(),
            quality,
            temp_path: std::path::PathBuf::from(format!("q{quality}.avif")),
            output_size: u64::from(quality),
            pure_media_size: u64::from(quality),
            content_blake3: quality.to_string(),
        };
        let q70 = candidate(70);
        let q90 = candidate(90);
        evidence.record_fit(&q70);
        evidence.record_fit(&q90);
        assert_eq!(evidence.highest_fitting_quality, Some(90));
    }

    #[test]
    fn fast_img_planned_output_rel_uses_strategy_extension_and_removes_failed_outputs()
    -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("wc");
        std::fs::create_dir_all(&wc)?;
        let src = root.path().join("test.gif");
        std::fs::write(&src, b"bad_gif")?;

        let (_, out_jxl) =
            super::fast_img_planned_output_rel(&src, &wc, Path::new("test.gif"), "hevc")?;
        assert_eq!(out_jxl, "test.JXL");

        let (_, out_avif) =
            super::fast_img_planned_output_rel(&src, &wc, Path::new("test.gif"), "avif")?;
        assert_eq!(out_avif, "test.AVIF");

        // Simulate failed transcode residue cleanup
        let avif_path = wc.join("test.AVIF");
        let jxl_path = wc.join("test.JXL");
        std::fs::write(&avif_path, b"corrupted")?;
        std::fs::write(&jxl_path, b"corrupted")?;

        let err = super::FastImgTranscodeError {
            rel_key: "test.gif".to_string(),
            out_rel_key: out_avif,
            src_hash: "hash".to_string(),
            reason: "malformed gif".to_string(),
        };

        super::fast_img_remove_failed_encode_output(&wc, &err)?;
        assert!(!avif_path.exists(), "target output AVIF must be removed");
        assert!(jxl_path.exists(), "unrelated JXL must not be deleted");

        Ok(())
    }

    #[test]
    fn test_avif_matching_hash_reusable() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let output = root.path().join("photo.AVIF");
        std::fs::write(&output, b"avif_data")?;
        let hash = foundation::common_utils::calculate_blake3_hash(&output)?;

        let res = super::fast_img_check_reused_delivery(
            &output,
            "avif",
            super::FAST_IMG_AVIF_CLEAN_POLICY_VERSION,
            &hash,
        )?;

        match res {
            super::ReuseDecision::Reusable {
                hash: returned_hash,
            } => {
                assert_eq!(returned_hash, hash);
            }
            super::ReuseDecision::NeedsReencode { reason } => {
                panic!("expected Reusable, got NeedsReencode: {reason}");
            }
        }
        Ok(())
    }

    #[test]
    fn test_avif_hash_drift_forces_reencode() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_dir = root.path().join("src");
        let wc = root.path().join("wc");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&wc)?;

        let source = src_dir.join("photo.jpg");
        let output = wc.join("photo.AVIF");
        std::fs::write(&source, b"source_data")?;
        std::fs::write(&output, b"tampered_avif_data")?;

        let res = super::fast_img_check_reused_delivery(
            &output,
            "avif",
            super::FAST_IMG_AVIF_CLEAN_POLICY_VERSION,
            "expected_recorded_hash_A",
        )?;
        assert!(matches!(res, super::ReuseDecision::NeedsReencode { .. }));

        let mut marker = WorkingCopyMarker::new(src_dir.clone(), wc, 1);
        marker.strategy = "avif".to_string();
        marker.metadata_policy_version = super::FAST_IMG_AVIF_CLEAN_POLICY_VERSION;
        marker.stage = FastImgStageName::TranscodeComplete;
        marker.blake3_log.insert(
            "photo.jpg".to_string(),
            foundation::pipeline::verification::Blake3Entry {
                src: "hash123".to_string(),
                out: "expected_recorded_hash_A".to_string(),
                out_rel: Some("photo.AVIF".to_string()),
                library_asset: None,
            },
        );

        let summary =
            super::fast_img_refresh_marker_deliveries(&mut marker, &src_dir, "avif", false)?;
        assert_eq!(summary.refreshed, 0);
        assert_eq!(summary.invalidated, 1);
        assert!(summary.marker_changed);

        assert_eq!(marker.stage, FastImgStageName::OutputPrepared);
        let entry = marker.blake3_log.get("photo.jpg").unwrap();
        assert!(
            entry.out.is_empty(),
            "entry.out must be cleared on hash drift"
        );
        assert_eq!(entry.out_rel.as_deref(), Some("photo.AVIF"));
        assert!(!marker.skipped_sources.contains_key("photo.jpg"));
        assert!(!output.exists(), "old tampered output must be deleted");
        Ok(())
    }

    #[test]
    fn test_avif_marker_entry_output_path_fallback() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("wc");
        let mut marker = WorkingCopyMarker::new(root.path().to_path_buf(), wc.clone(), 1);
        marker.strategy = "avif".to_string();

        let entry = foundation::pipeline::verification::Blake3Entry {
            src: "hash".to_string(),
            out: "out".to_string(),
            out_rel: None,
            library_asset: None,
        };

        let path = super::fast_img_marker_entry_output_path(&marker, "photo.jpg", &entry)?;
        assert_eq!(path, wc.join("photo.AVIF"));

        marker.strategy = "hevc".to_string();
        let path_jxl = super::fast_img_marker_entry_output_path(&marker, "photo.jpg", &entry)?;
        assert_eq!(path_jxl, wc.join("photo.JXL"));
        Ok(())
    }

    #[test]
    fn test_jxl_refresh_unchanged_keeps_stage() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_dir = root.path().join("src");
        let wc = root.path().join("wc");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&wc)?;
        let source = src_dir.join("photo.jpg");
        let output = wc.join("photo.JXL");
        std::fs::write(&source, b"source")?;
        std::fs::write(&output, b"jxl")?;
        let output_hash = foundation::common_utils::calculate_blake3_hash(&output)?;

        let mut marker = WorkingCopyMarker::new(src_dir.clone(), wc, 1);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.gate1_checks.count = foundation::pipeline::verification::CheckPassed(true);
        marker.blake3_log.insert(
            "photo.jpg".to_string(),
            foundation::pipeline::verification::Blake3Entry {
                src: foundation::common_utils::calculate_blake3_hash(&source)?,
                out: output_hash,
                out_rel: Some("photo.JXL".to_string()),
                library_asset: Some("asset_123".to_string()),
            },
        );

        let summary =
            super::fast_img_refresh_marker_deliveries(&mut marker, &src_dir, "hevc", false)?;
        assert_eq!(summary.refreshed, 1);
        assert_eq!(summary.invalidated, 0);
        assert!(!summary.marker_changed);
        assert_eq!(marker.stage, FastImgStageName::Gate1Passed);
        assert_eq!(
            marker.gate1_checks.count,
            foundation::pipeline::verification::CheckPassed(true)
        );
        assert_eq!(
            marker
                .blake3_log
                .get("photo.jpg")
                .and_then(|entry| entry.library_asset.as_deref()),
            Some("asset_123")
        );
        Ok(())
    }

    #[test]
    fn test_missing_source_preserves_stale_output_and_marker_proof() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let src_dir = root.path().join("src");
        let wc = root.path().join("wc");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&wc)?;

        let output = wc.join("photo.AVIF");
        std::fs::write(&output, b"stale_avif")?;

        let mut marker = WorkingCopyMarker::new(src_dir.clone(), wc, 1);
        marker.strategy = "avif".to_string();
        marker.metadata_policy_version = super::FAST_IMG_AVIF_CLEAN_POLICY_VERSION;
        marker.stage = FastImgStageName::TranscodeComplete;
        marker.blake3_log.insert(
            "photo.jpg".to_string(),
            foundation::pipeline::verification::Blake3Entry {
                src: "src_hash".to_string(),
                out: "recorded_hash".to_string(),
                out_rel: Some("photo.AVIF".to_string()),
                library_asset: Some("asset_id".to_string()),
            },
        );
        let before = marker.clone();

        let err = super::fast_img_refresh_marker_deliveries(&mut marker, &src_dir, "avif", false)
            .expect_err("missing source must stop before destructive invalidation");
        assert!(err.to_string().contains("source is missing"));
        assert!(
            err.to_string()
                .contains("preserving output and marker proof")
        );
        assert!(output.exists());
        assert_eq!(marker.blake3_log, before.blake3_log);
        assert_eq!(marker.stage, before.stage);
        Ok(())
    }

    #[test]
    fn test_resume_preserves_valid_entries_and_only_requeues_drifted_output() -> anyhow::Result<()>
    {
        let root = TempDir::new()?;
        let _env = fast_img_marker_state_test_env(root.path());
        let src_dir = root.path().join("src");
        let wc = root.path().join("wc");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&wc)?;

        let source_a = src_dir.join("a.png");
        let source_b = src_dir.join("b.png");
        let skipped_source = src_dir.join("skip.gif");
        let output_a = wc.join("a.AVIF");
        let output_b = wc.join("b.AVIF");
        std::fs::write(&source_a, b"source-a")?;
        std::fs::write(&source_b, b"source-b")?;
        std::fs::write(&skipped_source, b"source-skip")?;
        std::fs::write(&output_a, b"output-a")?;
        std::fs::write(&output_b, b"drifted-output-b")?;

        let output_a_hash = foundation::common_utils::calculate_blake3_hash(&output_a)?;
        let mut marker = WorkingCopyMarker::new(src_dir.clone(), wc, 3);
        marker.strategy = "avif".to_string();
        marker.metadata_policy_version = super::FAST_IMG_AVIF_CLEAN_POLICY_VERSION;
        marker.stage = FastImgStageName::TranscodeComplete;
        marker.blake3_log.insert(
            "a.png".to_string(),
            foundation::pipeline::verification::Blake3Entry {
                src: foundation::common_utils::calculate_blake3_hash(&source_a)?,
                out: output_a_hash.clone(),
                out_rel: Some("a.AVIF".to_string()),
                library_asset: Some("asset-a".to_string()),
            },
        );
        marker.blake3_log.insert(
            "b.png".to_string(),
            foundation::pipeline::verification::Blake3Entry {
                src: foundation::common_utils::calculate_blake3_hash(&source_b)?,
                out: "recorded-output-b".to_string(),
                out_rel: Some("b.AVIF".to_string()),
                library_asset: Some("asset-b".to_string()),
            },
        );
        marker.skipped_sources.insert(
            "skip.gif".to_string(),
            SkippedSourceEntry {
                src: foundation::common_utils::calculate_blake3_hash(&skipped_source)?,
                reason: "animated source".to_string(),
            },
        );

        let mut resume_stage = FastImgStageName::TranscodeComplete;
        assert!(super::fast_img_downgrade_resume_if_outputs_stale(
            &mut marker,
            &mut resume_stage
        )?);
        let summary = super::fast_img_refresh_and_persist_marker_deliveries(
            &mut marker,
            &src_dir,
            "avif",
            false,
        )?;
        assert_eq!(summary.invalidated, 1);

        let persisted = super::read_marker(&marker.working_copy)?;
        let entry_a = persisted
            .blake3_log
            .get("a.png")
            .ok_or_else(|| anyhow::anyhow!("missing valid entry A"))?;
        let entry_b = persisted
            .blake3_log
            .get("b.png")
            .ok_or_else(|| anyhow::anyhow!("missing drifted entry B"))?;
        assert_eq!(entry_a.out, output_a_hash);
        assert_eq!(entry_a.out_rel.as_deref(), Some("a.AVIF"));
        assert_eq!(entry_b.out, "");
        assert_eq!(entry_b.out_rel.as_deref(), Some("b.AVIF"));
        assert!(persisted.skipped_sources.contains_key("skip.gif"));
        assert_eq!(persisted.blake3_log.len(), 2);
        assert!(output_a.exists());
        assert!(!output_b.exists());
        assert!(super::fast_img_skip_hashes_match(
            &source_a, &output_a, entry_a
        )?);
        Ok(())
    }

    #[test]
    fn test_metadata_policy_version_written_on_avif_transcode_complete() {
        let mut marker = WorkingCopyMarker::new(PathBuf::from("/src"), PathBuf::from("/wc"), 1);
        marker.metadata_policy_version = 0;

        super::fast_img_commit_transcode_complete(&mut marker, "avif");

        assert_eq!(
            marker.metadata_policy_version,
            super::FAST_IMG_AVIF_CLEAN_POLICY_VERSION
        );
        assert_eq!(marker.stage, FastImgStageName::TranscodeComplete);

        let mut jxl_marker = WorkingCopyMarker::new(PathBuf::from("/src"), PathBuf::from("/wc"), 1);
        jxl_marker.metadata_policy_version = 0;
        super::fast_img_commit_transcode_complete(&mut jxl_marker, "hevc");

        assert_eq!(
            jxl_marker.metadata_policy_version, 0,
            "JXL strategy must NOT write AVIF policy version"
        );
    }

    #[test]
    fn test_exact_failed_cleanup_does_not_delete_case_variant() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("wc");
        std::fs::create_dir_all(wc.join("target"))?;
        std::fs::create_dir_all(wc.join("sibling"))?;

        let target = wc.join("target/image.avif");
        let sibling = wc.join("sibling/image.AVIF");
        std::fs::write(&target, b"failed_target")?;
        std::fs::write(&sibling, b"sibling_file")?;

        let err = super::FastImgTranscodeError {
            rel_key: "image.jpg".to_string(),
            out_rel_key: "target/image.avif".to_string(),
            src_hash: "hash".to_string(),
            reason: "encode failed".to_string(),
        };

        super::fast_img_remove_failed_encode_output(&wc, &err)?;
        assert!(!target.exists(), "exact out_rel_key must be removed");
        assert!(
            sibling.exists(),
            "unrelated sibling file must NOT be removed"
        );
        Ok(())
    }

    #[test]
    fn test_imagemagick_identify_fail_closed_on_error() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let source = root.path().join("non_existent.png");
        let temp_path = root.path().join("temp.png");

        let res = super::run_avif_input_decoder(
            super::AvifInputDecoder::ImageMagick,
            &source,
            &temp_path,
        );
        assert!(
            res.is_err(),
            "ImageMagick identify preflight must fail-closed on missing file/error"
        );
        Ok(())
    }
}
