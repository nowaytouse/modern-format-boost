use clap::{Parser, Subcommand, ValueEnum};
use serde_json;
use std::path::PathBuf;
use tracing::info;
use walkdir::WalkDir;

use vid_av1::{auto_convert, detect_video, determine_strategy, ConversionConfig};

#[derive(Parser)]
#[command(name = "vid-av1")]
#[command(version, about = "Video quality analyzer and format converter - AV1 compression", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Analyze {
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        #[arg(short, long, default_value = "human")]
        output: OutputFormat,
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

        #[arg(long)]
        lossless: bool,

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
        ms_ssim: bool,

        #[arg(long, default_value_t = 0.90)]
        ms_ssim_threshold: f64,

        #[arg(long, default_value_t = false)]
        force_ms_ssim_long: bool,

        #[arg(long)]
        ms_ssim_sampling: Option<u32>,

        #[arg(long, default_value_t = false)]
        full_ms_ssim: bool,

        #[arg(long, default_value_t = false)]
        skip_ms_ssim: bool,

        #[arg(long, default_value_t = false)]
        cpu: bool,

        #[arg(long)]
        base_dir: Option<PathBuf>,

        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,

        #[arg(long)]
        no_allow_size_tolerance: bool,

        #[arg(short, long)]
        verbose: bool,
    },

    Simple {
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(short, long)]
        output: Option<PathBuf>,

        #[arg(long)]
        lossless: bool,
    },

    Strategy {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Human,
    Json,
}

fn main() -> anyhow::Result<()> {
    let _ =
        shared_utils::logging::init_logging("vid_av1", shared_utils::logging::LogConfig::default());

    let cli = Cli::parse();

    match cli.command {
        Commands::Analyze {
            input,
            recursive,
            output,
        } => {
            if input.is_file() {
                let result = detect_video(&input)?;
                match output {
                    OutputFormat::Human => print_analysis_human(&result),
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&result)?);
                    }
                }
            } else if input.is_dir() {
                analyze_directory(&input, recursive, output)?;
            } else {
                eprintln!("❌ Error: Input path does not exist: {}", input.display());
                std::process::exit(1);
            }
        }

        Commands::Run {
            input,
            output,
            force,
            recursive,
            delete_original,
            in_place,
            explore,
            lossless,
            match_quality,
            compress,
            apple_compat,
            no_apple_compat,
            ultimate,
            ms_ssim,
            ms_ssim_threshold,
            force_ms_ssim_long,
            ms_ssim_sampling,
            full_ms_ssim,
            skip_ms_ssim,
            cpu,
            base_dir,
            allow_size_tolerance,
            no_allow_size_tolerance,
            verbose,
        } => {
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;

            if let Err(e) = shared_utils::validate_flags_result_with_ultimate(
                explore,
                match_quality,
                compress,
                ultimate,
            ) {
                eprintln!("{}", e);
                std::process::exit(1);
            }

            let base_dir = if let Some(explicit_base) = base_dir {
                Some(explicit_base)
            } else if recursive {
                if input.is_dir() {
                    Some(input.clone())
                } else {
                    input.parent().map(|p| p.to_path_buf())
                }
            } else {
                input.parent().map(|p| p.to_path_buf())
            };

            let thread_config = shared_utils::thread_manager::get_balanced_thread_config(
                shared_utils::thread_manager::WorkloadType::Video,
            );

            let config = ConversionConfig {
                output_dir: output.clone(),
                base_dir,
                force,
                delete_original,
                preserve_metadata: true,
                explore_smaller: explore,
                use_lossless: lossless,
                match_quality,
                in_place,
                min_ssim: 0.95,
                validate_ms_ssim: ms_ssim,
                ms_ssim_sampling,
                full_ms_ssim,
                skip_ms_ssim,
                min_ms_ssim: ms_ssim_threshold,
                require_compression: compress,
                apple_compat,
                use_gpu: !cpu,
                force_ms_ssim_long,
                ultimate_mode: ultimate,
                child_threads: thread_config.child_threads,
                allow_size_tolerance,
                verbose,
            };

            shared_utils::progress_mode::set_verbose_mode(verbose);
            info!("🎬 Run Mode Conversion (AV1)");
            info!("   Lossless sources → AV1 Lossless");
            info!("   Lossy sources → AV1 MP4 (CRF auto-matched to input quality)");
            if match_quality {
                info!("   🎯 Match Quality: ENABLED");
            }
            if lossless {
                info!("   ⚠️  Mathematical lossless AV1: ENABLED (VERY SLOW!)");
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
                info!("   🔥 Ultimate Explore: ENABLED (search until SSIM saturates)");
            }
            if cpu {
                info!("   🖥️  CPU Encoding: ENABLED (libaom for maximum SSIM)");
            }
            if ms_ssim {
                info!(
                    "   📊 MS-SSIM Verification: ENABLED (threshold: {:.2})",
                    ms_ssim_threshold
                );
                if force_ms_ssim_long {
                    info!("   ⚠️  Force MS-SSIM for long videos: ENABLED");
                }
                if skip_ms_ssim {
                    eprintln!("⚠️  Warning: --skip-ms-ssim conflicts with --ms-ssim, MS-SSIM will be skipped");
                } else if full_ms_ssim {
                    info!("   🔥 Full MS-SSIM: ENABLED (no sampling)");
                } else if let Some(rate) = ms_ssim_sampling {
                    info!("   📊 MS-SSIM Sampling: 1/{} frames", rate);
                } else {
                    info!("   📊 MS-SSIM Sampling: AUTO (based on video duration)");
                }
            } else if skip_ms_ssim {
                info!("   ⏭️  MS-SSIM: SKIPPED");
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
                },
                |file| auto_convert(file, &config).map_err(|e| e.into()),
            )?;
            shared_utils::progress_mode::xmp_merge_finalize();
        }

        Commands::Simple {
            input,
            output,
            lossless: _,
        } => {
            info!("🎬 Simple Mode Conversion (AV1)");
            info!("   ⚠️  ALL videos → AV1 MP4 (MATHEMATICAL LOSSLESS - VERY SLOW!)");
            info!("   (Note: Simple mode now enforces lossless conversion by default)");
            info!("");

            let result = vid_av1::simple_convert(&input, output.as_deref())?;

            info!("");
            info!("✅ Complete!");
            info!("   Output: {}", result.output_path);
            info!("   Size: {:.1}% of original", result.size_ratio * 100.0);
        }

        Commands::Strategy { input } => {
            let detection = detect_video(&input)?;
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

fn print_analysis_human(result: &vid_av1::VideoDetectionResult) {
    println!("\n📊 Video Analysis Report (AV1)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📁 File: {}", result.file_path);
    println!("📦 Format: {}", result.format);
    println!(
        "🎬 Codec: {} ({})",
        result.codec.as_str(),
        result.codec_long
    );
    println!("🔍 Compression: {}", result.compression.as_str());
    println!();
    println!("📐 Resolution: {}x{}", result.width, result.height);
    println!("🎞️  Frames: {} @ {:.2} fps", result.frame_count, result.fps);
    println!("⏱️  Duration: {:.2}s", result.duration_secs);
    println!("🎨 Bit Depth: {}-bit", result.bit_depth);
    println!("🌈 Pixel Format: {}", result.pix_fmt);
    println!();
    println!("💾 File Size: {} bytes", result.file_size);
    println!("📊 Bitrate: {} bps", result.bitrate);
    println!(
        "🎵 Audio: {}",
        if result.has_audio {
            result.audio_codec.as_deref().unwrap_or("yes")
        } else {
            "no"
        }
    );
    println!();
    println!("⭐ Quality Score: {}/100", result.quality_score);
    println!(
        "📦 Archival Candidate: {}",
        if result.archival_candidate {
            "✅ Yes"
        } else {
            "❌ No"
        }
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

fn analyze_directory(
    path: &PathBuf,
    recursive: bool,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let walker = if recursive {
        WalkDir::new(path).follow_links(true)
    } else {
        WalkDir::new(path).max_depth(1)
    };

    let mut results = Vec::new();
    let mut count = 0;

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        if let Some(ext) = path.extension() {
            if shared_utils::file_copier::SUPPORTED_VIDEO_EXTENSIONS
                .contains(&ext.to_str().unwrap_or("").to_lowercase().as_str())
            {
                match detect_video(path) {
                    Ok(analysis) => {
                        count += 1;
                        if output_format == OutputFormat::Json {
                            let result = serde_json::to_value(&analysis)?;
                            results.push(result);
                        } else {
                            println!("\n{}", "=".repeat(80));
                            print_analysis_human(&analysis);
                        }
                    }
                    Err(e) => {
                        eprintln!("⚠️  Failed to analyze {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    if output_format == OutputFormat::Json {
        println!(
            "{}",
            serde_json::json!({
                "total": count,
                "results": results
            })
        );
    } else {
        println!("\n{}", "=".repeat(80));
        println!("✅ Analysis complete: {} files processed", count);
    }

    Ok(())
}
