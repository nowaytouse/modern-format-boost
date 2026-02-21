use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use walkdir::WalkDir;
use serde_json;
use tracing::info;

// 使用 lib crate
use vid_hevc::{
    auto_convert, detect_video, determine_strategy, simple_convert, ConversionConfig,
    VideoDetectionResult,
};

// 🔥 使用 shared_utils 的统计报告功能（模块化）

#[derive(Parser)]
#[command(name = "vid-hevc")]
#[command(version, about = "Video quality analyzer and HEVC/H.265 converter", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze video properties
    Analyze {
        /// Input file or directory
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Recursive directory scan
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Output format
        #[arg(short, long, default_value = "human")]
        output: OutputFormat,
    },

    /// Run conversion: HEVC Lossless for lossless, HEVC CRF for lossy (default: explore + match_quality + compress + apple_compat + recursive + allow_size_tolerance)
    #[command(name = "run")]
    Run {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short, long)]
        force: bool,
        /// Recursive directory scan (always on; 强制递归)
        #[arg(short, long, default_value_t = true)]
        recursive: bool,
        #[arg(long)]
        delete_original: bool,
        /// In-place conversion: convert and delete original file
        #[arg(long)]
        in_place: bool,
        /// Size exploration + quality match + compression (default: on; required combination, no disable flag)
        #[arg(long, default_value_t = true)]
        explore: bool,
        #[arg(long)]
        lossless: bool,
        /// Match input video quality level (default: on; required, no disable flag)
        #[arg(long, default_value_t = true)]
        match_quality: bool,
        /// 🍎 Apple compatibility: AV1/VP9 → HEVC (default: on; use --no-apple-compat to disable)
        #[arg(long, default_value_t = true)]
        apple_compat: bool,
        /// Disable Apple compatibility mode
        #[arg(long)]
        no_apple_compat: bool,
        /// Require compression: output smaller than input (default: on; required, no disable flag)
        #[arg(long, default_value_t = true)]
        compress: bool,
        /// 🔥 Enable MS-SSIM verification (Multi-Scale SSIM, more accurate but slower)
        /// MS-SSIM is a perceptual quality metric with better correlation to human vision (0-1)
        #[arg(long, default_value_t = false)]
        ms_ssim: bool,
        /// 🔥 Minimum MS-SSIM score threshold (default: 0.90, range: 0-1)
        #[arg(long, default_value_t = 0.90)]
        ms_ssim_threshold: f64,
        /// 🔥 Force MS-SSIM verification even for long videos (>5min)
        /// By default, MS-SSIM is skipped for long videos to avoid slow processing
        #[arg(long, default_value_t = false)]
        force_ms_ssim_long: bool,
        /// 🔥 v7.6: MS-SSIM sampling rate (1/N, e.g., 3 for 1/3 sampling)
        /// Auto-selected by default based on video duration
        #[arg(long)]
        ms_ssim_sampling: Option<u32>,
        /// 🔥 v7.6: Force full MS-SSIM calculation (disable sampling)
        #[arg(long, default_value_t = false)]
        full_ms_ssim: bool,
        /// 🔥 v7.6: Skip MS-SSIM calculation entirely
        #[arg(long, default_value_t = false)]
        skip_ms_ssim: bool,
        /// 🔥 v6.2: Ultimate explore mode - search until SSIM fully saturates (Domain Wall)
        /// Uses adaptive wall limit based on CRF range, continues until no more quality gains
        /// ⚠️ MUST be used with --explore --match-quality --compress
        #[arg(long, default_value_t = false)]
        ultimate: bool,
        /// 🔥 v8.0: Base directory for output path generation (preserves directory structure)
        #[arg(long)]
        base_dir: Option<PathBuf>,
        /// Allow 1% size tolerance (default: on; use --no-allow-size-tolerance to disable)
        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,
        /// Disable 1% size tolerance
        #[arg(long)]
        no_allow_size_tolerance: bool,
        /// Verbose output (show skipped files and success messages)
        #[arg(short, long)]
        verbose: bool,
    },

    /// Simple mode: ALL videos → HEVC MP4
    Simple {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        lossless: bool,
    },

    /// Show recommended strategy without converting
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
    // 🔥 v7.8: 使用统一的日志系统
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
            recursive, // 强制递归，CLI 默认 true 且无 --no-recursive
            delete_original,
            in_place,
            explore,
            lossless,
            match_quality,
            apple_compat,
            no_apple_compat,
            compress,
            ms_ssim,
            ms_ssim_threshold,
            force_ms_ssim_long,
            ms_ssim_sampling,
            full_ms_ssim,
            skip_ms_ssim,
            ultimate,
            base_dir,
            allow_size_tolerance,
            no_allow_size_tolerance,
            verbose,
        } => {
            // Apply --no-* overrides (defaults are on; user turns off via --no-*). recursive 强制开启，无关闭项。
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;

            // 🔥 v6.2: Validate flag combinations with ultimate support
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

            let config = ConversionConfig {
                output_dir: output.clone(),
                base_dir: base_dir.clone(),
                force,
                delete_original,
                preserve_metadata: true,
                explore_smaller: explore,
                use_lossless: lossless,
                match_quality,
                in_place,
                apple_compat,
                require_compression: compress,
                use_gpu: true,
                validate_ms_ssim: ms_ssim,
                min_ms_ssim: ms_ssim_threshold,
                min_ssim: 0.95,
                force_ms_ssim_long,
                ultimate_mode: ultimate,
                // 🔥 v7.6: MS-SSIM优化参数
                ms_ssim_sampling,
                full_ms_ssim,
                skip_ms_ssim,
                // 🔥 v7.9: Balanced Thread Strategy (Video Mode)
                child_threads: shared_utils::thread_manager::get_balanced_thread_config(
                    shared_utils::thread_manager::WorkloadType::Video,
                )
                .child_threads,
                allow_size_tolerance,
                verbose,
            };

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
            if ms_ssim {
                info!(
                    "   📊 MS-SSIM Verification: ENABLED (threshold: {:.2})",
                    ms_ssim_threshold
                );
                if force_ms_ssim_long {
                    info!("   ⚠️  Force MS-SSIM for long videos: ENABLED");
                }
                // 🔥 v7.6: MS-SSIM优化信息
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
                    label: "HEVC Video".to_string(),
                    base_dir: base_dir.or_else(|| {
                        if output.is_some() {
                            Some(input.clone())
                        } else {
                            None
                        }
                    }), // 🔥 v8.0: Prefer explicit base_dir, fallback to input for adjacent mode
                },
                |file| auto_convert(file, &config).map_err(|e| e.into()),
            )?;
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
