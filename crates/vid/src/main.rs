#![allow(clippy::too_many_lines)]

use anyhow::Context;
use clap::{Parser, Subcommand};
use foundation::log_detail;
use std::path::PathBuf;

use foundation::analysis_cache::AnalysisCache;
use foundation::conversion_types::SelectedCodec;
use foundation::delivery_codec_strategy::DeliveryProduct;
use vid::{
    ConfigFlags, ConversionConfig, VidQualityError, auto_convert_with_cache, detect_video,
    determine_strategy_with_apple_compat,
};

#[derive(Parser)]
#[command(name = "vid")]
#[command(version, about = "High-performance video and animated media converter", long_about = None)]
struct Cli {
    #[command(flatten)]
    planned_source: foundation::batch::planned_source::PlannedSourceArgs,
    /// Overlay an explicit JSON configuration after user and project preferences.
    #[arg(long, global = true, conflicts_with = "no_config")]
    config: Option<PathBuf>,
    /// Ignore configuration files; explicit flags and legacy environment still apply.
    #[arg(long, global = true)]
    no_config: bool,
    /// Output codec for video runs.
    #[arg(long, global = true, value_enum)]
    codec: Option<foundation::runtime_config::VidCodec>,
    /// Standard VID only: include fusion quality checks for long videos; omission inherits configuration.
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    force_ms_ssim_long: Option<bool>,
    #[command(flatten)]
    performance: foundation::runtime_config::PerformanceArgs,
    #[command(flatten)]
    execution: foundation::runtime_config::ExecutionArgs,
    #[command(flatten)]
    cache: foundation::runtime_config::cache_args::CacheArgs,
    #[command(flatten)]
    tools: foundation::runtime_config::ToolsArgs,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Inspect persistent runtime preferences without processing media or accessing databases.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
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
        #[arg(long, default_value_t = false)]
        apple_compat: bool,
        #[arg(long)]
        no_apple_compat: bool,
        #[arg(long, default_value_t = true)]
        compress: bool,
        #[arg(long, default_value_t = false)]
        ultimate: bool,
        #[arg(long, default_value_t = false)]
        archive: bool,
        #[arg(long)]
        base_dir: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        allow_size_tolerance: bool,
        #[arg(long)]
        no_allow_size_tolerance: bool,
        /// Explicitly allow HDR10+ encodes to continue as static HDR10 if dynamic metadata extraction fails.
        #[arg(long, default_value_t = false)]
        allow_hdr10plus_static_fallback: bool,
        #[arg(short, long, default_value_t = true)]
        verbose: bool,
        /// ASCII symbols, no decorative ANSI (also respects `NO_COLOR` / `MODERN_FORMAT_PLAIN_UI=1`).
        #[arg(long, default_value_t = false)]
        plain: bool,
        #[arg(long, default_value_t = false)]
        resume: bool,
        #[arg(long)]
        no_resume: bool,
        #[arg(long, value_parser = ["default", "avif"], default_value = "default")]
        strategy: String,
    },

    Strategy {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
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

    /// Import original GIF files into Apple Photos and verify delivery before cleanup.
    #[command(name = "fast-vid")]
    FastVid(FastVidArgs),
}

#[derive(clap::Args)]
struct FastVidArgs {
    /// GIF file or directory to scan recursively; identity is checked from content.
    #[arg(value_name = "INPUT")]
    input: PathBuf,
    /// Exact private working copy for this task.
    #[arg(short, long, value_name = "WORKING_COPY")]
    output: Option<PathBuf>,
    /// Preview the admission without importing, deleting, or writing a checkpoint.
    #[arg(long)]
    dry_run: bool,
    #[command(flatten)]
    resume: FastVidResumeArgs,
    /// Exact parent checkpoint for a failed-only retry.
    #[arg(long, value_name = "WORKING_COPY", requires = "retry_failed")]
    retry_failed_parent: Option<PathBuf>,
    #[command(flatten)]
    photos: foundation::runtime_config::photos_args::PhotosArgs,
}

#[derive(clap::Args)]
#[group(multiple = false)]
struct FastVidResumeArgs {
    /// Continue a matching task from its verified checkpoint.
    #[arg(long)]
    retry: bool,
    /// Start a fresh task with its own working copy.
    #[arg(long)]
    no_resume: bool,
    /// Retry retained failures from a completed task.
    #[arg(long)]
    retry_failed: bool,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show resolved preferences and the source of every value without processing media.
    Show {
        /// Accepted for clarity; show always prints the effective configuration.
        #[arg(long)]
        effective: bool,
    },
    /// Validate the effective configuration, without processing or accessing databases.
    Validate,
    /// Inspect one resolved tool using bounded version commands, without processing media.
    Tool { name: String },
}

fn resolve_runtime(cli: &Cli) -> anyhow::Result<foundation::runtime_config::LoadedConfig> {
    anyhow::ensure!(
        cli.force_ms_ssim_long.is_none()
            || matches!(cli.command, Commands::Run { .. } | Commands::Config { .. }),
        "--force-ms-ssim-long is available only for standard video runs and configuration inspection"
    );
    let mut loaded = foundation::runtime_config::load(cli.config.as_deref(), cli.no_config)?;
    cli.performance.apply_to(&mut loaded);
    cli.execution.apply_to(&mut loaded);
    cli.cache.apply_to(&mut loaded);
    if let Some(codec) = cli.codec {
        loaded.config.vid.codec = codec;
        foundation::runtime_config::record_source(
            &mut loaded.sources,
            &mut loaded.source_chain,
            "vid.codec",
            "CLI",
        );
    }
    if let Some(force) = cli.force_ms_ssim_long {
        loaded.config.vid.force_ms_ssim_long = force;
        foundation::runtime_config::record_source(
            &mut loaded.sources,
            &mut loaded.source_chain,
            "vid.force_ms_ssim_long",
            "CLI",
        );
    }
    cli.tools.apply_to(&mut loaded)?;
    Ok(loaded)
}

fn resolve_run_apple_compat(
    requested: bool,
    disabled: bool,
    codec: foundation::runtime_config::VidCodec,
) -> bool {
    (requested || codec == foundation::runtime_config::VidCodec::Hevc) && !disabled
}

fn inspect_config(cli: &Cli, command: &ConfigCommand) -> anyhow::Result<()> {
    let loaded = resolve_runtime(cli)?;
    match command {
        ConfigCommand::Show { .. } => println!("{}", loaded.to_json(true)?),
        ConfigCommand::Tool { name } => {
            let source = loaded.sources.get(&format!("tools.paths.{name}")).cloned();
            foundation::runtime_config::install(loaded.config)?;
            let mut inspection = foundation::tools::inspect_tool(name)?;
            inspection.configuration_source = source;
            println!("{}", serde_json::to_string(&inspection)?);
        }
        ConfigCommand::Validate => println!(
            "{}",
            serde_json::json!({"valid": true, "sources": loaded.sources})
        ),
    }
    Ok(())
}

const fn command_requires_database(command: &Commands) -> bool {
    !matches!(command, Commands::Config { .. } | Commands::FastVid(_))
}

fn validate_command_strategy(command: &Commands) -> anyhow::Result<()> {
    if matches!(
        command,
        Commands::Run {
            strategy,
            ..
        } if strategy == "avif"
    ) {
        anyhow::bail!("manual AVIF selection is unavailable in vid run");
    }
    Ok(())
}

// Fast VID shares the source admission, Photos custody and cleanup worker with Fast IMG.
fn fast_vid_worker(cli: &Cli, args: &FastVidArgs) -> anyhow::Result<std::process::Command> {
    anyhow::ensure!(
        cli.codec.is_none() && cli.force_ms_ssim_long.is_none(),
        "Video encoding options do not apply to original GIF import"
    );
    for (name, _) in &cli.tools.tools {
        anyhow::ensure!(
            name == "osxphotos",
            "Original GIF import supports only the osxphotos tool override (received {name})"
        );
    }
    let worker =
        std::env::current_exe()?.with_file_name(format!("img{}", std::env::consts::EXE_SUFFIX));
    let mut command = std::process::Command::new(worker);
    command.args(["fast-img", "--strategy", "gif", "--shortest-path"]);
    if let Some(config) = &cli.config {
        command.arg("--config").arg(config);
    }
    if cli.no_config {
        command.arg("--no-config");
    }
    command.args(cli.performance.cli_arguments());
    command.args(cli.execution.cli_arguments());
    command.args(cli.cache.cli_arguments());
    command.args(cli.tools.cli_arguments()?);
    command.args(args.photos.cli_arguments());
    if let Some(output) = &args.output {
        command.arg("--output").arg(output);
    }
    if args.dry_run {
        command.arg("--dry-run");
    }
    if args.resume.retry {
        command.arg("--retry");
    }
    if args.resume.no_resume {
        command.arg("--no-resume");
    }
    if args.resume.retry_failed {
        command.arg("--retry-failed");
    }
    if let Some(parent) = &args.retry_failed_parent {
        command.arg("--retry-failed-parent").arg(parent);
    }
    command.arg("--").arg(&args.input);
    Ok(command)
}

fn run_fast_vid(cli: &Cli, args: &FastVidArgs) -> anyhow::Result<()> {
    let mut worker = fast_vid_worker(cli, args)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(worker.exec()).context("could not start the co-installed img worker for GIF import")
    }
    #[cfg(not(unix))]
    {
        let status = worker
            .status()
            .context("could not start the co-installed img worker for GIF import")?;
        std::process::exit(
            status
                .code()
                .context("GIF import worker exited without a status code")?,
        )
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
fn main() -> anyhow::Result<()> {
    foundation::entry_guard::assert_product_cli_entry("vid").context("vid entry guard")?;
    let cli = Cli::parse();
    cli.planned_source.verify_for_input(match &cli.command {
        Commands::Run { input, .. } => Some(input.as_path()),
        _ => None,
    })?;
    if let Commands::Config { command } = &cli.command {
        return inspect_config(&cli, command);
    }
    if let Commands::FastVid(args) = &cli.command {
        return run_fast_vid(&cli, args);
    }
    let _production_lease = matches!(
        &cli.command,
        Commands::Run { .. } | Commands::IngestSamples { .. }
    )
    .then(foundation::production_lock::acquire_production_lease)
    .transpose()?;
    validate_command_strategy(&cli.command)?;
    let runtime = resolve_runtime(&cli)?;
    let configured_codec = runtime.config.vid.codec;
    let force_ms_ssim_long = runtime.config.vid.force_ms_ssim_long;
    if matches!(&cli.command, Commands::Run { .. }) {
        runtime.emit_history_marker_if_session("vid")?;
    }
    foundation::runtime_config::install(runtime.config)?;
    foundation::init_ghost_mode().context("Failed to initialize ghost mode")?;

    foundation::logging::init("vid", &foundation::logging::LogConfig::default())
        .map_err(|e| e.context("Failed to initialize vid logging"))?;

    foundation::ctrlc_guard::init();

    if command_requires_database(&cli.command) {
        // Enforce PostgreSQL dependency as mandatory for the DB-backed video toolchain.
        if let Err(e) = foundation::database::open_pg_client() {
            foundation::log_fatal!(
                "Infrastructure",
                &format!(
                    "PostgreSQL database is mandatory for full feature availability. Connection failed: {e}"
                )
            );
            std::process::exit(foundation::constants::EXIT_CODE_ERROR);
        }
    }

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
        let input_abs = foundation::media_conversion_gate::canonicalize_for_tool_input(input);
        if input_abs.is_dir() {
            match foundation::acquire_dir_lock(&input_abs) {
                Ok(guard) => Some(guard),
                Err(e) => {
                    foundation::log_fatal!(
                        foundation::infra::static_logs::messages::LABEL_LOCK,
                        &e.to_string()
                    );
                    std::process::exit(foundation::constants::EXIT_CODE_LOCK_FAILURE);
                }
            }
        } else {
            None
        }
    });
    // ------------------------------------------------------

    match cli.command {
        Commands::Config { .. } => unreachable!("configuration inspection returned before startup"),
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
            ultimate,
            archive,
            base_dir,
            allow_size_tolerance,
            no_allow_size_tolerance,
            allow_hdr10plus_static_fallback,
            verbose,
            plain,
            resume,
            no_resume,
            strategy,
        } => {
            // Fail-fast if critical sub-tools are missing
            if let Err(e) = foundation::tools::require(&["ffmpeg", "ffprobe", "exiftool"]) {
                foundation::log_fatal!(foundation::infra::static_logs::messages::LABEL_TOOLS, &e);
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }

            let apple_compat =
                resolve_run_apple_compat(apple_compat, no_apple_compat, configured_codec);
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let resume = foundation::checkpoint::resolve_resume_choice(
                &input,
                output.as_deref(),
                resume,
                no_resume,
            )?;
            let selected_codec = match SelectedCodec::resolve_cli_delivery_codec(
                DeliveryProduct::Vid,
                configured_codec.as_str(),
                apple_compat,
            ) {
                Ok(c) => c,
                Err(e) => {
                    foundation::log_fatal!(
                        foundation::infra::static_logs::messages::LABEL_CONFIG,
                        &e.to_string(),
                    );
                    std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                }
            };
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_STRATEGY,
                foundation::delivery_codec_strategy::vid_run_routing_summary(selected_codec)
            );
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_STRATEGY,
                selected_codec.delivery_policy_summary()
            );

            if let Err(e) =
                foundation::validate_flags_result_with_ultimate(foundation::FlagRequest {
                    base: foundation::FlagBase {
                        explore,
                        match_quality,
                        compress,
                    },
                    tier: foundation::FlagTier { ultimate },
                })
            {
                foundation::log_fatal!(foundation::infra::static_logs::messages::LABEL_CONFIG, &e);
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }

            let base_dir =
                foundation::cli_runner::resolve_video_run_base_dir(&input, recursive, base_dir);

            let config = build_conversion_config(
                output.clone(),
                base_dir.clone(),
                force,
                delete_original,
                explore,
                match_quality,
                in_place,
                apple_compat,
                compress,
                force_ms_ssim_long,
                ultimate,
                archive,
                allow_size_tolerance,
                allow_hdr10plus_static_fallback,
                plain,
                &strategy,
                selected_codec,
            );

            foundation::progress_mode::configure_terminal_ux(plain);
            foundation::progress_mode::set_verbose_mode(verbose);
            foundation::progress_mode::maybe_log_inference_analytics_hint(verbose);
            // Automatically created under LogConfig::unified_log_dir() as vid_run_<timestamp>.log.
            if let Err(e) = foundation::progress_mode::set_default_run_log_file("vid") {
                foundation::log_fatal!(
                    foundation::infra::static_logs::messages::LABEL_RUN_LOG,
                    &format!(
                        "{}: {}",
                        foundation::infra::static_logs::messages::RUN_LOG_OPEN_FAIL,
                        e
                    )
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
            let strategy_msg = if explore && match_quality {
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_MAPPING_LOSSY_MATCH
            } else if explore {
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_MAPPING_LOSSY_BASE
            } else {
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_MAPPING_LOSSLESS
            };

            let mut config_parts = Vec::new();
            config_parts.push(if selected_codec == SelectedCodec::Hevc {
                format!(
                    "codec=HEVC quality={}",
                    if match_quality { "match" } else { "18-20" }
                )
            } else {
                format!(
                    "codec=AV1 quality={}",
                    if match_quality { "match" } else { "30-32" }
                )
            });

            if explore {
                config_parts.push("explore=ON".to_string());
            }
            if apple_compat {
                config_parts.push("apple_compat=ON".to_string());
                unsafe { std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1") };
            }
            if recursive {
                config_parts.push("recursive=ON".to_string());
            }
            if ultimate {
                config_parts.push("ultimate=ON".to_string());
            }
            if archive {
                config_parts.push("archive=ON".to_string());
            }
            if force_ms_ssim_long {
                config_parts.push("ms_ssim=ON".to_string());
            }
            if allow_hdr10plus_static_fallback {
                config_parts.push("hdr10plus_static_fallback=ON".to_string());
            }

            let cache = match AnalysisCache::default_local() {
                Ok(cache) => {
                    config_parts.push("cache=ON".to_string());
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
                    Some(cache)
                }
                Err(e) => {
                    foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                        "analysis_cache_unavailable",
                        format!("failed to initialize persistent cache: {e}"),
                    );
                    None
                }
            };

            foundation::log_info!(
                foundation::infra::static_logs::messages::LABEL_CONFIG,
                &format!(
                    "Session Audit: {} | {}",
                    strategy_msg.replace("{}", &selected_codec.as_str().to_uppercase()),
                    config_parts.join(" | ")
                )
            );

            foundation::database::report_db_status();

            log_detail!("");

            foundation::cli_runner::run_auto_command(
                &foundation::cli_runner::Config {
                    input: input.clone(),
                    output: output.clone(),
                    recursive,
                    label: "HEVC Video".to_string(),
                    base_dir: foundation::media_conversion_gate::delivery_cli_base_dir_or_input_when_output(
                        base_dir,
                        output.as_deref(),
                        &input,
                    ),
                    resume,
                    protect_destructive_dirs: delete_original || in_place,
                    prune_empty_source_dirs: delete_original,
                    error_mode: foundation::BatchErrorMode::current(),
                },
                |file| {
                    auto_convert_with_cache(file, &config, cache.as_ref())
                        .map_err(|e: VidQualityError| anyhow::anyhow!(e))
                },
            )?;
            foundation::progress_mode::xmp_merge_finalize();
            foundation::progress_mode::flush_log_file();
        }

        Commands::FastVid(_) => unreachable!("GIF import handed off before video initialization"),

        Commands::Strategy { input } => {
            let detection = detect_video(&input)?;
            let selected_codec = SelectedCodec::resolve_cli_delivery_codec(
                DeliveryProduct::Vid,
                configured_codec.as_str(),
                false,
            )
            .map_err(|e| {
                foundation::log_fatal!(
                    foundation::infra::static_logs::messages::LABEL_CONFIG,
                    &e.to_string(),
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            })?;
            let strategy = determine_strategy_with_apple_compat(
                &detection,
                &input,
                false,
                false,
                selected_codec,
            );

            foundation::log_summary_header!(
                foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_AUDIT
            );
            foundation::log_detail!(
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_ASSET
                    .replace("{}", &input.display().to_string())
            );
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_DETECTION,
                foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_DETECTION
                    .replacen("{}", detection.codec.as_str(), 1)
                    .replacen("{}", detection.compression.as_str(), 1)
            );
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_STRATEGY,
                foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_TARGET
                    .replace("{}", strategy.target.as_str())
            );
            foundation::log_detail!(
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_BASIS
                    .replace("{}", &strategy.reason)
            );
        }

        Commands::IngestSamples { input, label } => {
            if !input.is_dir() {
                foundation::media_conversion_gate::probe_layer_batch_audit(
                    "ingest_path_not_directory",
                    format!(
                        "ingest-samples requires a directory; got {}",
                        input.display()
                    ),
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
            if let Some(lbl) = &label {
                log_detail!(format!(
                    "{save} Active Learning Audit: Ingesting labeled video samples [{lbl}] from {path}",
                    save = foundation::modern_ui::symbols::SAVE,
                    path = input.display(),
                ));
            } else {
                log_detail!(format!(
                    "{save} Active Learning Audit: Discovering video samples in {path}",
                    save = foundation::modern_ui::symbols::SAVE,
                    path = input.display(),
                ));
            }
            let conn_str = foundation::database::get_pg_conn_str();
            if conn_str.trim().is_empty() {
                anyhow::bail!("MFB_PG_CONNSTR must be set before ingest-samples");
            }
            match foundation::database::batch_ingest_loop_intent_samples(
                &input,
                label.as_deref(),
                &conn_str,
            ) {
                Ok(count) => {
                    crate::log_detail!(format!(
                        "{check} Active Learning Audit: Successfully ingested {count} video feature vectors",
                        check = foundation::modern_ui::symbols::CHECK,
                    ));
                }
                Err(e) => {
                    foundation::media_conversion_gate::probe_layer_audit(
                        "ingest_batch_failed",
                        &input,
                        format!("batch ingestion failed: {e}"),
                    );
                    std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                }
            }
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
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_ENGINE
                            .replace("{}", &report.pg_version)
                    );
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        format!(
                            "Infrastructure Audit: Required vector extensions: {}",
                            if report.has_vector_extension {
                                format!(
                                    "Infrastructure Audit: pgvector v{} discovered and verified",
                                    foundation::media_conversion_gate::infra_version_label_or_audit(
                                        "pgvector_version",
                                        report.vector_extension_version.as_deref(),
                                        "unknown",
                                    )
                                )
                            } else {
                                "Infrastructure Audit: Missing critical pgvector extension"
                                    .to_string()
                            }
                        )
                    );
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_MATURITY
                            .replace("{}", &report.maturity_status)
                    );

                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_INVENTORY_AUDIT,
                        foundation::infra::static_logs::messages::DB_INVENTORY_CROSS_REF
                    );
                    let mut tables: Vec<_> = report.table_counts.iter().collect();
                    tables.sort_by_key(|(name, _)| *name);
                    for (name, count) in tables {
                        foundation::log_info!(
                            foundation::infra::static_logs::messages::LABEL_REPORT,
                            &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_TABLE
                                .replace("{name:<20}", &format!("{name:<20}"))
                                .replace("{count:>8}", &format!("{count:>8}"))
                        );
                    }

                    if report.corruption_found {
                        foundation::media_conversion_gate::probe_layer_batch_audit(
                            "db_health_corruption",
                            foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CORRUPTION,
                        );
                        for detail in report.corruption_details {
                            foundation::media_conversion_gate::probe_layer_batch_audit(
                                "db_health_corruption_detail",
                                foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CORRUPTION_ALERT
                                    .replace("{}", &detail),
                            );
                        }
                    } else {
                        foundation::log_info!(
                            foundation::infra::static_logs::messages::LABEL_REPORT,
                            foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_INTEGRITY_OK
                        );
                    }
                }
                Err(e) => {
                    foundation::media_conversion_gate::probe_layer_batch_audit(
                        "db_health_check_failed",
                        foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_ABORT
                            .replace("{}", &e.to_string()),
                    );
                    return Err(e).context("db-health failed");
                }
            }
        }
    }

    {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            // Historically waited for macOS UI confirmation via foundation::macos_ui.
            // The foundation crate no longer exposes that module; keep this as a no-op.
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn build_conversion_config(
    output_dir: Option<PathBuf>,
    base_dir: Option<PathBuf>,
    force: bool,
    delete_original: bool,
    explore: bool,
    match_quality: bool,
    in_place: bool,
    apple_compat: bool,
    compress: bool,
    force_ms_ssim_long: bool,
    ultimate: bool,
    archive: bool,
    allow_size_tolerance: bool,
    allow_hdr10plus_static_fallback: bool,
    _plain: bool,
    _strategy: &str,
    codec: SelectedCodec,
) -> ConversionConfig {
    ConversionConfig {
        output_dir,
        base_dir,
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
            | if allow_hdr10plus_static_fallback {
                ConfigFlags::ALLOW_HDR10PLUS_STATIC_FALLBACK
            } else {
                ConfigFlags::empty()
            },
        min_ssim: foundation::constants::MIN_SSIM_DEFAULT,
        child_threads: foundation::thread_manager::get_balanced_thread_config(
            foundation::thread_manager::WorkloadType::Video,
        )
        .child_threads,
        codec,
    }
}

#[cfg(test)]
mod cli_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::expect_fun_call,
        clippy::panic
    )]
    use super::{
        Cli, Commands, command_requires_database, fast_vid_worker, resolve_run_apple_compat,
        resolve_runtime, validate_command_strategy,
    };
    use clap::Parser;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn fast_vid_forwards_original_gif_custody_and_preview_to_its_sibling() -> anyhow::Result<()> {
        let cli = Cli::try_parse_from([
            "vid",
            "--no-config",
            "fast-vid",
            "--dry-run",
            "--retry",
            "--output",
            "checkpoint",
            "--photos-import-root",
            "GIF Originals",
            "--",
            "--strategy",
        ])?;
        let Commands::FastVid(args) = &cli.command else {
            anyhow::bail!("expected Fast VID")
        };
        assert!(!command_requires_database(&cli.command));
        let command = fast_vid_worker(&cli, args)?;
        assert_eq!(
            std::path::Path::new(command.get_program()).parent(),
            std::env::current_exe()?.parent()
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "fast-img",
                "--strategy",
                "gif",
                "--shortest-path",
                "--no-config",
                "--photos-import-root",
                "GIF Originals",
                "--output",
                "checkpoint",
                "--dry-run",
                "--retry",
                "--",
                "--strategy",
            ]
        );
        Ok(())
    }

    #[test]
    fn retired_fast_vid_routes_and_conflicting_retry_flags_are_rejected() -> anyhow::Result<()> {
        assert!(Cli::try_parse_from(["vid", "fast-gif", "."]).is_err());
        for option in [
            "--strategy",
            "--shortest-path",
            "--originals",
            "--force",
            "--apple-compat",
            "--recursive",
            "--external-tool-snapshot",
            "--external-tool-snapshot=false",
        ] {
            assert!(
                Cli::try_parse_from(["vid", "fast-vid", ".", option]).is_err(),
                "{option}"
            );
        }
        assert!(Cli::try_parse_from(["vid", "fast-vid", ".", "--retry", "--no-resume"]).is_err());
        assert!(
            Cli::try_parse_from(["vid", "fast-vid", ".", "--retry-failed-parent", "old"]).is_err()
        );
        for options in [["--codec", "av1"], ["--tool", "cjxl=/unused/cjxl"]] {
            let cli = Cli::try_parse_from(["vid", "fast-vid", "."].into_iter().chain(options))?;
            let Commands::FastVid(args) = &cli.command else {
                anyhow::bail!("expected Fast VID")
            };
            assert!(fast_vid_worker(&cli, args).is_err(), "{options:?}");
        }
        Ok(())
    }

    #[test]
    fn config_inspection_uses_same_codec_resolution_as_processing() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let path = root.path().join("vid.json");
        std::fs::write(&path, br#"{"config_version":1,"vid":{"codec":"av1"}}"#)?;
        let path = path.to_string_lossy().to_string();
        let shown =
            Cli::try_parse_from(["vid", "config", "show", "--effective", "--config", &path])?;
        assert!(matches!(&shown.command, Commands::Config { .. }));
        assert!(!command_requires_database(&shown.command));
        let loaded = resolve_runtime(&shown)?;
        assert_eq!(loaded.config.vid.codec.as_str(), "av1");
        assert_eq!(loaded.sources["vid.codec"], path);
        assert_eq!(loaded.source_chain["vid.codec"], ["default", path.as_str()]);

        let overridden = Cli::try_parse_from([
            "vid", "config", "validate", "--config", &path, "--codec", "hevc",
        ])?;
        let loaded = resolve_runtime(&overridden)?;
        assert_eq!(loaded.config.vid.codec.as_str(), "hevc");
        assert_eq!(loaded.sources["vid.codec"], "CLI");
        assert_eq!(
            loaded.source_chain["vid.codec"],
            ["default", path.as_str(), "CLI"]
        );
        let run = Cli::try_parse_from(["vid", "run", "/unused", "--no-config", "--codec", "av1"])?;
        assert_eq!(resolve_runtime(&run)?.config.vid.codec.as_str(), "av1");
        assert!(
            Cli::try_parse_from(["vid", "config", "show", "--config", &path, "--no-config",])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn shared_tool_paths_reach_video_inspection_run_and_fast_vid_without_execution()
    -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let file = root.path().join("tools.json");
        std::fs::write(&file, br#"{"config_version":1,"tools":{"paths":{"ffmpeg":"/saved/ffmpeg","ffprobe":"/saved/ffprobe"}}}"#)?;
        let file = file
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("synthetic config path is not UTF-8"))?;
        let override_value = "ffmpeg=/synthetic/\u{5de5}\u{5177} nightly=1/ffmpeg";
        for (command, tail) in [
            ("config", "show"),
            ("run", "/unused"),
            ("fast-vid", "/unused"),
        ] {
            let inherited = Cli::try_parse_from(["vid", command, tail, "--config", file])?;
            assert_eq!(
                resolve_runtime(&inherited)?.config.tools.paths["ffmpeg"],
                PathBuf::from("/saved/ffmpeg")
            );
            let cli = Cli::try_parse_from([
                "vid",
                command,
                tail,
                "--config",
                file,
                "--tool",
                "ffmpeg=/first/ffmpeg",
                "--tool",
                override_value,
            ])?;
            let loaded = resolve_runtime(&cli)?;
            assert_eq!(
                loaded.config.tools.paths["ffmpeg"],
                PathBuf::from("/synthetic/\u{5de5}\u{5177} nightly=1/ffmpeg")
            );
            assert_eq!(
                loaded.config.tools.paths["ffprobe"],
                PathBuf::from("/saved/ffprobe")
            );
            assert_eq!(loaded.sources["tools.paths.ffmpeg"], "CLI");
            assert_eq!(
                loaded.source_chain["tools.paths.ffmpeg"],
                [file, "CLI", "CLI"]
            );
            assert_eq!(loaded.sources["tools.paths.ffprobe"], file);
        }
        for value in ["FFMPEG=/synthetic/ffmpeg", "ffmpeg=relative", "ffmpeg="] {
            assert!(Cli::try_parse_from(["vid", "config", "show", "--tool", value]).is_err());
        }
        Ok(())
    }

    #[test]
    fn long_video_quality_policy_inherits_and_accepts_explicit_false() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let path = root.path().join("quality.json");
        std::fs::write(
            &path,
            br#"{"config_version":1,"vid":{"force_ms_ssim_long":true}}"#,
        )?;
        let path = path.to_str().unwrap();
        let inherited = Cli::try_parse_from(["vid", "config", "show", "--config", path])?;
        let loaded = resolve_runtime(&inherited)?;
        assert!(loaded.config.vid.force_ms_ssim_long);
        assert_eq!(loaded.sources["vid.force_ms_ssim_long"], path);
        let explicit = Cli::try_parse_from([
            "vid",
            "run",
            "/synthetic",
            "--config",
            path,
            "--force-ms-ssim-long=false",
        ])?;
        let loaded = resolve_runtime(&explicit)?;
        assert!(!loaded.config.vid.force_ms_ssim_long);
        assert_eq!(
            loaded.source_chain["vid.force_ms_ssim_long"],
            ["default", path, "CLI"]
        );
        let enabled = Cli::try_parse_from([
            "vid",
            "run",
            "/synthetic",
            "--no-config",
            "--force-ms-ssim-long",
        ])?;
        assert!(resolve_runtime(&enabled)?.config.vid.force_ms_ssim_long);
        let default = Cli::try_parse_from(["vid", "config", "show", "--no-config"])?;
        assert!(!resolve_runtime(&default)?.config.vid.force_ms_ssim_long);
        let fast = Cli::try_parse_from([
            "vid",
            "fast-vid",
            "/synthetic",
            "--force-ms-ssim-long=false",
        ])?;
        assert!(resolve_runtime(&fast).is_err());
        for enabled in [true, false] {
            let config = super::build_conversion_config(
                None,
                None,
                false,
                false,
                true,
                true,
                false,
                true,
                true,
                enabled,
                false,
                false,
                false,
                false,
                false,
                "",
                foundation::conversion_types::SelectedCodec::Hevc,
            );
            assert_eq!(
                config
                    .flags
                    .contains(super::ConfigFlags::FORCE_MS_SSIM_LONG),
                enabled
            );
        }
        Ok(())
    }

    #[test]
    fn execution_flags_reach_video_inspection_and_processing() -> anyhow::Result<()> {
        for (command, tail) in [
            ("config", "show"),
            ("run", "/unused"),
            ("fast-vid", "/unused"),
        ] {
            for mode in [
                foundation::BatchErrorMode::LogAndContinue,
                foundation::BatchErrorMode::FailFast,
            ] {
                let cli = Cli::try_parse_from([
                    "vid",
                    command,
                    tail,
                    "--no-config",
                    "--error-mode",
                    mode.as_str(),
                ])?;
                let loaded = resolve_runtime(&cli)?;
                assert_eq!(loaded.config.execution.error_mode, mode);
                assert_eq!(loaded.sources["execution.error_mode"], "CLI");
                assert_eq!(
                    loaded.source_chain["execution.error_mode"]
                        .last()
                        .map(String::as_str),
                    Some("CLI")
                );
            }
        }
        assert!(Cli::try_parse_from(["vid", "config", "show", "--error-mode", "debug"]).is_err());
        Ok(())
    }

    #[test]
    fn cache_flags_reach_video_inspection_and_processing() -> anyhow::Result<()> {
        for (command, tail) in [("config", "show"), ("run", "/unused")] {
            let cli = Cli::try_parse_from([
                "vid",
                command,
                tail,
                "--no-config",
                "--cache-max-bytes",
                "4096",
                "--cache-ttl-seconds",
                "60",
            ])?;
            let loaded = resolve_runtime(&cli)?;
            assert_eq!(loaded.config.cache.path_tree_max_bytes, 4096);
            assert_eq!(loaded.config.cache.path_tree_ttl_seconds, 60);
            assert_eq!(
                loaded.source_chain["cache.path_tree_max_bytes"],
                ["default", "CLI"]
            );
        }
        Ok(())
    }

    #[test]
    fn video_run_apple_compat_default_follows_resolved_codec() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let path = root.path().join("vid.json");
        std::fs::write(&path, br#"{"config_version":1,"vid":{"codec":"av1"}}"#)?;
        let path = path.to_string_lossy().to_string();
        let av1 = Cli::try_parse_from(["vid", "run", "/unused", "--config", &path])?;
        let codec = resolve_runtime(&av1)?.config.vid.codec;
        let Commands::Run {
            apple_compat,
            no_apple_compat,
            ..
        } = av1.command
        else {
            anyhow::bail!("expected video run");
        };
        assert_eq!(codec.as_str(), "av1");
        assert!(!resolve_run_apple_compat(
            apple_compat,
            no_apple_compat,
            codec
        ));

        let explicit =
            Cli::try_parse_from(["vid", "run", "/unused", "--config", &path, "--apple-compat"])?;
        let Commands::Run {
            apple_compat,
            no_apple_compat,
            ..
        } = explicit.command
        else {
            anyhow::bail!("expected video run");
        };
        let apple_compat = resolve_run_apple_compat(apple_compat, no_apple_compat, codec);
        assert!(apple_compat);
        assert!(
            foundation::conversion_types::SelectedCodec::resolve_cli_delivery_codec(
                foundation::delivery_codec_strategy::DeliveryProduct::Vid,
                codec.as_str(),
                apple_compat,
            )
            .is_err()
        );

        let hevc = Cli::try_parse_from(["vid", "run", "/unused", "--no-config"])?;
        let codec = resolve_runtime(&hevc)?.config.vid.codec;
        assert_eq!(codec.as_str(), "hevc");
        assert!(resolve_run_apple_compat(false, false, codec));
        assert!(!resolve_run_apple_compat(false, true, codec));
        Ok(())
    }

    #[test]
    fn normal_vid_run_rejects_manual_avif_strategy() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["vid", "run", "/media/in", "--strategy", "avif"])?;

        let Err(error) = validate_command_strategy(&parsed.command) else {
            anyhow::bail!("normal vid run unexpectedly accepted --strategy avif");
        };
        assert!(
            error
                .to_string()
                .contains("manual AVIF selection is unavailable in vid run")
        );
        Ok(())
    }

    #[test]
    fn run_command_accepts_archive_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["vid", "run", "/media/in", "--archive"])?;

        let Commands::Run { archive, .. } = parsed.command else {
            anyhow::bail!("expected run command");
        };
        assert!(archive);
        Ok(())
    }
}
