use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use tracing::info;

// 使用 lib crate
use vidquality_hevc::{
    auto_convert, detect_video, determine_strategy, simple_convert, ConversionConfig,
    VideoDetectionResult,
};

// 🔥 使用 shared_utils 的统计报告功能（模块化）

#[derive(Parser)]
#[command(name = "vidquality-hevc")]
#[command(version, about = "Video quality analyzer and HEVC/H.265 converter", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze video properties
    Analyze {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long, default_value = "human")]
        output: OutputFormat,
    },

    /// Auto mode: HEVC Lossless for lossless, HEVC CRF for lossy
    Auto {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short, long)]
        force: bool,
        /// Recursive directory scan
        #[arg(short, long)]
        recursive: bool,
        #[arg(long)]
        delete_original: bool,
        /// In-place conversion: convert and delete original file
        #[arg(long)]
        in_place: bool,
        #[arg(long)]
        explore: bool,
        #[arg(long)]
        lossless: bool,
        /// Match input video quality level (auto-calculate CRF based on input bitrate)
        /// Use --match-quality to enable
        #[arg(long)]
        match_quality: bool,
        /// 🍎 Apple compatibility mode: Convert non-Apple-compatible modern codecs (AV1, VP9) to HEVC
        /// When enabled, AV1/VP9/VVC/AV2 videos will be converted to HEVC for Apple device compatibility
        /// Only HEVC videos will be skipped (already Apple compatible)
        #[arg(long, default_value_t = false)]
        apple_compat: bool,
        /// 🔥 Require compression: output must be smaller than input
        /// Use with --explore --match-quality for precise quality match + guaranteed compression
        #[arg(long, default_value_t = false)]
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
        "vidquality_hevc",
        shared_utils::logging::LogConfig::default(),
    );

    let cli = Cli::parse();

    match cli.command {
        Commands::Analyze { input, output } => {
            let result = detect_video(&input)?;
            match output {
                OutputFormat::Human => print_analysis_human(&result),
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            }
        }

        Commands::Auto {
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
            compress,
            ms_ssim,
            ms_ssim_threshold,
            force_ms_ssim_long,
            ms_ssim_sampling,
            full_ms_ssim,
            skip_ms_ssim,
            ultimate,
        } => {
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

            let base_dir = if recursive {
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
                base_dir,
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
                    shared_utils::thread_manager::WorkloadType::Video
                ).child_threads,
            };

            info!("🎬 Auto Mode Conversion (HEVC/H.265)");
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
                    base_dir: if output.is_some() {
                        Some(input.clone())
                    } else {
                        None
                    }, // 🔥 v7.4.5
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
