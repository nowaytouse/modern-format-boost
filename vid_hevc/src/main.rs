use clap::{Parser, Subcommand, ValueEnum};
use serde_json;
use std::path::PathBuf;
use tracing::info;
use walkdir::WalkDir;

use vid_hevc::{
    auto_convert, detect_video, determine_strategy, simple_convert, ConversionConfig,
    VideoDetectionResult,
};

#[derive(Parser)]
#[command(name = "vid-hevc")]
#[command(version, about = "Video quality analyzer and HEVC/H.265 converter", long_about = None)]
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
    let _ = shared_utils::logging::init_logging(
        "vid_hevc",
        shared_utils::logging::LogConfig::default(),
    );

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
            apple_compat,
            no_apple_compat,
            compress,
            force_ms_ssim_long,
            ultimate,
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

            let base_dir =
                shared_utils::cli_runner::resolve_video_run_base_dir(&input, recursive, base_dir);

            let config = ConversionConfig {
                output_dir: output.clone(),
                base_dir: base_dir.clone(),
                force,
                delete_original,
                explore_smaller: explore,
                use_lossless: lossless,
                match_quality,
                in_place,
                apple_compat,
                require_compression: compress,
                use_gpu: true,
                min_ssim: 0.95,
                force_ms_ssim_long,
                ultimate_mode: ultimate,
                child_threads: shared_utils::thread_manager::get_balanced_thread_config(
                    shared_utils::thread_manager::WorkloadType::Video,
                )
                .child_threads,
                allow_size_tolerance,
            };

            shared_utils::progress_mode::set_verbose_mode(verbose);
            // Run 时自动创建并写入 ./logs/vid_hevc_run_<timestamp>.log，无需任何 flag
            if let Err(e) = shared_utils::progress_mode::set_default_run_log_file("vid_hevc") {
                eprintln!("⚠️  Could not open run log file: {}", e);
            }
            info!("🎬 Run Mode Conversion (HEVC/H.265)");
            info!("   Lossless sources → HEVC Lossless MKV");
            if match_quality {
                info!("   Lossy sources → HEVC MP4 (CRF auto-matched to input quality)");
            } else {
                info!("   Lossy sources → HEVC MP4 (CRF 18-20)");
            }
            if lossless {
                info!("   ⚠️  HEVC Lossless: ENABLED");
            }
            if explore {
                info!("   📊 Size exploration: ENABLED");
            }
            if match_quality {
                info!("   🎯 Match Quality: ENABLED");
            }
            if apple_compat {
                info!("   🍎 Apple Compatibility: ENABLED (AV1/VP9 → HEVC)");
                std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1");
            }
            if recursive {
                info!("   📂 Recursive: ENABLED");
            }
            if ultimate {
                info!("   🔥 Ultimate Explore: ENABLED (search until SSIM saturates)");
            }
            if force_ms_ssim_long {
                info!("   ⚠️  Force MS-SSIM for long videos: ENABLED");
            }
            info!("");

            shared_utils::cli_runner::run_auto_command(
                shared_utils::cli_runner::CliRunnerConfig {
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
                },
                |file| auto_convert(file, &config).map_err(|e| e.into()),
            )?;
            shared_utils::progress_mode::xmp_merge_finalize();
            shared_utils::progress_mode::flush_log_file();
        }

        Commands::Simple {
            input,
            output,
            lossless: _,
        } => {
            info!("🎬 Simple Mode Conversion (HEVC/H.265)");
            info!("   ALL videos → HEVC MP4 (CRF 18)");
            info!("");

            let result = simple_convert(&input, output.as_deref())?;

            info!("");
            info!("✅ Complete!");
            info!("   Output: {}", result.output_path);
            info!("   Size: {:.1}% of original", result.size_ratio * 100.0);
        }

        Commands::Strategy { input } => {
            let detection = detect_video(&input)?;
            let strategy = determine_strategy(&detection);

            println!("\n🎯 Recommended Strategy (HEVC Auto Mode)");
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

fn print_analysis_human(result: &VideoDetectionResult) {
    println!("\n📊 Video Analysis Report (HEVC)");
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
