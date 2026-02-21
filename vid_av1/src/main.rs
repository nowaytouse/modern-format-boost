use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use tracing::info;

use vid_av1::{auto_convert, detect_video, determine_strategy, ConversionConfig};

// 🔥 使用 shared_utils 的统计报告功能（模块化）

#[derive(Parser)]
#[command(name = "vidquality")]
#[command(version, about = "Video quality analyzer and format converter - FFV1 archival and AV1 compression", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze video properties
    Analyze {
        /// Input video file
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Output format
        #[arg(short, long, default_value = "human")]
        output: OutputFormat,
    },

    /// Run conversion: FFV1 for lossless, AV1 for lossy (intelligent selection); default explore+match_quality+compress
    #[command(name = "run")]
    Run {
        /// Input video file
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Output directory
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Force overwrite existing files
        #[arg(short, long)]
        force: bool,

        /// Recursive directory scan
        #[arg(short, long)]
        recursive: bool,

        /// Delete original after conversion
        #[arg(long)]
        delete_original: bool,

        /// In-place conversion: convert and delete original file
        /// Effectively "replaces" the original with the new format
        #[arg(long)]
        in_place: bool,

        /// Explore + match-quality + compress (default: on; required).
        #[arg(long, default_value_t = true)]
        explore: bool,

        /// Use mathematical lossless AV1 (⚠️ VERY SLOW, huge files)
        #[arg(long)]
        lossless: bool,

        /// Match input quality (default: on; required).
        #[arg(long, default_value_t = true)]
        match_quality: bool,

        /// Require compression (default: on; required).
        #[arg(long, default_value_t = true)]
        compress: bool,

        /// 🍎 Apple compatibility mode: Skip AV1 conversion (AV1 not natively supported on Apple devices)
        /// When enabled, shows a warning that AV1 files may not play on Apple devices
        #[arg(long, default_value_t = false)]
        apple_compat: bool,

        /// 🔥 v4.15: Force CPU encoding (libaom) instead of hardware acceleration
        /// Use --cpu for maximum quality (higher SSIM)
        #[arg(long, default_value_t = false)]
        cpu: bool,

        /// 🔥 v8.0: Base directory for output path generation (preserves directory structure)
        #[arg(long)]
        base_dir: Option<PathBuf>,

        /// 🔥 v8.0: Allow 1% size tolerance (default: enabled)
        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,

        /// Verbose output (show skipped files and success messages)
        #[arg(short, long)]
        verbose: bool,
    },

    /// Simple mode: ALL videos → AV1 MP4
    Simple {
        /// Input video file
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Output directory
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Use mathematical lossless AV1 (⚠️ VERY SLOW, huge files)
        #[arg(long)]
        lossless: bool,
    },

    /// Show recommended strategy without converting
    Strategy {
        /// Input video file
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable output
    Human,
    /// JSON output
    Json,
}

fn main() -> anyhow::Result<()> {
    // 🔥 v7.8: 使用统一的日志系统
    let _ = shared_utils::logging::init_logging(
        "vid_av1",
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
            cpu,
            base_dir,
            allow_size_tolerance,
            verbose,
        } => {
            // Determine base directory
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

            // 🔥 v7.9: Balanced thread config (AV1 always uses Video workload)
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
                // 🔥 v3.5: 裁判机制增强参数
                min_ssim: 0.95,          // 默认 SSIM 阈值
                validate_ms_ssim: false, // 默认不启用 VMAF（较慢）
                // 🔥 v7.6: MS-SSIM优化配置
                ms_ssim_sampling: None, // 自动选择
                full_ms_ssim: false,
                skip_ms_ssim: false,
                min_ms_ssim: 85.0,             // 默认 VMAF 阈值
                require_compression: compress, // 🔥 v4.6
                apple_compat,                  // 🍎 v4.15
                use_gpu: !cpu,                 // 🔥 v4.15: CPU mode = no GPU
                // HEVC flags (unused in AV1)
                force_ms_ssim_long: false,
                ultimate_mode: false,
                // 🔥 v7.9: Pass down thread limit
                child_threads: thread_config.child_threads,
                allow_size_tolerance,
                verbose,
            };

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
                info!("   🍎 Apple Compatibility: ENABLED (⚠️ Note: AV1 not natively supported on Apple devices)");
            }
            if cpu {
                info!("   🖥️  CPU Encoding: ENABLED (libaom for maximum SSIM)");
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
            info!("🎬 Simple Mode Conversion");
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

            println!("\n🎯 Recommended Strategy (Auto Mode)");
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
    println!("\n📊 Video Analysis Report");
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
