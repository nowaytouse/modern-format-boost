use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;

use vid_av1::{auto_convert_with_cache, detect_video_with_cache, determine_strategy, ConversionConfig, VidQualityError};
use shared_utils::analysis_cache::AnalysisCache;

#[derive(Parser)]
#[command(name = "vid-av1")]
#[command(version, about = "Video quality analyzer and format converter - AV1 compression", long_about = None)]
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
        compress: bool,

        #[arg(long, default_value_t = true)]
        apple_compat: bool,

        #[arg(long)]
        no_apple_compat: bool,

        #[arg(long, default_value_t = false)]
        ultimate: bool,

        #[arg(long, default_value_t = false)]
        force_ms_ssim_long: bool,

        #[arg(long)]
        base_dir: Option<PathBuf>,

        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,

        #[arg(long)]
        no_allow_size_tolerance: bool,

        #[arg(short, long)]
        verbose: bool,

        #[arg(long, default_value_t = true)]
        resume: bool,

        #[arg(long)]
        no_resume: bool,
    },

    Strategy {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let _ =
        shared_utils::logging::init_logging("vid_av1", shared_utils::logging::LogConfig::default());

    shared_utils::ctrlc_guard::init();

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
            force_ms_ssim_long,
            base_dir,
            allow_size_tolerance,
            no_allow_size_tolerance,
            verbose,
            resume,
            no_resume,
        } => {
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let resume = resume && !no_resume;

            if let Err(e) = shared_utils::validate_flags_result_with_ultimate(
                explore,
                match_quality,
                compress,
                ultimate,
            ) {
                eprintln!("{}", e);
                std::process::exit(1);
            }

            let base_dir =
                shared_utils::cli_runner::resolve_video_run_base_dir(&input, recursive, base_dir);

            let thread_config = shared_utils::thread_manager::get_balanced_thread_config(
                shared_utils::thread_manager::WorkloadType::Video,
            );

            let config = ConversionConfig {
                output_dir: output.clone(),
                base_dir,
                force,
                delete_original,
                explore_smaller: explore,
                use_lossless: false,
                match_quality,
                in_place,
                min_ssim: 0.95,
                require_compression: compress,
                apple_compat,
                use_gpu: true,
                force_ms_ssim_long,
                ultimate_mode: ultimate,
                child_threads: thread_config.child_threads,
                allow_size_tolerance,
            };

            shared_utils::progress_mode::set_verbose_mode(verbose);
            // Run 时自动创建并写入 ./logs/vid_av1_run_<timestamp>.log
            if let Err(e) = shared_utils::progress_mode::set_default_run_log_file("vid_av1") {
                shared_utils::log_eprintln!("⚠️  {}: {}", "\x1b[33mCould not open run log file\x1b[0m", e);
            }
            info!("🎬 Run Mode Conversion (AV1)");
            info!("   Lossless sources → AV1 Lossless");
            if match_quality {
                info!("   Lossy sources → AV1 MP4 (CRF auto-matched to input quality)");
            } else {
                info!("   Lossy sources → AV1 MP4 (CRF 20)");
            }
            if explore {
                info!("   📊 Size exploration: ENABLED");
            }
            if compress {
                info!("   📦 Compression: ENABLED");
            }
            if recursive {
                info!("   📂 Recursive: ENABLED");
            }
            if apple_compat {
                info!("   🍎 Apple Compatibility: ENABLED (⚠️ Note: AV1 not natively supported on older Apple devices)");
                std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1");
            }
            if ultimate {
                info!("   🔍 Ultimate Explore: ENABLED (search until SSIM saturates)");
            }
            if force_ms_ssim_long {
                info!("   ⚠️  Force MS-SSIM for long videos: ENABLED");
            }
            let cache = AnalysisCache::default_local().ok();
            if cache.is_some() {
                info!("   💽 Persistent Cache: ENABLED");
            }

            info!("");

            shared_utils::cli_runner::run_auto_command(
                shared_utils::cli_runner::CliRunnerConfig {
                    input: input.clone(),
                    output: output.clone(),
                    recursive,
                    label: "AV1 Video".to_string(),
                    base_dir: if output.is_some() {
                        Some(input.clone())
                    } else {
                        None
                    },
                    resume,
                },
                |file| auto_convert_with_cache(file, &config, cache.as_ref()).map_err(|e: VidQualityError| anyhow::anyhow!(e)),
            )?;
            shared_utils::progress_mode::xmp_merge_finalize();
            shared_utils::progress_mode::flush_log_file();
        }

        Commands::Strategy { input } => {
            let detection = detect_video_with_cache(&input, None)?;
            let strategy = determine_strategy(&detection);

            println!("\n🎯 Recommended Strategy (AV1 Auto Mode)");
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!("📁 File: {}", input.display());
            println!(
                "🎬 Codec: {} ({})",
                detection.codec.as_str(),
                detection.compression.as_str()
            );
            println!();
            println!("💡 Target: {}", strategy.target.as_str());
            println!("📝 Reason: {}", strategy.reason);
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        }
    }

    Ok(())
}
