use clap::{Parser, Subcommand, ValueEnum};
use tracing::info;
use std::path::PathBuf;

use vidquality_av1::{detect_video, auto_convert, determine_strategy, ConversionConfig};

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

    /// Auto mode: FFV1 for lossless, AV1 for lossy (intelligent selection)
    Auto {
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

        /// Explore smaller size (try higher CRF if output > input)
        #[arg(long)]
        explore: bool,

        /// Use mathematical lossless AV1 (⚠️ VERY SLOW, huge files)
        #[arg(long)]
        lossless: bool,

        /// Match input video quality level (auto-calculate CRF based on input bitrate)
        /// Use --match-quality true to enable, --match-quality false to disable
        #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
        match_quality: bool,
        
        /// 🔥 Require compression: output must be smaller than input
        /// Use with --explore --match-quality for precise quality match + guaranteed compression
        #[arg(long, default_value_t = false)]
        compress: bool,

        /// 🍎 Apple compatibility mode: Skip AV1 conversion (AV1 not natively supported on Apple devices)
        /// When enabled, shows a warning that AV1 files may not play on Apple devices
        #[arg(long, default_value_t = false)]
        apple_compat: bool,

        /// 🔥 v4.15: Force CPU encoding (libaom) instead of hardware acceleration
        /// Use --cpu for maximum quality (higher SSIM)
        #[arg(long, default_value_t = false)]
        cpu: bool,
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

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

        Commands::Auto { input, output, force, recursive, delete_original, in_place, explore, lossless, match_quality, compress, apple_compat, cpu } => {
            // Determine base directory
            let base_dir = if recursive {
                if input.is_dir() { Some(input.clone()) } else { input.parent().map(|p| p.to_path_buf()) }
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
                // 🔥 v3.5: 裁判机制增强参数
                min_ssim: 0.95,       // 默认 SSIM 阈值
                validate_ms_ssim: false, // 默认不启用 VMAF（较慢）
                min_ms_ssim: 85.0,       // 默认 VMAF 阈值
                require_compression: compress, // 🔥 v4.6
                apple_compat,         // 🍎 v4.15
                use_gpu: !cpu,        // 🔥 v4.15: CPU mode = no GPU
                // HEVC flags (unused in AV1)
                force_ms_ssim_long: false,
                ultimate_mode: false,
            };
            
            info!("🎬 Auto Mode Conversion (AV1)");
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
                },
                |file| auto_convert(file, &config).map_err(|e| e.into())
            )?;
        }

        Commands::Simple { input, output, lossless: _ } => {
            info!("🎬 Simple Mode Conversion");
            info!("   ⚠️  ALL videos → AV1 MP4 (MATHEMATICAL LOSSLESS - VERY SLOW!)");
            info!("   (Note: Simple mode now enforces lossless conversion by default)");
            info!("");
            
            let result = vidquality_av1::simple_convert(&input, output.as_deref())?;
            
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
            println!("🎬 Codec: {} ({})", detection.codec.as_str(), detection.compression.as_str());
            println!();
            println!("💡 Target: {}", strategy.target.as_str());
            println!("📝 Reason: {}", strategy.reason);
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        }
    }

    Ok(())
}

fn print_analysis_human(result: &vidquality_av1::VideoDetectionResult) {
    println!("\n📊 Video Analysis Report");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📁 File: {}", result.file_path);
    println!("📦 Format: {}", result.format);
    println!("🎬 Codec: {} ({})", result.codec.as_str(), result.codec_long);
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
    println!("🎵 Audio: {}", if result.has_audio { 
        result.audio_codec.as_deref().unwrap_or("yes") 
    } else { 
        "no" 
    });
    println!();
    println!("⭐ Quality Score: {}/100", result.quality_score);
    println!("📦 Archival Candidate: {}", if result.archival_candidate { "✅ Yes" } else { "❌ No" });
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}
