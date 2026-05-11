#![allow(
    clippy::multiple_crate_versions,
    reason = "Legitimate deviation from standard linting rules justified by specific project architecture."
)]
#![allow(
    unexpected_cfgs,
    reason = "macos_ui is an optional feature that may not be defined in all builds"
)]

use clap::{Parser, Subcommand};
use shared_utils::log_detail;
use std::path::PathBuf;

use shared_utils::analysis_cache::AnalysisCache;
use shared_utils::conversion_types::SelectedCodec;
use vid::{
    ConfigFlags, ConversionConfig, VidQualityError, auto_convert_with_cache, detect_video,
    determine_strategy_with_apple_compat,
};

#[derive(Parser)]
#[command(name = "vid")]
#[command(version, about = "High-performance video and animated media converter", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(name = "run")]
    Run {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
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
        apple_compat: bool,
        #[arg(long)]
        no_apple_compat: bool,
        #[arg(long, default_value_t = true)]
        compress: bool,
        #[arg(long, default_value_t = false)]
        force_ms_ssim_long: bool,
        #[arg(long, default_value_t = false)]
        ultimate: bool,
        #[arg(long)]
        base_dir: Option<PathBuf>,
        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,
        #[arg(long)]
        no_allow_size_tolerance: bool,
        #[arg(short, long)]
        verbose: bool,
        #[arg(long, default_value_t = false)]
        resume: bool,
        #[arg(long)]
        no_resume: bool,
        #[arg(long, value_parser = ["hevc", "av1"], default_value = "hevc")]
        codec: String,
    },

    Strategy {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(long, value_parser = ["hevc", "av1"], default_value = "hevc")]
        codec: String,
    },

    #[command(
        name = "ingest-samples",
        about = "Batch ingest unannotated GIF samples into SQLite database for Active Learning"
    )]
    IngestSamples {
        #[arg(value_name = "INPUT_DIR")]
        input: PathBuf,
        #[arg(short, long)]
        label: Option<String>,
    },

    #[command(
        name = "db-health",
        about = "Perform deep diagnostic scan of the database infrastructure and data integrity"
    )]
    DbHealth,
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn main() -> anyhow::Result<()> {
    if let Err(e) = shared_utils::init_ghost_mode() {
        shared_utils::log_anomaly!(
            shared_utils::static_logs::messages::LABEL_GHOST_MODE,
            &e.to_string()
        );
    }

    if let Err(e) = shared_utils::logging::init("vid", &shared_utils::logging::LogConfig::default())
    {
        shared_utils::log_anomaly!(
            shared_utils::static_logs::messages::LABEL_LOGGING,
            &e.to_string()
        );
    }

    shared_utils::ctrlc_guard::init();

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
        _ => None,
    };

    let _lock_guard = input_to_lock.and_then(|input| {
        let input_abs = std::fs::canonicalize(input).unwrap_or_else(|_| input.clone());
        if input_abs.is_dir() {
            match shared_utils::acquire_dir_lock(&input_abs) {
                Ok(guard) => Some(guard),
                Err(e) => {
                    shared_utils::log_fatal!(
                        shared_utils::static_logs::messages::LABEL_LOCK,
                        &e.to_string()
                    );
                    std::process::exit(shared_utils::constants::EXIT_CODE_LOCK_FAILURE);
                }
            }
        } else {
            None
        }
    });
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
            apple_compat,
            no_apple_compat,
            compress,
            force_ms_ssim_long,
            ultimate,
            base_dir,
            allow_size_tolerance,
            no_allow_size_tolerance,
            verbose,
            resume,
            no_resume,
            codec,
        } => {
            // Fail-fast if critical sub-tools are missing
            if let Err(e) = shared_utils::tools::require(&["ffmpeg", "ffprobe", "exiftool"]) {
                shared_utils::log_fatal!(shared_utils::static_logs::messages::LABEL_TOOLS, &e);
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }

            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let resume = resume && !no_resume;
            let selected_codec = if codec.to_lowercase() == "av1" {
                SelectedCodec::Av1
            } else {
                SelectedCodec::Hevc
            };

            if selected_codec == SelectedCodec::Av1 && apple_compat {
                shared_utils::log_fatal!(
                    shared_utils::static_logs::messages::LABEL_CONFIG,
                    shared_utils::static_logs::messages::APPLE_COMPAT_HEVC,
                );
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }

            if let Err(e) =
                shared_utils::validate_flags_result_with_ultimate(shared_utils::FlagRequest {
                    base: shared_utils::FlagBase {
                        explore,
                        match_quality,
                        compress,
                    },
                    tier: shared_utils::FlagTier { ultimate },
                })
            {
                shared_utils::log_fatal!(shared_utils::static_logs::messages::LABEL_CONFIG, &e);
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }

            let base_dir =
                shared_utils::cli_runner::resolve_video_run_base_dir(&input, recursive, base_dir);

            let config = ConversionConfig {
                output_dir: output.clone(),
                base_dir: base_dir.clone(),
                flags: ConfigFlags::empty()
                    | if force {
                        ConfigFlags::FORCE
                    } else {
                        ConfigFlags::empty()
                    }
                    | if delete_original {
                        ConfigFlags::DELETE_ORIGINAL
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
                    | if in_place {
                        ConfigFlags::IN_PLACE
                    } else {
                        ConfigFlags::empty()
                    }
                    | if apple_compat {
                        ConfigFlags::APPLE_COMPAT
                    } else {
                        ConfigFlags::empty()
                    }
                    | if compress {
                        ConfigFlags::REQUIRE_COMPRESSION
                    } else {
                        ConfigFlags::empty()
                    }
                    | ConfigFlags::USE_GPU
                    | if force_ms_ssim_long {
                        ConfigFlags::FORCE_MS_SSIM_LONG
                    } else {
                        ConfigFlags::empty()
                    }
                    | if ultimate {
                        ConfigFlags::ULTIMATE_MODE
                    } else {
                        ConfigFlags::empty()
                    }
                    | if allow_size_tolerance {
                        ConfigFlags::ALLOW_SIZE_TOLERANCE
                    } else {
                        ConfigFlags::empty()
                    },
                min_ssim: shared_utils::constants::MIN_SSIM_DEFAULT,
                child_threads: shared_utils::thread_manager::get_balanced_thread_config(
                    shared_utils::thread_manager::WorkloadType::Video,
                )
                .child_threads,
                codec: selected_codec,
            };

            shared_utils::progress_mode::set_verbose_mode(verbose);
            // Automatically created and written to ./logs/vid_run_<timestamp>.log during run, no flags needed.
            if let Err(e) = shared_utils::progress_mode::set_default_run_log_file("vid") {
                shared_utils::log_anomaly!(
                    shared_utils::static_logs::messages::LABEL_RUN_LOG,
                    shared_utils::static_logs::messages::RUN_LOG_OPEN_FAIL
                );
                shared_utils::log_info!(
                    shared_utils::static_logs::messages::LABEL_RUN_LOG,
                    &format!("Detailed run log failure: {e}")
                );
            }
            log_detail!(&format!(
                "🎬 Run Mode Conversion ({})",
                selected_codec.as_str().to_uppercase()
            ));
            if selected_codec == SelectedCodec::Hevc {
                log_detail!("Lossless sources → HEVC Lossless MKV");
                if match_quality {
                    log_detail!("Lossy sources → HEVC MP4 (CRF auto-matched to input quality)");
                } else {
                    log_detail!("Lossy sources → HEVC MP4 (CRF 18-20)");
                }
            } else {
                log_detail!("Lossless sources → AV1 Lossless MKV");
                if match_quality {
                    log_detail!("Lossy sources → AV1 MP4 (CRF auto-matched to input quality)");
                } else {
                    log_detail!("Lossy sources → AV1 MP4 (CRF 30-32)");
                }
            }
            if explore {
                log_detail!("📊 Size exploration: ENABLED");
            }
            if match_quality {
                log_detail!("🎯 Match Quality: ENABLED");
            }
            if apple_compat {
                log_detail!("🍎 Apple Compatibility: ENABLED (AV1/VP9 → HEVC)");
                unsafe { std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1") };
            }
            if recursive {
                log_detail!("📂 Recursive: ENABLED");
            }
            if ultimate {
                log_detail!("🔍 Ultimate Explore: ENABLED (search until SSIM saturates)");
            }
            if force_ms_ssim_long {
                log_detail!("⚠️  Force MS-SSIM for long videos: ENABLED");
            }
            let cache = match AnalysisCache::default_local() {
                Ok(cache) => Some(cache),
                Err(e) => {
                    shared_utils::log_anomaly!(
                        shared_utils::static_logs::messages::LABEL_CACHE,
                        &format!("Failed to initialize persistent cache: {e}")
                    );
                    None
                }
            };
            if cache.is_some() {
                log_detail!("💽 Persistent Cache: ENABLED");
            }
            shared_utils::database::report_db_status();

            log_detail!("");

            shared_utils::cli_runner::run_auto_command(
                &shared_utils::cli_runner::Config {
                    input: input.clone(),
                    output: output.clone(),
                    recursive,
                    label: "HEVC Video".to_string(),
                    base_dir: base_dir.or_else(|| {
                        if output.is_some() {
                            Some(input.clone())
                        } else {
                            None
                        }
                    }),
                    resume,
                    protect_destructive_dirs: delete_original || in_place,
                },
                |file| {
                    auto_convert_with_cache(file, &config, cache.as_ref())
                        .map_err(|e: VidQualityError| anyhow::anyhow!(e))
                },
            )?;
            shared_utils::progress_mode::xmp_merge_finalize();
            shared_utils::progress_mode::flush_log_file();
        }

        Commands::Strategy { input, codec } => {
            let detection = detect_video(&input)?;
            let selected_codec = if codec.to_lowercase() == "av1" {
                SelectedCodec::Av1
            } else {
                SelectedCodec::Hevc
            };
            let strategy = determine_strategy_with_apple_compat(
                &detection,
                &input,
                false,
                false,
                selected_codec,
            );

            log_detail!("\n🎯 Recommended Strategy (Auto Mode)");
            log_detail!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            log_detail!("📁 File: {}", input.display());
            log_detail!(
                "🎬 Codec: {} ({})",
                detection.codec.as_str(),
                detection.compression.as_str(),
            );
            log_detail!();
            log_detail!("💡 Target: {}", strategy.target.as_str());
            log_detail!("📝 Reason: {}", strategy.reason);
            log_detail!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        }

        Commands::IngestSamples { input, label } => {
            if !input.is_dir() {
                shared_utils::log_anomaly!(
                    shared_utils::static_logs::messages::LABEL_ANOMALY,
                    "Input path must be a directory"
                );
                std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
            }
            if let Some(lbl) = &label {
                log_detail!(
                    "📥 Ingesting GIF samples with label '{}' from: {}",
                    lbl,
                    input.display(),
                );
            } else {
                log_detail!("📥 Ingesting GIF samples from: {}", input.display());
            }
            match shared_utils::database::batch_ingest_samples(&input, label.as_deref()) {
                Ok(count) => {
                    log_detail!(
                        "✅ Successfully ingested {count} samples into PostgreSQL database"
                    );
                }
                Err(e) => {
                    shared_utils::log_anomaly!(
                        shared_utils::static_logs::messages::LABEL_ANOMALY,
                        &format!("Failed to ingest samples: {e}")
                    );
                    std::process::exit(shared_utils::constants::EXIT_CODE_ERROR);
                }
            }
        }
        Commands::DbHealth => {
            shared_utils::log_info!(
                shared_utils::static_logs::messages::LABEL_REPORT,
                "Starting Deep Database Health Scan..."
            );
            match shared_utils::database::check_database_health() {
                Ok(report) => {
                    shared_utils::log_info!(
                        shared_utils::static_logs::messages::LABEL_REPORT,
                        "[DATABASE HEALTH REPORT]"
                    );
                    shared_utils::log_info!(
                        shared_utils::static_logs::messages::LABEL_REPORT,
                        &format!(
                            "   - Connection: {}",
                            if report.connected {
                                "✅ Connected"
                            } else {
                                "❌ Failed"
                            }
                        )
                    );
                    shared_utils::log_info!(
                        shared_utils::static_logs::messages::LABEL_REPORT,
                        &format!("   - PG Version: {}", report.pg_version)
                    );
                    shared_utils::log_info!(
                        shared_utils::static_logs::messages::LABEL_REPORT,
                        &format!(
                            "   - pgvector Status: {}",
                            if report.has_vector_extension {
                                format!(
                                    "✅ Installed ({})",
                                    report.vector_extension_version.unwrap_or_default()
                                )
                            } else {
                                "❌ Missing".to_string()
                            }
                        )
                    );
                    shared_utils::log_info!(
                        shared_utils::static_logs::messages::LABEL_REPORT,
                        &format!("   - Maturity: {}", report.maturity_status)
                    );

                    shared_utils::log_info!(
                        shared_utils::static_logs::messages::LABEL_REPORT,
                        "[Table Statistics]"
                    );
                    let mut tables: Vec<_> = report.table_counts.iter().collect();
                    tables.sort_by_key(|(name, _)| *name);
                    for (name, count) in tables {
                        shared_utils::log_info!(
                            shared_utils::static_logs::messages::LABEL_REPORT,
                            &format!("   - {name:<20}: {count:>8} records")
                        );
                    }

                    if report.corruption_found {
                        shared_utils::log_anomaly!(
                            shared_utils::static_logs::messages::LABEL_REPORT,
                            "[INTEGRITY WARNINGS]"
                        );
                        for detail in report.corruption_details {
                            shared_utils::log_anomaly!(
                                shared_utils::static_logs::messages::LABEL_REPORT,
                                &format!("   {detail}")
                            );
                        }
                    } else {
                        shared_utils::log_info!(
                            shared_utils::static_logs::messages::LABEL_REPORT,
                            "[Integrity]: No NaN/Inf corruption found in feature vectors."
                        );
                    }
                }
                Err(e) => {
                    shared_utils::log_anomaly!(
                        shared_utils::static_logs::messages::LABEL_REPORT,
                        &format!("Health Check Failed: {e}")
                    );
                }
            }
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
