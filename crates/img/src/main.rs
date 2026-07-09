#![allow(clippy::too_many_lines)]

use anyhow::Context;
use clap::{Parser, Subcommand};
use img::Rational;
use img::lossless_converter::{
    ConvertFlags as LosslessConvertFlags, ConvertOptions as LosslessConvertOptions,
    convert_jpeg_to_jxl,
};

use core::sync::atomic::{AtomicUsize, Ordering};
use foundation::ToolBuilder;
use foundation::analysis_cache::AnalysisCache;
use foundation::common_utils::calculate_blake3_hash;
use foundation::fast_img::{
    IntegrityResult, apply_library_assets_to_marker, apply_tier2_library_assets_to_marker,
    delete_verified_modern_lossy_static_sources, import_jxl_outputs_with_library_verifier,
    import_modern_lossy_static_tier, is_true_jpeg, library_handle_from_marker_import_proof,
    library_handle_from_marker_tier2_proof, prompt_user_confirm,
    prune_empty_source_dirs_for_tier2_assets, safe_delete_jpeg_source,
    safe_delete_matching_xmp_sidecar, verify_final_jxl_delivery_integrity,
};
use foundation::image::format_detect::FormatKind;
use foundation::image::orientation::{
    PixelDiffResult, orientation_diff_tolerance_for_format, verify_orientation_pixel_diff,
};
use foundation::modern_lossy_static::ModernLossyStaticCandidate;
use foundation::modern_ui::{colors, symbols};
use foundation::pipeline::verification::{
    Blake3Entry, FastImgStageName, Gate1Checks, Gate1Local, Gate2Checks, Gate2Import, Gate3Checks,
    Gate3Deep, LibraryHandle, PipelineCtx, SkippedSourceEntry, VerificationGate, WorkingCopyMarker,
    confirm_import_required, deep_scan_complete_or_later, gate1_complete_or_later,
    gate2_complete_or_later, gate3_complete_or_later, import_complete_or_later,
    marker_checks_from_result, marker_path_for_working_copy, output_prepared_or_later,
    prepare_jxl_output_dir, read_marker, resolve_working_copy_dir, retry_resume_stage,
    transcode_complete_or_later, write_marker_atomic,
};
use foundation::quality_matcher::SourceCodec;
use foundation::scan_modern_lossy_static_candidates;
use foundation::{
    PauseController, Summary, check_dangerous_directory, disk_full_pause_reason, log_detail,
    log_failure, log_fatal, log_hint, log_skip, log_stat, log_success, log_summary_header,
    print_summary,
};
use img::{
    ConfigFlags, calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description,
};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
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

        /// Archive mode: hard-overrides encoder effort/presets for maximum compression.
        #[arg(long, default_value_t = false)]
        archive: bool,

        #[arg(long, default_value_t = false)]
        allow_size_tolerance: bool,

        #[arg(long)]
        no_allow_size_tolerance: bool,

        /// Enable expert/lab-only encoder parameters. Required before JPEG lossless transcode may test cjxl e11.
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

        /// Static still delivery: `hevc`→JXL (default), `av1`→AVIF. Does not invoke `vid`; animated files are ignored.
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

    /// Fast JPEG-only transcode: true JPEGs → adjacent JXL-only output.
    ///
    /// Detects true JPEGs via magic bytes (never extension-only), strips residual
    /// EXIF Orientation tag post-encode, deletes verified source JPEGs, and
    /// optionally imports verified JXLs to Photos/iCloud in shortest-path mode.
    ///
    /// Locked decisions: D1=Photos import required, D2=abort on delete failure,
    /// D3=pixel-diff plus tag assert, D4=Rust-only, D5=subcommand,
    /// D6=verified source delete mandatory, D7=JPEG-path-only detection.
    #[command(name = "fast-img")]
    FastImg {
        /// Input directory (or single file) containing source JPEGs.
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Deprecated for Rev2 fast mode; working-copy output is derived from input.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Deprecated for Rev2 fast mode; verified source JPEG cleanup is mandatory.
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

        /// Enable shortest-path Photos import workflow after the shared JXL-only local output passes Gate 1.
        #[arg(long = "shortest-path", default_value_t = false)]
        shortest_path: bool,

        /// Archive mode: JPEG → JXL uses cjxl effort 11.
        #[arg(long, default_value_t = false)]
        archive: bool,

        /// Retry a previous `gate*_failed` working-copy marker.
        #[arg(long, default_value_t = false)]
        retry: bool,

        /// Enable expert/lab-only encoder parameters. Required before JPEG lossless transcode may test cjxl e11.
        #[arg(long = "allow_expert_options", default_value_t = false)]
        allow_expert_options: bool,
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
    },
}

const fn command_requires_database(command: &Commands) -> bool {
    !matches!(
        command,
        Commands::FastImg { .. } | Commands::RestoreJpeg { .. }
    )
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
fn main() -> anyhow::Result<()> {
    foundation::entry_guard::assert_product_cli_entry("img").context("img entry guard")?;
    foundation::init_ghost_mode().context("Failed to initialize ghost mode")?;

    foundation::logging::init("img", &foundation::logging::LogConfig::default())
        .map_err(|e| e.context("Failed to initialize img logging"))?;

    let cli = Cli::parse();

    // Initialize Ctrl+C guard for long-running batch operations
    foundation::ctrlc_guard::init();

    let cache = if command_requires_database(&cli.command) {
        // Enforce PostgreSQL dependency as mandatory for the full image toolchain.
        // Fast JPEG-only mode is intentionally excluded: it only scans true JPEGs,
        // writes verified JXL outputs, and does not need DB/cache/probe state.
        if let Err(e) = foundation::database::open_pg_client() {
            foundation::log_fatal!(
                "Infrastructure",
                &format!(
                    "PostgreSQL database is mandatory for full feature availability. Connection failed: {e}"
                )
            );
            std::process::exit(foundation::constants::EXIT_CODE_ERROR);
        }

        let cache = match AnalysisCache::default_local() {
            Ok(cache) => Some(Arc::new(cache)),
            Err(e) => {
                foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                    "analysis_cache_unavailable",
                    format!("failed to initialize persistent cache: {e}"),
                );
                None
            }
        };

        if let Some(ref cache) = cache {
            match cache.cleanup_old_records(foundation::constants::CACHE_PRUNE_AGE_SECS) {
                Ok(removed) if removed > 0 => {
                    foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                        "analysis_cache_age_prune_completed",
                        format!("removed={removed}"),
                    );
                }
                Err(e) => {
                    foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                        "analysis_cache_age_prune_failed",
                        format!("failed to prune aged cache rows: {e}"),
                    );
                }
                Ok(_) => {}
            }
        }
        cache
    } else {
        None
    };

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

    let _lock_guard = match input_to_lock {
        None => None,
        Some(input) => {
            let input_abs = foundation::media_conversion_gate::canonicalize_for_tool_input(input);
            if input_abs.is_dir() {
                match foundation::acquire_dir_lock(&input_abs) {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        log_fatal!(
                            foundation::infra::static_logs::messages::LABEL_LOCK,
                            &e.to_string()
                        );
                        std::process::exit(foundation::constants::EXIT_CODE_LOCK_FAILURE);
                    }
                }
            } else {
                None
            }
        }
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

            let resume = resume_flag && !no_resume;
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let should_delete = delete_original || in_place;

            let img_static_delivery = match resolve_cli_img_static_delivery(&codec, apple_compat) {
                Ok(d) => d,
                Err(e) => {
                    log_fatal!(
                        foundation::infra::static_logs::messages::LABEL_CONFIG,
                        &e.to_string(),
                    );
                    std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                }
            };

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
                    "Expert Options Audit: lab-only encoder parameters enabled; JPEG lossless transcode may test cjxl e11"
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

            let config = build_auto_convert_config(
                output,
                base_dir,
                force,
                should_delete,
                preserve_timestamps,
                preserve,
                compress,
                apple_compat,
                in_place,
                explore,
                match_quality,
                ultimate,
                archive,
                allow_size_tolerance,
                allow_expert_options,
                verbose,
                cache.clone(),
                img_static_delivery,
            );

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
            allow_expert_options,
        } => {
            run_fast_img(FastImgRunOptions {
                input: &input,
                output_dir: output.as_deref(),
                delete_source: DeleteSourceFlag(delete_source),
                dry_run: DryRunFlag(dry_run),
                recursive: RecursiveFlag(recursive),
                auto_import: AutoImportFlag(auto_import),
                shortest_path: ShortestPathFlag(shortest_path),
                retry: RetryFlag(retry),
                archive,
                allow_expert_options,
            })?;
        }
        Commands::RestoreJpeg {
            input,
            output,
            recursive,
            force,
        } => {
            run_restore_jpeg(&input, output.as_deref(), recursive, force)?;
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

    let Ok(compression) = foundation::image_detection::detect_compression(&detected_format, input)
    else {
        return Ok(None);
    };

    let skip = foundation::should_skip_image_format(
        detected_format.as_str(),
        compression == CompressionType::Lossless,
    );
    if !skip.should_skip {
        return Ok(None);
    }

    foundation::progress_mode::image_skipped(input, &skip.reason);
    copy_original_if_adjacent_mode(input, config)?;
    Ok(Some(ConversionOutput {
        original_path: input.display().to_string(),
        output_path: input.display().to_string(),
        skipped: true,
        ignored: false,
        message: skip.reason,
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

fn convert_result_to_output(result: foundation::TaskResult) -> ConversionOutput {
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
    ConversionOutput {
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
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
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

    let output = convert_result_to_output(result);

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

    // 🔬 Level 4 Feedback: KNN Static Quality Score
    // JPEG bypass: cjxl transcode is fast enough to skip DB lookup.
    // Returns a BPP heuristic (confidence=0.0) when DB is unavailable.
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

    Ok(match (format, is_lossless) {
        ("PNG", _) if foundation::is_true_png(input).unwrap_or(false) => {
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
                let q = quality.map(|q| {
                    foundation::numeric_cast::f64_to_u8_sat((q.score * 100.0).clamp(0.0, 100.0))
                });
                if config.verbose() {
                    foundation::log_detail!(&format!(
                        "{} Lossy→AVIF: {}",
                        foundation::infra::static_logs::messages::LABEL_DONE,
                        input.display()
                    ));
                }
                return Ok(img::lossless_converter::convert_to_avif(input, q, options)?);
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

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
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
            "  Queue Strategy: deeper paths → fast JPEG/direct transcodes → smaller files → lower resolution",
        );
    }

    // Initialize checkpoint manager for resume/progress tracking
    let checkpoint = if resume {
        match foundation::checkpoint::Manager::new_resuming_with_context(
            input,
            config.output_dir.as_deref(),
        ) {
            Ok(cp) => {
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
            }
            Err(e) => {
                if config.verbose() {
                    foundation::media_conversion_gate::delivery_jxl_batch_fallback_audit(
                        "checkpoint_init_failed",
                        format!("failed to initialize checkpoint: {e}"),
                    );
                }
                None
            }
        }
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
                                actual_input_bytes.fetch_add(result.original_size, Ordering::Relaxed);
                                if let Some(out_size) = result.output_size {
                                    actual_output_bytes.fetch_add(out_size, Ordering::Relaxed);
                                }
                                // Mark as completed in checkpoint manager on success (thread-safe)
                                if let Some(cp) = checkpoint.as_ref()
                                    && let Err(e) = cp.mark_completed(path)
                                {
                                    foundation::media_conversion_gate::delivery_api_path_fallback_audit(
                                        "checkpoint_mark_completed",
                                        path,
                                        format!("failed to mark completed: {e}"),
                                    );
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

                            let is_skip = match e
                                .downcast_ref::<foundation::unified_error::UnifiedError>()
                            {
                                // String fallback: only the controlled "Skipped:" sentinel prefix
                                // counts as a skip. Substring matches (e.g. a hard error merely
                                // containing "Skipped" or "already optimized") are failures.
                                None => err_str.starts_with("Skipped:"),
                                Some(ue) => ue.is_skip(),
                            };

                            if is_skip {
                                foundation::progress_mode::image_skipped(path, &err_str);
                                skipped.fetch_add(1, Ordering::Relaxed);
                                foundation::progress_mode::image_processed_success(); // Skip with copy is a partial success

                                // Copy original file to output directory to prevent data loss for skips
                                if let Some(ref output_dir) = config.output_dir
                                    && let Err(copy_err) = foundation::copy_on_skip_or_fail(
                                        path,
                                        Some(output_dir),
                                        config.base_dir.as_deref(),
                                        config.verbose(),
                                    )
                                {
                                    log_fatal!(
                                        "Fatal Integrity Violation",
                                        &format!(
                                            "Critical Data Link failure after skip ({}): {}. DATA LOSS RISK!",
                                            path.display(),
                                            copy_err
                                        )
                                    );
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
                                if let Ok(mut v) = failed_paths.lock() {
                                    v.push((path.clone(), err_str.clone()));
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

    if !result.paused
        && let Some(ref output_dir) = config.output_dir
    {
        log_detail!("");
        foundation::log_static!(
            info,
            foundation::infra::static_logs::messages::COPYING_UNSUPPORTED
        );
        let copy_result = foundation::copy_unsupported_files(
            foundation::media_conversion_gate::base_dir_or_default(
                config.base_dir.as_deref(),
                "copy_unsupported_base",
            ),
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
        } else if failed_count == 0 {
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
        if let Ok(paths) = failed_paths.lock() {
            for (p, reason) in paths.iter() {
                foundation::log_auto_error!("Failed file", "{}: {}", p.display(), reason);
            }
        }
        anyhow::bail!("Batch completed with {failed_count} failed file(s)");
    }

    Ok(())
}

/// Fast JPEG-only batch pipeline: adjacent JXL-only delivery, verified source delete, optional Photos gates.
#[allow(clippy::too_many_lines)]
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
struct FastImgRunOptions<'a> {
    input: &'a Path,
    output_dir: Option<&'a Path>,
    delete_source: DeleteSourceFlag,
    dry_run: DryRunFlag,
    recursive: RecursiveFlag,
    auto_import: AutoImportFlag,
    shortest_path: ShortestPathFlag,
    retry: RetryFlag,
    archive: bool,
    allow_expert_options: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FastImgPostGate1Policy {
    JxlOnlyDelivery,
    ShortestPathImportAndVerify,
}

fn run_fast_img(options: FastImgRunOptions<'_>) -> anyhow::Result<()> {
    let FastImgRunOptions {
        input,
        output_dir,
        delete_source,
        dry_run,
        recursive,
        auto_import,
        shortest_path,
        retry,
        archive,
        allow_expert_options,
    } = options;

    if let Some(output_dir) = output_dir {
        anyhow::bail!(
            "--output is not supported by Rev2 fast-img; adjacent JXL output is fixed by source path ({} ignored)",
            output_dir.display()
        );
    }
    if delete_source.0 {
        tracing::warn!(
            target: "fast_img",
            "--delete-source is redundant; Rev2 fast-img always deletes verified source JPEGs"
        );
    }
    if auto_import.0 && !shortest_path.0 {
        anyhow::bail!("--auto-import requires --shortest-path");
    }

    let input_plan = FastImgInputPlan::from_input(input, recursive.0)?;
    let src_dir = input_plan.src_root;
    let _source_lock = foundation::acquire_dir_lock(&src_dir).with_context(|| {
        format!(
            "fast-img could not acquire exclusive lock for {}",
            src_dir.display()
        )
    })?;
    let working_copy = resolve_working_copy_dir(&src_dir);

    let mut existing_marker = read_existing_fast_img_marker(&working_copy)?;
    println!("[SCAN    ] scanning true JPEGs in {}", src_dir.display());
    let lossy_modern_static_candidates =
        scan_modern_lossy_static_candidates(&src_dir, &input_plan.candidates)?;
    let mut source_jpegs = Vec::new();
    for path in input_plan.candidates {
        if is_true_jpeg(&path)? {
            source_jpegs.push(path);
        }
    }
    let current_source_hashes = fast_img_source_hash_set(&src_dir, &source_jpegs)?;
    println!(
        "[SCAN    ] Found {} true JPEGs in {}",
        source_jpegs.len(),
        src_dir.display()
    );
    if !lossy_modern_static_candidates.is_empty() {
        println!(
            "[SCAN    ] Found {} lossy modern static image(s) eligible for tier-2 Photos import",
            lossy_modern_static_candidates.len()
        );
    }

    if let Some(marker) = &existing_marker
        && retry.0
        && foundation::pipeline::verification::stage_requires_retry(&marker.stage)
        && fast_img_retry_marker_source_set_is_stale(marker, &src_dir, source_jpegs.len())
    {
        println!(
            "[RESUME  ] existing {} marker is stale for the current source set; rebuilding from current sources",
            marker.stage.as_str()
        );
        tracing::warn!(
            target: "fast_img",
            stage = %marker.stage.as_str(),
            working_copy = %working_copy.display(),
            marker_count = marker.src_jpeg_count,
            source_count = source_jpegs.len(),
            "fast-img retry marker ignored because current source JPEGs no longer match failed run"
        );
        existing_marker = None;
    }

    if let Some(marker) = &existing_marker
        && marker.stage == FastImgStageName::CleanupComplete
    {
        match fast_img_cleanup_complete_source_state(
            marker,
            source_jpegs.len(),
            &current_source_hashes,
        ) {
            Ok(FastImgCleanupCompleteSourceState::RestoredOriginal) => {
                println!(
                    "[RESUME  ] existing cleanup marker belongs to a completed run, but original source JPEGs were restored; rebuilding from restored sources"
                );
                tracing::warn!(
                    target: "fast_img",
                    working_copy = %working_copy.display(),
                    source_count = source_jpegs.len(),
                    "fast-img cleanup marker ignored because original source JPEGs were restored after cleanup"
                );
                existing_marker = None;
            }
            Ok(FastImgCleanupCompleteSourceState::DeletedConverted) => {}
            Ok(FastImgCleanupCompleteSourceState::StaleCurrent) => {
                println!(
                    "[RESUME  ] existing cleanup marker is stale for the current source set; rebuilding from current sources"
                );
                tracing::warn!(
                    target: "fast_img",
                    working_copy = %working_copy.display(),
                    source_count = source_jpegs.len(),
                    "fast-img cleanup marker ignored because current source JPEGs no longer match the completed run"
                );
                existing_marker = None;
            }
            Err(err) => return Err(err),
        }
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
        if !retry.0 {
            anyhow::bail!(
                "fast-img previous cleanup completed with {} failed source(s); rerun with --retry to retry retained source JPEGs",
                marker.failed_sources.len()
            );
        }
        println!(
            "[RESUME  ] existing cleanup marker contains {} failed source(s); retrying retained source JPEGs",
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
                "[RESUME  ] existing cleanup marker has missing/drifted JXL output; rebuilding from source JPEGs"
            );
            tracing::warn!(
                target: "fast_img",
                working_copy = %working_copy.display(),
                "fast-img cleanup marker output proof is not current; rebuilding because source JPEGs still exist"
            );
            false
        } else if resume_local_delivery_for_shortest_path {
            println!(
                "[RESUME  ] existing JXL-only delivery will continue to shortest-path Photos import"
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
        && !retry.0
        && !fast_img_auto_retry_failed_marker(marker)
    {
        anyhow::bail!(
            "fast-img previous run stopped at {}; inspect {} or rerun with --retry",
            marker.stage.as_str(),
            working_copy.display()
        );
    }
    if let Some(marker) = &existing_marker
        && fast_img_auto_retry_failed_marker(marker)
        && !retry.0
    {
        tracing::info!(
            target: "fast_img",
            stage = %marker.stage.as_str(),
            working_copy = %working_copy.display(),
            "fast-img auto-retrying failed marker after source-state validation"
        );
        println!(
            "[RESUME  ] previous {} marker will be retried after source-state validation",
            marker.stage.as_str()
        );
    }

    if dry_run.0 {
        println!(
            "[DRY-RUN ] would transcode {} JPEGs from {} into JXL-only output {}",
            source_jpegs.len(),
            src_dir.display(),
            working_copy.display()
        );
        return Ok(());
    }

    let reuse_marker_import_proof = existing_marker
        .as_ref()
        .is_some_and(fast_img_reuses_marker_import_proof_on_resume);
    let saved_dir_timestamps = foundation::save_directory_timestamps(&src_dir)
        .with_context(|| format!("snapshot fast-img directory metadata {}", src_dir.display()))?;

    let mut marker = existing_marker.unwrap_or_else(|| {
        WorkingCopyMarker::new(src_dir.clone(), working_copy.clone(), source_jpegs.len())
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
        retry_resume_stage(
            &marker.stage,
            retry.0 || fast_img_auto_retry_failed_marker(&marker),
        )
    };
    if !resume_local_delivery_for_shortest_path
        && transcode_complete_or_later(&resume_stage)
        && !fast_img_marker_outputs_current(&marker)?
    {
        println!(
            "[RESUME  ] existing marker has missing/drifted JXL output; rebuilding local JXL outputs"
        );
        tracing::warn!(
            target: "fast_img",
            stage = %marker.stage.as_str(),
            working_copy = %working_copy.display(),
            "fast-img marker output proof is not current; downgrading resume stage to output_prepared"
        );
        resume_stage = FastImgStageName::OutputPrepared;
        marker.transcoded_count = 0;
        marker.gate1_checks = Gate1Checks::default();
        marker.gate2_checks = Gate2Checks::default();
        marker.gate3_checks = Gate3Checks::default();
        marker.blake3_log.clear();
        marker.skipped_sources.clear();
    }
    if !resume_local_delivery_for_shortest_path && !retry_failed_sources_from_cleanup {
        marker.src_jpeg_count = source_jpegs.len();
    }
    let refresh_transcode_complete_resume_outputs =
        transcode_complete_or_later(&resume_stage) && !gate1_complete_or_later(&resume_stage);
    marker.stage = if output_prepared_or_later(&resume_stage) {
        resume_stage
    } else {
        FastImgStageName::ScanComplete
    };
    marker.error = None;
    write_marker_atomic(&marker)?;

    if marker.stage == FastImgStageName::ScanComplete {
        let msg = fast_img_delete_notice_message(
            source_jpegs.len(),
            lossy_modern_static_candidates.len(),
            &src_dir,
        );
        println!("{msg}");
        tracing::info!(target: "fast_img", message = %msg, "fast-img delete notice acknowledged automatically");
    }

    if !output_prepared_or_later(&marker.stage) {
        println!("[PREPARE ] JXL output {}", working_copy.display());
        prepare_jxl_output_dir(&working_copy).with_context(|| {
            format!(
                "create fast-img adjacent JXL output directory {}",
                working_copy.display()
            )
        })?;
        marker.stage = FastImgStageName::OutputPrepared;
        write_marker_atomic(&marker)?;
    }

    if !transcode_complete_or_later(&marker.stage) {
        fast_img_run_transcode_phase(
            &mut marker,
            &source_jpegs,
            &current_source_hashes,
            &src_dir,
            &working_copy,
            retry_failed_sources_from_cleanup,
            archive,
            allow_expert_options,
        )?;
    }

    if refresh_transcode_complete_resume_outputs {
        let refreshed = fast_img_refresh_marker_jxl_deliveries(&mut marker, &src_dir)?;
        if refreshed > 0 {
            write_marker_atomic(&marker)?;
        }
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
        &lossy_modern_static_candidates,
    )?;

    Ok(())
}

const fn fast_img_post_gate1_policy(shortest_path: ShortestPathFlag) -> FastImgPostGate1Policy {
    if shortest_path.0 {
        FastImgPostGate1Policy::ShortestPathImportAndVerify
    } else {
        FastImgPostGate1Policy::JxlOnlyDelivery
    }
}

const fn fast_img_auto_retry_failed_stage(stage: &FastImgStageName) -> bool {
    matches!(stage, FastImgStageName::Gate1Failed)
}

fn fast_img_auto_retry_failed_marker(marker: &WorkingCopyMarker) -> bool {
    fast_img_auto_retry_failed_stage(&marker.stage)
        || (marker.stage == FastImgStageName::Gate3Failed
            && fast_img_marker_has_complete_import_proof(marker))
}

fn fast_img_reuses_marker_import_proof_on_resume(marker: &WorkingCopyMarker) -> bool {
    marker.stage == FastImgStageName::Gate3Failed
        && fast_img_marker_has_complete_import_proof(marker)
}

fn fast_img_marker_has_complete_import_proof(marker: &WorkingCopyMarker) -> bool {
    marker.blake3_log.len() == marker.expected_output_count()
        && marker.blake3_log.values().all(|entry| {
            entry
                .library_asset
                .as_ref()
                .is_some_and(|hash| *hash == entry.out)
        })
}

fn fast_img_delete_notice_message(jpeg_count: usize, tier2_count: usize, src_dir: &Path) -> String {
    let tier2_notice = if tier2_count > 0 {
        format!(
            " It will also delete {tier2_count} verified tier-2 lossy modern static source file(s) after Photos import."
        )
    } else {
        String::new()
    };
    format!(
        "[NOTICE  ] fast-img JXL-only delivery for {jpeg_count} JPEGs from {}. \
         This workflow will directly delete original JPEG files after strict verification.{tier2_notice} \
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
) -> PipelineCtx {
    PipelineCtx {
        working_copy: marker.working_copy.clone(),
        src_dir: marker.src_dir.clone(),
        blake3_log: marker.blake3_log.clone(),
        expected_count,
        library_handle,
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

/// Session-scoped size accounting: ONLY files transcoded in THIS run.
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
) -> anyhow::Result<(PathBuf, String)> {
    let naive_out = working_copy.join(rel.with_extension("JXL"));
    let reserved_out = foundation::conversion::reserve_output_path(source, &naive_out);
    let out_rel_key =
        fast_img_output_rel_key(&reserved_out, working_copy, "fast_img_planned_output_rel")?;
    if reserved_out != naive_out {
        println!(
            "[NOTICE  ] {} reserved output {out_rel_key} due to filename collision with an existing reservation or on-disk JXL",
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

fn fast_img_effective_transcode_parallelism(
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

fn fast_img_run_transcode_job_inner(
    job: &FastImgTranscodeJob,
    src_dir: &Path,
    working_copy: &Path,
    child_threads: usize,
    archive: bool,
    allow_expert_options: bool,
) -> anyhow::Result<FastImgTranscodeOutcome> {
    let options = LosslessConvertOptions {
        output_dir: Some(working_copy.to_path_buf()),
        base_dir: Some(src_dir.to_path_buf()),
        flags: LosslessConvertFlags::FORCE
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
    let result = convert_jpeg_to_jxl(&job.source, &options, None)?;
    if result.skipped && result.output_path.is_none() {
        let skip_reason = result.skip_reason.as_deref().unwrap_or("<none>");
        if skip_reason == img::lossless_converter::JPEG_LOSSLESS_TRANSCODE_UNAVAILABLE_SKIP_REASON {
            return Ok(FastImgTranscodeOutcome::Skipped(
                FastImgSkippedSourceProof {
                    rel_key: job.rel_key.clone(),
                    src_hash: calculate_blake3_hash(&job.source)?,
                    reason: result.message,
                },
            ));
        }
        anyhow::bail!(
            "fast-img transcode skipped without JXL output for {}: reason={skip_reason} message={}",
            job.source.display(),
            result.message
        );
    }
    let out_path = result.output_path.as_ref().map(Path::new).ok_or_else(|| {
        anyhow::anyhow!(
            "transcode produced no output path for {}",
            job.source.display()
        )
    })?;

    let orientation_tolerance = orientation_diff_tolerance_for_format(FormatKind::Jxl)
        .ok_or_else(|| anyhow::anyhow!("missing shared orientation tolerance for JXL output"))?;
    match verify_orientation_pixel_diff(
        &job.source,
        out_path,
        FormatKind::Jxl,
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

    let src_hash = calculate_blake3_hash(&job.source)?;
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

fn fast_img_run_transcode_job(
    job: &FastImgTranscodeJob,
    src_dir: &Path,
    working_copy: &Path,
    child_threads: usize,
    archive: bool,
    allow_expert_options: bool,
) -> FastImgJobResult {
    fast_img_run_transcode_job_inner(
        job,
        src_dir,
        working_copy,
        child_threads,
        archive,
        allow_expert_options,
    )
    .map_err(|err| FastImgTranscodeError {
        rel_key: job.rel_key.clone(),
        out_rel_key: job.out_rel_key.clone(),
        src_hash: job.src_hash.clone(),
        reason: err.to_string(),
    })
}

fn fast_img_remove_failed_transcode_output(
    working_copy: &Path,
    err: &FastImgTranscodeError,
) -> anyhow::Result<()> {
    let out_rel = fast_img_checked_rel_path(&err.out_rel_key)?;
    let output = working_copy.join(out_rel);
    match std::fs::remove_file(&output) {
        Ok(()) => {
            tracing::warn!(
                target: "fast_img",
                source_rel = %err.rel_key,
                output = %output.display(),
                "removed failed fast-img output before continuing batch"
            );
            Ok(())
        }
        Err(remove_err) if remove_err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(remove_err) => Err(remove_err).with_context(|| {
            format!(
                "fast-img failed to remove failed output {}; refusing to continue with a stale/corrupt JXL in the delivery directory",
                output.display()
            )
        }),
    }
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

fn fast_img_refresh_marker_jxl_deliveries(
    marker: &mut WorkingCopyMarker,
    src_dir: &Path,
) -> anyhow::Result<usize> {
    let mut refreshed = 0usize;
    for (rel, entry) in &mut marker.blake3_log {
        let source = src_dir.join(fast_img_checked_rel_path(rel)?);
        let out_rel = if let Some(out_rel) = entry.out_rel.as_deref() {
            fast_img_checked_rel_path(out_rel)?
        } else {
            fast_img_checked_rel_path(rel)?.with_extension("JXL")
        };
        let output = marker.working_copy.join(out_rel);
        let refreshed_out_hash = fast_img_refresh_reused_jxl_delivery(&source, &output)?;
        if entry.out != refreshed_out_hash {
            entry.library_asset = None;
        }
        entry.out = refreshed_out_hash;
        refreshed += 1;
    }
    Ok(refreshed)
}

fn fast_img_validate_recorded_source_hashes_current(
    marker: &WorkingCopyMarker,
    current_source_hashes: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
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
                reason: "fast-img transcode left source without disposition record; source retained unmodified"
                    .to_string(),
            },
        );
        reconciled += 1;
        println!("[SKIP    ] {rel_key} retained: transcode disposition was not recorded");
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
) -> anyhow::Result<()> {
    if marker.src_jpeg_count != current_count {
        anyhow::bail!(
            "fast-img source count changed before JXL-only delivery: marker={} current={current_count}",
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
        anyhow::bail!("fast-img JXL-only output hash incomplete for {rel}");
    }
    if !fast_img_marker_outputs_current(marker)? {
        anyhow::bail!("fast-img JXL output proof missing/drifted before delivery");
    }

    Ok(())
}

fn fast_img_validate_cleanup_retry_jxl_only_delivery_exit(
    marker: &WorkingCopyMarker,
    current_count: usize,
    current_source_hashes: &BTreeMap<String, String>,
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
        anyhow::bail!("fast-img cleanup retry JXL output proof missing/drifted before delivery");
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

fn fast_img_delete_verified_source_jpegs(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
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
        println!(
            "[VERIFY  ] final JXL delete proofs pending {} · parallel {} djxl checks",
            existing.len(),
            parallelism
        );
        tracing::info!(
            target: "fast_img_delete",
            pending = existing.len(),
            parallelism,
            "fast-img final JXL delete proof verification start"
        );
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(parallelism)
            .build()
            .map_err(|err| anyhow::anyhow!("fast-img verify thread pool init failed: {err}"))?;
        let results = pool.install(|| {
            existing
                .par_iter()
                .map(|candidate| {
                    let integrity =
                        verify_final_jxl_delivery_integrity(&candidate.source, &candidate.output)
                            .map_err(|err| {
                                anyhow::anyhow!(
                                    "fast-img source delete gate final JXL proof failed for {} -> {}: {err}",
                                    candidate.source.display(),
                                    candidate.output.display()
                                )
                            })?;
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
        fast_img_checked_rel_path(rel)?.with_extension("JXL")
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

fn fast_img_strip_non_jxl_files(working_copy: &Path) -> anyhow::Result<()> {
    let mut pending_dirs = vec![working_copy.to_path_buf()];
    let mut files_to_delete = Vec::new();
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
            let is_jxl = path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jxl"));
            if is_jxl {
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
                "delete non-JXL fast-img working-copy file {}",
                path.display()
            )
        })?;
        tracing::info!(
            target: "fast_img",
            path = %path.display(),
            "deleted non-JXL working-copy file after Gate 1"
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
    records: &[RestoreJpegCommitProof],
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
        content.push_str(&restore_jpeg_hex_encode(&record.source_rel));
        content.push('\t');
        content.push_str(&restore_jpeg_hex_encode(&record.output_rel));
        content.push('\t');
        content.push_str(&record.source_hash);
        content.push('\t');
        content.push_str(&record.output_hash);
        content.push_str("\ttrue\n");
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
    restore_records: &mut Vec<RestoreJpegCommitProof>,
    proof: &RestoreJpegCommitProof,
) -> anyhow::Result<bool> {
    restore_records.push(proof.clone());
    write_restore_jpeg_manifest(output_root, restore_records).with_context(|| {
        format!(
            "restore-jpeg failed to persist deletion manifest before removing {}",
            proof.source.display()
        )
    })?;
    restore_jpeg_delete_verified_source(proof)
}

fn restore_jpeg_remove_temp(temp: &Path, context: &str) -> anyhow::Result<()> {
    foundation::io_utils::safe_remove_file(temp).with_context(|| {
        format!(
            "restore-jpeg failed to remove temp file {} after {context}",
            temp.display()
        )
    })
}

fn restore_jpeg_decode_to_temp(input: &Path, temp_output: &Path) -> anyhow::Result<()> {
    let decode = foundation::DjxlBuilder::new()
        .input(input)
        .output(temp_output)
        .build()
        .output()
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

    let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
        .map_err(|err| anyhow::anyhow!("restore-jpeg temp path failed: {err}"))?;
    let decode = foundation::DjxlBuilder::new()
        .input(input)
        .output(&temp_output)
        .build()
        .output()
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

    let committed = foundation::conversion::commit_temp_to_output_with_metadata(
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
    })?;

    let proof = restore_jpeg_build_current_proof(input, input_root, &output, output_root)
        .with_context(|| {
            format!(
                "restore-jpeg failed to build deletion proof for {} -> {}",
                input.display(),
                output.display()
            )
        })?;

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

fn run_restore_jpeg(
    input: &Path,
    output_dir: Option<&Path>,
    recursive: bool,
    force: bool,
) -> anyhow::Result<()> {
    if let Err(err) = foundation::tools::require(&["djxl", "exiftool"]) {
        log_fatal!(foundation::infra::static_logs::messages::LABEL_TOOLS, &err);
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    let input_root = restore_jpeg_input_root(input)?;
    let output_root = match output_dir {
        Some(path) => path.to_path_buf(),
        None => restore_jpeg_default_output_dir(input)?,
    };
    let files = restore_jpeg_candidate_files(input, recursive)?;
    println!(
        "[SCAN    ] Found {} true JXL files in {}",
        files.len(),
        input_root.display()
    );

    let mut restored = 0usize;
    let mut skipped = 0usize;
    let mut deleted_sources = 0usize;
    let mut deleted_source_dirs = Vec::new();
    let mut restore_records = Vec::new();
    write_restore_jpeg_manifest(&output_root, &restore_records)?;
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
        "[DONE    ] restored {restored} JPEGs to {} ({skipped} existing outputs skipped) source JXLs deleted={deleted_sources}",
        output_root.display()
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
    if marker.src_jpeg_count != current_count {
        anyhow::bail!(
            "fast-img marker source count changed: marker={} current={current_count}",
            marker.src_jpeg_count
        );
    }
    let marker_hashes = fast_img_marker_recorded_source_hashes(marker)?;
    if !marker_hashes.is_empty() {
        let partial_log_allowed = marker.stage == FastImgStageName::Gate1Failed
            || marker.stage == FastImgStageName::OutputPrepared;
        let marker_hashes_match = if partial_log_allowed {
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
            "fast-img marker missing BLAKE3 source log for post-transcode JXL-only output; refusing stale resume"
        );
    }
    Ok(())
}

fn fast_img_retry_marker_source_set_is_stale(
    marker: &WorkingCopyMarker,
    src_dir: &Path,
    current_count: usize,
) -> bool {
    if marker.src_dir != src_dir {
        return true;
    }
    if marker.src_jpeg_count != current_count {
        return true;
    }
    false
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

#[allow(clippy::too_many_arguments)]
fn fast_img_run_transcode_phase(
    marker: &mut WorkingCopyMarker,
    source_jpegs: &[std::path::PathBuf],
    current_source_hashes: &std::collections::BTreeMap<String, String>,
    src_dir: &std::path::Path,
    working_copy: &std::path::Path,
    retry_failed_sources_from_cleanup: bool,
    archive: bool,
    allow_expert_options: bool,
) -> anyhow::Result<()> {
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

        if let Some(entry) = marker.skipped_sources.get(&rel_key) {
            fast_img_emit_explicit_skip(&rel_key, &entry.reason);
            continue;
        }
        if marker.failed_sources.contains_key(&rel_key) && !retry_failed_sources_from_cleanup {
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
            let refreshed_out_hash = fast_img_refresh_reused_jxl_delivery(source, &resume_out)?;
            if let Some(entry) = marker.blake3_log.get_mut(&rel_key) {
                if entry.out != refreshed_out_hash {
                    entry.library_asset = None;
                }
                entry.out = refreshed_out_hash;
                entry.out_rel = Some(resume_out_rel_key.clone());
            }
            println!("[TRANSCODE] reused verified output for {rel_key} -> {resume_out_rel_key}");
            completed_from_resume += 1;
            continue;
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
                "pre-claimed marker output path for stale-proof retranscode"
            );
            let reserved = foundation::conversion::reserve_output_path(source, &recorded_out);
            let out_rel_key = fast_img_output_rel_key(
                &reserved,
                working_copy,
                "fast_img_resume_retranscode_rel",
            )?;
            if reserved != recorded_out {
                tracing::warn!(
                    target: "fast_img",
                    rel = %rel_key,
                    recorded = %recorded_out_rel,
                    actual = %reserved.display(),
                    "stale-proof retranscode: marker out_rel was already taken by another source; using new path"
                );
            }
            (reserved, out_rel_key)
        } else {
            fast_img_planned_output_rel(source, working_copy, rel)?
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
        let (parallel_tasks, child_threads) = fast_img_effective_transcode_parallelism(
            pending,
            thread_config.parallel_tasks,
            thread_config.child_threads,
        );
        println!(
            "[TRANSCODE] pending {pending}/{total} · skipped {completed_from_resume} · parallel {parallel_tasks} × {child_threads} cjxl threads"
        );
        tracing::info!(
            target: "fast_img",
            pending,
            skipped = completed_from_resume,
            total,
            parallel_tasks,
            child_threads,
            "fast-img parallel transcode start"
        );
        let completed = AtomicUsize::new(0);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(parallel_tasks)
            .build()
            .map_err(|err| anyhow::anyhow!("fast-img transcode thread pool init failed: {err}"))?;
        let results = pool.install(|| {
            jobs.par_iter()
                .map(|job| {
                    let result = fast_img_run_transcode_job(
                        job,
                        src_dir,
                        working_copy,
                        child_threads,
                        archive,
                        allow_expert_options,
                    );
                    if result.is_ok() {
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        println!("[TRANSCODE] {done}/{pending} {}", job.source.display());
                    }
                    result
                })
                .collect::<Vec<_>>()
        });

        let mut transcoded = completed_from_resume;
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
                    fast_img_remove_failed_transcode_output(working_copy, &err)?;
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
                    transcoded += 1;
                    marker.transcoded_count = transcoded;
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
                "[SKIP    ] {session_skipped} source JPEG(s) explicitly skipped during transcode"
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
            println!("[FAIL    ] {session_failed} source JPEG(s) failed and were left in place");
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
        marker.transcoded_count = completed_from_resume;
        println!(
            "[TRANSCODE] 0 pending · reused {completed_from_resume}/{total} verified JXL outputs"
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
        println!("[SKIP    ] {reconciled} source JPEG(s) reconciled as explicit skips");
    }
    marker.stage = FastImgStageName::TranscodeComplete;
    write_marker_atomic(marker)?;
    Ok(())
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
    auto_import: AutoImportFlag,
    reuse_marker_import_proof: bool,
    lossy_modern_static_candidates: &[ModernLossyStaticCandidate],
) -> anyhow::Result<()> {
    let reconciled = fast_img_reconcile_unrecorded_source_disposition(
        marker,
        src_dir,
        source_jpegs,
        current_source_hashes,
    )?;
    if reconciled > 0 {
        write_marker_atomic(marker)?;
        println!(
            "[SKIP    ] {reconciled} source JPEG(s) reconciled as explicit skips before delivery"
        );
    }
    let expected_count = fast_img_effective_expected_count(
        marker,
        source_jpegs.len(),
        resume_local_delivery_for_shortest_path,
    );
    let ctx = fast_img_pipeline_ctx(marker, expected_count, None);
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

    if fast_img_post_gate1_policy(shortest_path) == FastImgPostGate1Policy::JxlOnlyDelivery {
        if retry_failed_sources_from_cleanup {
            fast_img_validate_cleanup_retry_jxl_only_delivery_exit(
                marker,
                source_jpegs.len(),
                current_source_hashes,
            )?;
        } else {
            fast_img_validate_jxl_only_delivery_exit(
                marker,
                source_jpegs.len(),
                current_source_hashes,
            )?;
        }
        let (source_deleted, source_already_deleted) =
            fast_img_delete_verified_source_jpegs(marker, src_dir)?;
        let source_dirs_pruned = fast_img_prune_empty_source_dirs(marker, src_dir)?;
        fast_img_strip_non_jxl_files(working_copy)?;
        foundation::restore_delivery_directory_metadata(
            saved_dir_timestamps,
            src_dir,
            working_copy,
        )
        .with_context(|| {
            format!(
                "restore fast-img directory metadata {} -> {} after JXL-only cleanup",
                src_dir.display(),
                working_copy.display()
            )
        })?;
        marker.stage = FastImgStageName::CleanupComplete;
        marker.error = None;
        write_marker_atomic(marker)?;
        println!(
            "[DELIVER ] Gate 1 passed; JXL-only output at {}; source JPEGs deleted={} already_absent={} empty_dirs_pruned={}",
            working_copy.display(),
            source_deleted,
            source_already_deleted,
            source_dirs_pruned
        );
        return Ok(());
    }

    let library_handle = if import_complete_or_later(&marker.stage) {
        let library_handle = if reuse_marker_import_proof
            && let Some(library_handle) = library_handle_from_marker_import_proof(marker)
                .map_err(|err| anyhow::anyhow!("fast-img marker import proof invalid: {err}"))?
        {
            tracing::info!(
                target: "fast_img",
                imported = library_handle.imported_assets.len(),
                "fast-img reusing marker Photos import proof for retry/resume"
            );
            library_handle
        } else {
            import_jxl_outputs_with_library_verifier(marker).map_err(|err| {
                anyhow::anyhow!(
                    "fast-img shortest-path resume requires fresh Photos/iCloud verification: {err}"
                )
            })?
        };
        apply_library_assets_to_marker(marker, &library_handle)
            .map_err(|err| anyhow::anyhow!("fast-img marker/library verifier mismatch: {err}"))?;
        write_marker_atomic(marker)?;
        library_handle
    } else {
        if confirm_import_required(&marker.stage, auto_import.0) {
            let confirmed = prompt_user_confirm(&format!(
                "Gate 1 passed. Import {expected_count} JXLs to Photos? [y/N] "
            ))?;
            if !confirmed {
                marker.stage = FastImgStageName::Aborted;
                marker.error = Some("ConfirmImport declined by user".to_string());
                write_marker_atomic(marker)?;
                anyhow::bail!(
                    "fast-img aborted at ConfirmImport; wc preserved at {}",
                    working_copy.display()
                );
            }
        }
        let library_handle = import_jxl_outputs_with_library_verifier(marker).map_err(|err| {
            anyhow::anyhow!("fast-img shortest-path import verifier failed: {err}")
        })?;
        apply_library_assets_to_marker(marker, &library_handle)
            .map_err(|err| anyhow::anyhow!("fast-img marker/library verifier mismatch: {err}"))?;
        if !lossy_modern_static_candidates.is_empty() {
            println!(
                "[TIER 2  ] importing {} lossy modern static source(s) to Photos",
                lossy_modern_static_candidates.len()
            );
            let tier2_handle =
                import_modern_lossy_static_tier(src_dir, lossy_modern_static_candidates).map_err(
                    |err| {
                        anyhow::anyhow!("fast-img tier-2 modern lossy static import failed: {err}")
                    },
                )?;
            apply_tier2_library_assets_to_marker(marker, &tier2_handle).map_err(|err| {
                anyhow::anyhow!("fast-img tier-2 marker import proof failed: {err}")
            })?;
            tracing::info!(
                target: "fast_img",
                imported = tier2_handle.imported_assets.len(),
                "fast-img tier-2 Photos import completed"
            );
            println!(
                "[TIER 2  ] imported {} lossy modern static asset(s) to Photos",
                tier2_handle.imported_assets.len()
            );
        }
        marker.stage = FastImgStageName::ImportComplete;
        marker.error = None;
        write_marker_atomic(marker)?;
        library_handle
    };

    if import_complete_or_later(&marker.stage)
        && !lossy_modern_static_candidates.is_empty()
        && marker.tier2_imported_assets.is_empty()
    {
        println!(
            "[TIER 2  ] importing {} lossy modern static source(s) to Photos (resume/backfill)",
            lossy_modern_static_candidates.len()
        );
        let tier2_handle = import_modern_lossy_static_tier(src_dir, lossy_modern_static_candidates)
            .map_err(|err| {
                anyhow::anyhow!(
                    "fast-img tier-2 modern lossy static import failed on resume: {err}"
                )
            })?;
        apply_tier2_library_assets_to_marker(marker, &tier2_handle).map_err(|err| {
            anyhow::anyhow!("fast-img tier-2 marker import proof failed on resume: {err}")
        })?;
        write_marker_atomic(marker)?;
        println!(
            "[TIER 2  ] imported {} lossy modern static asset(s) to Photos (resume/backfill)",
            tier2_handle.imported_assets.len()
        );
    }

    let tier2_library_handle = library_handle_from_marker_tier2_proof(marker);

    if !gate2_complete_or_later(&marker.stage) {
        print_photos_verifier_proof_summary(&library_handle, expected_count);
        println!("[GATE 2  ] verifying Photos import");
        let gate2 = Gate2Import.run(&fast_img_pipeline_ctx(
            marker,
            expected_count,
            Some(library_handle.clone()),
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
        fast_img_delete_verified_source_jpegs(marker, src_dir)?;
    let source_dirs_pruned = fast_img_prune_empty_source_dirs(marker, src_dir)?;
    let (tier2_deleted, tier2_already_deleted) = if let Some(tier2_library_handle) =
        tier2_library_handle.as_ref()
    {
        if !tier2_library_handle.imported_assets.is_empty() {
            println!(
                "[DELETE  ] removing {} verified tier-2 lossy modern static source(s)",
                tier2_library_handle.imported_assets.len()
            );
        }
        delete_verified_modern_lossy_static_sources(src_dir, tier2_library_handle, true).map_err(
            |err| anyhow::anyhow!("fast-img tier-2 source delete failed after Gate 3: {err}"),
        )?
    } else {
        (0, 0)
    };
    let tier2_dirs_pruned = if let Some(tier2_library_handle) = tier2_library_handle.as_ref() {
        prune_empty_source_dirs_for_tier2_assets(src_dir, &tier2_library_handle.imported_assets)
            .map_err(|err| {
                anyhow::anyhow!("fast-img tier-2 empty source dir prune failed: {err}")
            })?
    } else {
        0
    };
    fast_img_strip_non_jxl_files(working_copy)?;
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
        tier2_deleted,
        tier2_already_deleted,
        empty_dirs_pruned = source_dirs_pruned,
        tier2_empty_dirs_pruned = tier2_dirs_pruned,
        src_dir = %src_dir.display(),
        "fast-img deleted verified source files after Gate 3"
    );

    marker.stage = FastImgStageName::CleanupComplete;
    marker.error = None;
    write_marker_atomic(marker)?;
    if tier2_deleted + tier2_already_deleted > 0 {
        println!(
            "[DONE    ] {} JXL files · {} source JPEGs deleted · {} tier-2 modern static deleted · {} empty source dirs pruned · JXL-only output at {} · gates: ①②③ all ✅",
            ctx.expected_count,
            source_deleted,
            tier2_deleted,
            source_dirs_pruned + tier2_dirs_pruned,
            working_copy.display()
        );
    } else {
        println!(
            "[DONE    ] {} files · {} source JPEGs deleted · {} empty source dirs pruned · JXL-only output at {} · gates: ①②③ all ✅",
            ctx.expected_count,
            source_deleted,
            source_dirs_pruned,
            working_copy.display()
        );
    }
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

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn build_auto_convert_config(
    output_dir: Option<PathBuf>,
    base_dir: Option<PathBuf>,
    force: bool,
    should_delete: bool,
    preserve_timestamps: bool,
    preserve: bool,
    compress: bool,
    apple_compat: bool,
    in_place: bool,
    explore: bool,
    match_quality: bool,
    ultimate: bool,
    archive: bool,
    allow_size_tolerance: bool,
    allow_expert_options: bool,
    verbose: bool,
    cache: Option<Arc<AnalysisCache>>,
    static_delivery: foundation::delivery_codec_strategy::ImgStaticDelivery,
) -> AutoConvertConfig {
    AutoConvertConfig {
        output_dir,
        base_dir,
        flags: {
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
                | if archive {
                    ConfigFlags::ARCHIVE_MODE
                } else {
                    ConfigFlags::empty()
                }
                | if allow_size_tolerance {
                    ConfigFlags::ALLOW_SIZE_TOLERANCE
                } else {
                    ConfigFlags::empty()
                }
                | if allow_expert_options {
                    ConfigFlags::ALLOW_EXPERT_OPTIONS
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
        cache,
        static_delivery,
    }
}

#[cfg(test)]
mod fast_img_hardening_tests {
    use super::{
        AutoImportFlag, Cli, Commands, DeleteSourceFlag, DryRunFlag,
        FastImgCleanupCompleteSourceState, FastImgInputPlan, FastImgPostGate1Policy,
        FastImgRunOptions, FastImgTranscodeError, RecursiveFlag, RetryFlag, ShortestPathFlag,
        command_requires_database, fast_img_auto_retry_failed_marker,
        fast_img_auto_retry_failed_stage, fast_img_cleanup_complete_has_shortest_path_proof,
        fast_img_cleanup_complete_should_resume_shortest_path_import,
        fast_img_cleanup_complete_source_state, fast_img_delete_notice_message,
        fast_img_delete_verified_source_jpegs_with, fast_img_effective_expected_count,
        fast_img_effective_transcode_parallelism, fast_img_effective_verify_parallelism,
        fast_img_marker_entry_output_path, fast_img_marker_outputs_current, fast_img_pipeline_ctx,
        fast_img_planned_output_rel, fast_img_post_gate1_policy, fast_img_prune_empty_source_dirs,
        fast_img_reconcile_unrecorded_source_disposition, fast_img_refresh_marker_jxl_deliveries,
        fast_img_refresh_reused_jxl_delivery, fast_img_remove_failed_transcode_output,
        fast_img_retry_marker_source_set_is_stale, fast_img_run_transcode_phase,
        fast_img_skip_hashes_match, fast_img_source_hash_set, fast_img_strip_non_jxl_files,
        fast_img_validate_cleanup_retry_jxl_only_delivery_exit,
        fast_img_validate_jxl_only_delivery_exit, restore_jpeg_build_current_proof_with_decoder,
        restore_jpeg_candidate_files, restore_jpeg_delete_verified_source,
        restore_jpeg_output_path_for, restore_jpeg_prune_empty_source_dirs, run_fast_img,
        validate_cleanup_complete_marker, validate_fast_img_marker_source_state,
    };
    use anyhow::Context;
    use clap::Parser;
    use foundation::fast_img::{
        FastImgLibraryAssetProbe, IntegrityResult, apply_library_assets_to_marker, is_true_jpeg,
        library_handle_from_probes,
    };
    use foundation::pipeline::verification::{
        Blake3Entry, FastImgStageName, Gate2Import, Gate3Deep, PipelineCtx, SkippedSourceEntry,
        VerificationGate, WorkingCopyMarker,
    };
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static FAST_IMG_TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnvGuard {
        key: &'static str,
        old_value: Option<std::ffi::OsString>,
    }

    struct TestEnvPolicyGuard {
        _lock: MutexGuard<'static, ()>,
        _guards: Vec<TestEnvGuard>,
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
            _lock: lock,
            _guards: guards,
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
            _lock: lock,
            _guards: guards,
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
        ])?;

        let Commands::RestoreJpeg {
            input,
            output,
            recursive,
            force,
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
        Ok(())
    }

    #[test]
    fn restore_jpeg_command_does_not_require_database_preflight() {
        let command = Commands::RestoreJpeg {
            input: std::path::PathBuf::from("/photos/Album_optimized"),
            output: Some(std::path::PathBuf::from("/photos/Album_restored_jpeg")),
            recursive: true,
            force: false,
        };

        assert!(!command_requires_database(&command));
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
        marker.transcoded_count = 1;
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
            archive: false,
            allow_expert_options: false,
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

        fast_img_remove_failed_transcode_output(&wc, &err)?;

        assert!(!out.exists());
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

        fast_img_validate_cleanup_retry_jxl_only_delivery_exit(&marker, 1, &current_hashes)?;

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
        marker.transcoded_count = 1;
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
            archive: false,
            allow_expert_options: false,
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
        let old_hash = foundation::common_utils::calculate_blake3_hash(&out)?;
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 1);
        marker.stage = FastImgStageName::TranscodeComplete;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: foundation::common_utils::calculate_blake3_hash(&src)?,
                out: old_hash.clone(),
                library_asset: Some(old_hash.clone()),
            },
        );

        let refreshed = fast_img_refresh_marker_jxl_deliveries(&mut marker, &src_root)?;
        let entry = marker
            .blake3_log
            .get("a.jpg")
            .ok_or_else(|| anyhow::anyhow!("missing refreshed marker entry"))?;

        assert_eq!(refreshed, 1);
        assert_ne!(entry.out, old_hash);
        assert_eq!(entry.library_asset, None);
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

        fast_img_run_transcode_phase(
            &mut marker,
            std::slice::from_ref(&src),
            &current_source_hashes,
            &src_root,
            &wc,
            false,
            false,
            false,
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
    fn stale_proof_retranscode_keeps_marker_out_rel_path() -> anyhow::Result<()> {
        if !foundation::ExiftoolBuilder::check_available()
            || !foundation::common_utils::is_command_available(foundation::constants::TOOL_CJXL)
        {
            return Ok(());
        }

        let root = TempDir::new()?;
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

        // Compute the *current* source hash (needed for the transcode phase scan)
        let current_source_hashes =
            fast_img_source_hash_set(&src_root, std::slice::from_ref(&src))?;

        // Build a marker with a STALE source hash so hashes won't match,
        // forcing existing_output_current = false → re-transcode branch
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

        // Run the transcode phase — it will detect stale proof, re-transcode,
        // and must write back to a.JXL (not a (1).JXL)
        fast_img_run_transcode_phase(
            &mut marker,
            std::slice::from_ref(&src),
            &current_source_hashes,
            &src_root,
            &wc,
            false,
            false,
            false,
        )?;

        let entry = marker
            .blake3_log
            .get("a.jpg")
            .context("missing marker entry after retranscode")?;

        assert_eq!(
            entry.out_rel.as_deref(),
            Some("a.JXL"),
            "stale-proof retranscode must keep the marker's recorded out_rel"
        );
        assert!(
            !wc.join("a (1).JXL").exists(),
            "stale-proof retranscode must not produce a spurious collision path"
        );
        Ok(())
    }

    #[test]
    fn fast_img_transcode_options_force_overwrite_stale_outputs() -> anyhow::Result<()> {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
        )?;
        let options_pos = source
            .find("let options = LosslessConvertOptions")
            .ok_or_else(|| anyhow::anyhow!("fast-img transcode options must exist"))?;
        let options_block = &source[options_pos..];
        let force_pos = options_block
            .find("LosslessConvertFlags::FORCE")
            .ok_or_else(|| {
                anyhow::anyhow!("fast-img transcode must force overwrite stale JXL outputs")
            })?;
        let require_output_delivery_pos = options_block
            .find("LosslessConvertFlags::REQUIRE_OUTPUT_DELIVERY")
            .ok_or_else(|| anyhow::anyhow!("fast-img transcode must require output delivery"))?;

        assert!(
            force_pos < require_output_delivery_pos,
            "fast-img queued transcodes must overwrite stale/corrupt JXL siblings before delivery checks"
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
    fn retry_marker_stale_source_count_is_discarded_for_fresh_run() -> anyhow::Result<()> {
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
        let mut marker = WorkingCopyMarker::new(src_root.clone(), wc, 2);
        marker.stage = FastImgStageName::Gate1Failed;
        marker.transcoded_count = 1;
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

        run_fast_img(FastImgRunOptions {
            input: &src_root,
            output_dir: None,
            delete_source: DeleteSourceFlag(false),
            dry_run: DryRunFlag(true),
            recursive: RecursiveFlag(true),
            auto_import: AutoImportFlag(false),
            shortest_path: ShortestPathFlag(false),
            retry: RetryFlag(true),
            archive: false,
            allow_expert_options: false,
        })?;

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
        assert!(fast_img_retry_marker_source_set_is_stale(
            &marker, &src_root, 1
        ));
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

        assert!(!fast_img_retry_marker_source_set_is_stale(
            &marker, &src_root, 1
        ));
        let Err(err) =
            validate_fast_img_marker_source_state(&marker, &src_root, 1, &current_hashes)
        else {
            anyhow::bail!("retry marker unexpectedly accepted same-count source drift");
        };
        assert!(err.to_string().contains("source hash set changed"));
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
    fn output_prepared_empty_log_accepts_pre_transcode_resume() -> anyhow::Result<()> {
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
                Ok(IntegrityResult::FinalJxlDelivery {
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
    fn transcode_complete_marker_without_log_rejects_resume() -> anyhow::Result<()> {
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
            anyhow::bail!("transcode-complete marker unexpectedly accepted missing hash log");
        };

        assert!(err.to_string().contains("missing BLAKE3 source log"));
        Ok(())
    }

    #[test]
    fn default_policy_is_jxl_only_delivery() {
        assert_eq!(
            fast_img_post_gate1_policy(ShortestPathFlag(false)),
            FastImgPostGate1Policy::JxlOnlyDelivery
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
    fn only_gate1_failed_is_auto_retried_without_explicit_retry() {
        let mut marker = WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("wc"), 1);
        marker.stage = FastImgStageName::Gate1Failed;
        assert!(fast_img_auto_retry_failed_stage(
            &FastImgStageName::Gate1Failed
        ));
        assert!(fast_img_auto_retry_failed_marker(&marker));
        assert!(!fast_img_auto_retry_failed_stage(
            &FastImgStageName::Gate2Failed
        ));
        assert!(!fast_img_auto_retry_failed_stage(
            &FastImgStageName::Gate3Failed
        ));
    }

    #[test]
    fn gate3_failed_auto_retry_requires_complete_import_proof() -> anyhow::Result<()> {
        let mut marker = WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("wc"), 1);
        marker.stage = FastImgStageName::Gate3Failed;
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: "out".to_string(),
                library_asset: None,
            },
        );
        assert!(!fast_img_auto_retry_failed_marker(&marker));

        marker
            .blake3_log
            .get_mut("a.jpg")
            .ok_or_else(|| anyhow::anyhow!("missing test marker entry"))?
            .library_asset = Some("out".to_string());
        assert!(fast_img_auto_retry_failed_marker(&marker));

        marker
            .blake3_log
            .get_mut("a.jpg")
            .ok_or_else(|| anyhow::anyhow!("missing test marker entry"))?
            .library_asset = Some("drift".to_string());
        assert!(!fast_img_auto_retry_failed_marker(&marker));
        Ok(())
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
            allow_expert_options: false,
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
    fn run_command_accepts_archive_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["img", "run", "/photos", "--archive"])?;

        let Commands::Run { archive, .. } = parsed.command else {
            anyhow::bail!("expected run command");
        };
        assert!(archive);
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
        let message = fast_img_delete_notice_message(3, 0, std::path::Path::new("/photos"));

        assert!(message.contains("directly delete original JPEG files"));
        assert!(message.contains("Back up"));
        assert!(!message.contains("[y/N]"));
    }

    #[test]
    fn delete_notice_mentions_tier2_sources_when_present() {
        let message = fast_img_delete_notice_message(2, 4, std::path::Path::new("/photos"));

        assert!(message.contains("tier-2 lossy modern static"));
        assert!(message.contains("4"));
    }

    #[test]
    fn fastmode_parallelism_caps_to_pending_jobs_and_keeps_child_threads() {
        assert_eq!(fast_img_effective_transcode_parallelism(3, 8, 2), (3, 2));
        assert_eq!(fast_img_effective_transcode_parallelism(10, 4, 0), (4, 1));
        assert_eq!(fast_img_effective_transcode_parallelism(0, 4, 2), (1, 2));
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

        fast_img_validate_jxl_only_delivery_exit(&marker, 1, &current_hashes)?;
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
                reason: "lossless JPEG transcode failed after strict cascade".to_string(),
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
                reason: "lossless JPEG transcode failed after strict cascade".to_string(),
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
                reason: "lossless JPEG transcode failed after strict cascade".to_string(),
            },
        );

        fast_img_validate_jxl_only_delivery_exit(&marker, 2, &current_hashes)?;
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
                reason: "lossless JPEG transcode failed after strict cascade".to_string(),
            },
        );

        let (deleted, already_deleted) =
            fast_img_delete_verified_source_jpegs_with(&marker, &src_root, |_source, _output| {
                Ok(IntegrityResult::FinalJxlDelivery {
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
                Ok(IntegrityResult::FinalJxlDelivery {
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
                Ok(IntegrityResult::FinalJxlDelivery {
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
                Ok(IntegrityResult::FinalJxlDelivery {
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
            fast_img_planned_output_rel(&source_jpeg, &wc, Path::new("photo.jpeg"))?;
        let (_, collision_jxl_rel) =
            fast_img_planned_output_rel(&input_jpg_file, &wc, Path::new("photo.jpg"))?;

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

        fast_img_validate_jxl_only_delivery_exit(&marker, 2, &current_hashes)?;
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

        let err = match fast_img_validate_jxl_only_delivery_exit(&marker, 1, &current_hashes) {
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
    fn wc_contains_only_jxl_after_gate1() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let wc = root.path().join("Photos_");
        std::fs::create_dir_all(&wc)?;
        let jpeg = wc.join("a.jpg");
        let jxl = wc.join("a.JXL");
        write_jpeg(&jpeg, b"source")?;
        std::fs::write(&jxl, b"jxl")?;

        fast_img_strip_non_jxl_files(&wc)?;

        assert!(!jpeg.exists());
        assert!(jxl.exists());
        let remaining = std::fs::read_dir(&wc)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(remaining, vec![std::ffi::OsString::from("a.JXL")]);
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
        std::fs::write(&out, b"jxl-bytes")?;
        std::fs::write(&library_asset, b"jxl-bytes")?;

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
        std::fs::write(wc.join("a.JXL"), b"jxl-bytes")?;
        std::fs::write(&library_asset, b"jxl-bytes")?;
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
        std::fs::write(wc.join("a.JXL"), b"jxl-bytes")?;
        std::fs::write(&library_asset, b"jxl-bytes")?;
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
        std::fs::write(&out, b"jxl-bytes")?;
        std::fs::write(&library_asset, b"jxl-bytes")?;
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

        let ctx = fast_img_pipeline_ctx(&marker, marker.src_jpeg_count, Some(handle));

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
        std::fs::write(wc.join("a.JXL"), b"jxl-bytes")?;
        std::fs::write(&library_asset, b"jxl-bytes")?;
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
}
