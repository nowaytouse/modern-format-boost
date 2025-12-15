use clap::{Parser, Subcommand, ValueEnum};
use tracing::info;
use std::path::PathBuf;
use std::time::Instant;

// 使用 lib crate
use vidquality_hevc::{
    detect_video, auto_convert, simple_convert, determine_strategy, 
    ConversionConfig, VideoDetectionResult
};

// 🔥 使用 shared_utils 的统计报告功能（模块化）
use shared_utils::{print_summary_report, BatchResult};

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
        /// Use --match-quality true to enable, --match-quality false to disable
        #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
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
        /// 🔥 v4.15: Force CPU encoding (libx265) instead of GPU
        /// VideoToolbox hardware encoding caps at ~0.95 SSIM. Use --cpu to achieve 0.98+ SSIM
        #[arg(long, default_value_t = false)]
        cpu: bool,
        /// 🔥 v5.75: Enable VMAF verification (slower but more accurate)
        /// VMAF is Netflix's perceptual quality metric (0-100)
        #[arg(long, default_value_t = false)]
        vmaf: bool,
        /// 🔥 v5.75: Minimum VMAF score threshold (default: 85.0)
        #[arg(long, default_value_t = 85.0)]
        vmaf_threshold: f64,
        /// 🔥 v5.75: Force VMAF verification even for long videos (>5min)
        /// By default, VMAF is skipped for long videos to avoid slow processing
        #[arg(long, default_value_t = false)]
        force_vmaf_long: bool,
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

        Commands::Auto { input, output, force, recursive, delete_original, in_place, explore, lossless, match_quality, apple_compat, compress, cpu, vmaf, vmaf_threshold, force_vmaf_long } => {
            // 🔥 v5.1: Validate flag combinations for consistency
            if let Err(e) = shared_utils::validate_flags_result(explore, match_quality, compress) {
                eprintln!("{}", e);
                std::process::exit(1);
            }

            let config = ConversionConfig {
                output_dir: output.clone(),
                force,
                delete_original,
                preserve_metadata: true,
                explore_smaller: explore,
                use_lossless: lossless,
                match_quality,
                in_place,
                apple_compat,
                require_compression: compress,
                use_gpu: !cpu,  // 🔥 v4.15: CPU mode = no GPU
                // 🔥 v5.75: VMAF 验证参数
                validate_vmaf: vmaf,
                min_vmaf: vmaf_threshold,
                force_vmaf_long,
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
            if cpu {
                info!("   🖥️  CPU Encoding: ENABLED (libx265 for SSIM ≥0.98)");
            }
            if vmaf {
                info!("   📊 VMAF Verification: ENABLED (threshold: {:.1})", vmaf_threshold);
                if force_vmaf_long {
                    info!("   ⚠️  Force VMAF for long videos: ENABLED");
                }
            }
            info!("");
            
            if input.is_dir() {
                use walkdir::WalkDir;
                let video_extensions = ["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "mpg", "mpeg", "ts", "mts"];
                
                // 🔥 支持递归目录遍历
                let walker = if recursive {
                    WalkDir::new(&input).follow_links(true)
                } else {
                    WalkDir::new(&input).max_depth(1)
                };
                
                let files: Vec<_> = walker
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file())
                    .filter(|e| {
                        if let Some(ext) = e.path().extension() {
                            video_extensions.contains(&ext.to_str().unwrap_or("").to_lowercase().as_str())
                        } else {
                            false
                        }
                    })
                    .map(|e| e.path().to_path_buf())
                    .collect();
                
                // 🔥 响亮报错：目录中没有视频文件
                if files.is_empty() {
                    anyhow::bail!(
                        "❌ 目录中没有找到视频文件: {}\n\
                         💡 支持的视频格式: {}\n\
                         💡 如果要处理图像，请使用 imgquality 工具",
                        input.display(),
                        video_extensions.join(", ")
                    );
                }
                
                info!("📂 Found {} video files to process", files.len());
                
                // 🔥 使用 shared_utils 的 BatchResult 进行统计（模块化）
                let start_time = Instant::now();
                let mut batch_result = BatchResult::new();
                let mut total_input_bytes: u64 = 0;
                let mut total_output_bytes: u64 = 0;
                
                for file in &files {
                    match auto_convert(file, &config) {
                        Ok(result) => {
                            // 🔥 修复：区分跳过和真正成功的转换
                            if result.output_size == 0 && result.output_path.is_empty() {
                                // 跳过的文件（已是现代编码）
                                info!("⏭️ {} → SKIP ({:.1}%)", 
                                    file.file_name().unwrap_or_default().to_string_lossy(),
                                    result.size_ratio * 100.0
                                );
                                batch_result.skip();
                            } else {
                                // 真正成功的转换
                                info!("✅ {} → {} ({:.1}%)", 
                                    file.file_name().unwrap_or_default().to_string_lossy(),
                                    result.output_path,
                                    result.size_ratio * 100.0
                                );
                                batch_result.success();
                                total_input_bytes += result.input_size;
                                total_output_bytes += result.output_size;
                            }
                        }
                        Err(e) => {
                            // 🔥 修复：将"Output exists"错误视为跳过而非失败
                            let error_msg = e.to_string();
                            if error_msg.contains("Output exists:") {
                                info!("⏭️ {} → SKIP (output exists)", 
                                    file.file_name().unwrap_or_default().to_string_lossy()
                                );
                                batch_result.skip();
                            } else {
                                info!("❌ {} failed: {}", file.display(), e);
                                batch_result.fail(file.clone(), e.to_string());
                            }
                        }
                    }
                }
                
                // 🔥 使用 shared_utils 的统一报告格式（模块化）
                print_summary_report(
                    &batch_result,
                    start_time.elapsed(),
                    total_input_bytes,
                    total_output_bytes,
                    "HEVC Video",
                );
            } else {
                // 🔥 单文件处理：先检查是否是视频文件
                let video_extensions = ["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "mpg", "mpeg", "ts", "mts"];
                let ext = input.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();
                
                if !video_extensions.contains(&ext.as_str()) {
                    anyhow::bail!(
                        "❌ 不是视频文件: {}\n\
                         💡 文件扩展名: .{}\n\
                         💡 支持的视频格式: {}\n\
                         💡 如果要处理图像，请使用 imgquality 工具",
                        input.display(),
                        ext,
                        video_extensions.join(", ")
                    );
                }
                
                let result = auto_convert(&input, &config)?;
                
                info!("");
                info!("📊 Conversion Summary:");
                info!("   Input:  {} ({} bytes)", result.input_path, result.input_size);
                info!("   Output: {} ({} bytes)", result.output_path, result.output_size);
                info!("   Ratio:  {:.1}%", result.size_ratio * 100.0);
                if result.exploration_attempts > 0 {
                    info!("   🔍 Explored {} CRF values, final: CRF {:.1}", result.exploration_attempts, result.final_crf);
                }
            }
        }

        Commands::Simple { input, output, lossless: _ } => {
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
            println!("🎬 Codec: {} ({})", detection.codec.as_str(), detection.compression.as_str());
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
