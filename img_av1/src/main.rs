use clap::{Parser, Subcommand, ValueEnum};
use img_av1::{analyze_image, get_recommendation};
use img_av1::{calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description};
use rayon::prelude::*;
use serde_json::json;
use shared_utils::{check_dangerous_directory, print_summary_report, BatchResult};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use walkdir::WalkDir;

use img_av1::conversion_api::ConversionOutput;

/// Configuration for auto-convert operations
#[derive(Clone)]
struct AutoConvertConfig {
    output_dir: Option<PathBuf>,
    force: bool,
    recursive: bool,
    delete_original: bool,
    in_place: bool,
    lossless: bool,
    explore: bool,
    match_quality: bool,
    compress: bool,
    apple_compat: bool,
    /// 🔥 v4.15: Use GPU acceleration (default: true)
    use_gpu: bool,
    /// 🔥 v6.2: 极限探索模式（AV1 暂不支持 Domain Wall，但保留 flag 以对齐接口）
    ultimate: bool,
    /// Verbose output
    verbose: bool,
    /// Base directory for relative path preservation
    base_dir: Option<PathBuf>,
    /// 🔥 v7.9: Balanced thread config
    child_threads: usize,
    /// 🔥 v8.3: Allow 1% size tolerance
    allow_size_tolerance: bool,
}

#[derive(Parser)]
#[command(name = "img-av1")]
#[command(version, about = "Image quality analyzer and format upgrade tool - AV1/AVIF", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze image quality parameters
    Analyze {
        /// Input file or directory
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Recursive directory scan
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Output format
        #[arg(short, long, value_enum, default_value = "human")]
        output: OutputFormat,

        /// Include upgrade recommendation
        #[arg(short = 'R', long)]
        recommend: bool,
    },

    /// Run conversion: format-based (JPEG→JXL, PNG→JXL, Animated→AV1 MP4); default explore+match_quality+compress
    #[command(name = "run")]
    Run {
        /// Output directory (default: same as input)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Base directory for preserving directory structure (optional)
        #[arg(long)]
        base_dir: Option<PathBuf>,

        /// Input file or directory
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Force conversion even if already processed
        #[arg(short, long)]
        force: bool,

        /// Recursive directory scan (always on; 强制递归)
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Delete original after successful conversion
        #[arg(long)]
        delete_original: bool,

        /// In-place conversion: convert and delete original file
        /// Effectively "replaces" the original with the new format
        /// Example: image.png → image.jxl (original .png deleted)
        #[arg(long)]
        in_place: bool,

        /// Use mathematical lossless AV1 (⚠️ VERY SLOW, huge files)
        #[arg(long)]
        lossless: bool,

        /// Explore + match-quality + compress (default: on; required for animated→video).
        #[arg(long, default_value_t = true)]
        explore: bool,

        /// Match input quality (default: on; required).
        #[arg(long, default_value_t = true)]
        match_quality: bool,

        /// Require compression for animated→video (default: on; required).
        #[arg(long, default_value_t = true)]
        compress: bool,

        /// 🍎 Apple compatibility mode: Convert non-Apple-compatible animated formats to AV1
        /// When enabled, animated WebP (VP8/VP9) will be converted to AV1 MP4
        /// instead of being skipped as "modern format"
        #[arg(long, default_value_t = true)]
        apple_compat: bool,

        /// Disable Apple compatibility mode
        #[arg(long)]
        no_apple_compat: bool,

        /// Uses adaptive wall limit based on CRF range, continues until no more quality gains
        /// ⚠️ MUST be used with --explore --match-quality --compress
        #[arg(long, default_value_t = false)]
        ultimate: bool,

        /// 🔥 v4.15: Force CPU encoding (libaom) instead of GPU
        /// Hardware encoding may have lower quality ceiling. Use --cpu for maximum SSIM
        #[arg(long, default_value_t = false)]
        cpu: bool,

        /// Verbose output (show skipped files and success messages)
        #[arg(short, long)]
        verbose: bool,

        /// 🔥 v7.9: Max threads for child processes (ffmpeg/cjxl/x265)
        #[arg(long, default_value_t = 0)]
        child_threads: usize,

        /// 🔥 v8.3: Allow 1% size tolerance (default: enabled)
        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,

        /// Disable 1% size tolerance
        #[arg(long)]
        no_allow_size_tolerance: bool,
    },

    /// Verify conversion quality
    Verify {
        /// Original file
        original: PathBuf,

        /// Converted file
        converted: PathBuf,
    },

    /// 从源目录恢复输出目录的时间戳（目录+文件）
    /// 供脚本在后处理（如 JXL Container Fix）后调用，逻辑在 shared_utils，此处仅转发
    RestoreTimestamps {
        /// 源目录（如 test）
        #[arg(value_name = "SOURCE_DIR")]
        source: PathBuf,

        /// 输出目录（如 test_optimized）
        #[arg(value_name = "OUTPUT_DIR")]
        output: PathBuf,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable output
    Human,
    /// JSON output (for API use)
    Json,
}

fn main() -> anyhow::Result<()> {
    // 🔥 v7.8: 初始化日志系统
    let _ =
        shared_utils::logging::init_logging("img_av1", shared_utils::logging::LogConfig::default());

    let cli = Cli::parse();

    match cli.command {
        Commands::Analyze {
            input,
            recursive,
            output,
            recommend,
        } => {
            if input.is_file() {
                analyze_single_file(&input, output, recommend)?;
            } else if input.is_dir() {
                analyze_directory(&input, recursive, output, recommend)?;
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
            lossless,
            explore,
            match_quality,
            compress,
            apple_compat,
            no_apple_compat,
            ultimate,
            cpu,
            base_dir,
            verbose,
            child_threads,
            allow_size_tolerance,
            no_allow_size_tolerance,
        } => {
            // Apply --no-apple-compat override
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            // in_place implies delete_original
            let should_delete = delete_original || in_place;

            // 🔥 v6.2: 使用模块化的 flag 验证器（含 ultimate 支持）
            let flag_mode = match shared_utils::validate_flags_result_with_ultimate(
                explore,
                match_quality,
                compress,
                ultimate,
            ) {
                Ok(mode) => mode,
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            };

            if lossless {
                eprintln!("⚠️  Mathematical lossless mode: ENABLED (VERY SLOW!)");
                eprintln!("   Smart quality matching: DISABLED");
            } else if verbose {
                eprintln!("🎬 {} (for animated→video)", flag_mode.description_cn());
                eprintln!("📷 Static images: Always lossless (JPEG→JXL, PNG→JXL)");
            }
            if apple_compat {
                eprintln!("🍎 Apple Compatibility: ENABLED (animated WebP → AV1)");
                std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1");
            }
            if in_place {
                eprintln!(
                    "🔄 In-place mode: ENABLED (original files will be deleted after conversion)"
                );
            }
            if ultimate {
                eprintln!("🔥 Ultimate Explore: ENABLED (search until SSIM saturates)");
            }
            if !allow_size_tolerance {
                eprintln!(
                    "📏 Size Tolerance: DISABLED (output must be strictly smaller than input)"
                );
            }
            if cpu {
                eprintln!("🖥️  CPU Encoding: ENABLED (libaom for maximum SSIM)");
            }

            // 🔥 v7.9: Calculate balanced thread configuration
            let workload = if input.is_dir() {
                shared_utils::thread_manager::WorkloadType::Image
            } else {
                shared_utils::thread_manager::WorkloadType::Video
            };
            let thread_config = shared_utils::thread_manager::get_balanced_thread_config(workload);

            let config = AutoConvertConfig {
                output_dir: output.clone(),
                force,
                recursive,
                delete_original: should_delete,
                in_place,
                lossless,
                explore,
                match_quality,
                compress,
                apple_compat,
                use_gpu: !cpu,
                ultimate,
                verbose,
                base_dir: base_dir.clone(),
                child_threads: if child_threads > 0 {
                    child_threads
                } else {
                    thread_config.child_threads
                },
                allow_size_tolerance,
            };

            if input.is_file() {
                auto_convert_single_file(&input, &config)?;
            } else if input.is_dir() {
                auto_convert_directory(&input, &config)?;
            } else {
                eprintln!("❌ Error: Input path does not exist: {}", input.display());
                std::process::exit(1);
            }
        }

        Commands::Verify {
            original,
            converted,
        } => {
            verify_conversion(&original, &converted)?;
        }

        Commands::RestoreTimestamps { source, output } => {
            if let Err(e) = shared_utils::restore_timestamps_from_source_to_output(&source, &output)
            {
                eprintln!("⚠️ restore-timestamps failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn analyze_single_file(
    path: &Path,
    output_format: OutputFormat,
    recommend: bool,
) -> anyhow::Result<()> {
    let analysis = analyze_image(path)?;

    if output_format == OutputFormat::Json {
        let mut result = serde_json::to_value(&analysis)?;

        if recommend {
            let recommendation = get_recommendation(&analysis);
            result["recommendation"] = serde_json::to_value(&recommendation)?;
        }

        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_analysis_human(&analysis);

        if recommend {
            let recommendation = get_recommendation(&analysis);
            print_recommendation_human(&recommendation);
        }
    }

    Ok(())
}

fn analyze_directory(
    path: &PathBuf,
    recursive: bool,
    output_format: OutputFormat,
    recommend: bool,
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
            if shared_utils::IMAGE_EXTENSIONS_ANALYZE
                .contains(&ext.to_str().unwrap_or("").to_lowercase().as_str())
            {
                // 🔥 v7.9: Validate file integrity first
                if let Err(e) = shared_utils::common_utils::validate_file_integrity(path) {
                    eprintln!("⚠️  Skipping invalid file {}: {}", path.display(), e);
                    continue;
                }

                match analyze_image(path) {
                    Ok(analysis) => {
                        count += 1;
                        if output_format == OutputFormat::Json {
                            let mut result = serde_json::to_value(&analysis)?;
                            if recommend {
                                let recommendation = get_recommendation(&analysis);
                                result["recommendation"] = serde_json::to_value(&recommendation)?;
                            }
                            results.push(result);
                        } else {
                            println!("\n{}", "=".repeat(80));
                            print_analysis_human(&analysis);
                            if recommend {
                                let recommendation = get_recommendation(&analysis);
                                print_recommendation_human(&recommendation);
                            }
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
            json!({
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

fn verify_conversion(original: &PathBuf, converted: &PathBuf) -> anyhow::Result<()> {
    println!("🔍 Verifying conversion quality...");
    println!("   Original:  {}", original.display());
    println!("   Converted: {}", converted.display());

    let original_analysis = analyze_image(original)?;
    let converted_analysis = analyze_image(converted)?;

    println!("\n📊 Size Comparison:");
    println!(
        "   Original size:  {} bytes ({:.2} KB)",
        original_analysis.file_size,
        original_analysis.file_size as f64 / 1024.0
    );
    println!(
        "   Converted size: {} bytes ({:.2} KB)",
        converted_analysis.file_size,
        converted_analysis.file_size as f64 / 1024.0
    );

    let reduction =
        100.0 * (1.0 - converted_analysis.file_size as f64 / original_analysis.file_size as f64);
    println!("   Size reduction: {:.2}%", reduction);

    // Load images for quality comparison
    let orig_img = load_image_safe(original)?;
    let conv_img = load_image_safe(converted)?;

    println!("\n📏 Quality Metrics:");
    if let Some(psnr) = calculate_psnr(&orig_img, &conv_img) {
        if psnr.is_infinite() {
            println!("   PSNR: ∞ dB (Identical - mathematically lossless)");
        } else {
            println!(
                "   PSNR: {:.2} dB ({})",
                psnr,
                psnr_quality_description(psnr)
            );
        }
    }

    if let Some(ssim) = calculate_ssim(&orig_img, &conv_img) {
        println!("   SSIM: {:.6} ({})", ssim, ssim_quality_description(ssim));
    }

    println!("\n✅ Verification complete");

    Ok(())
}

/// Load image safely, handling JXL via external decoder if needed
fn load_image_safe(path: &PathBuf) -> anyhow::Result<image::DynamicImage> {
    let is_jxl = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase() == "jxl")
        .unwrap_or(false);

    if is_jxl {
        use std::process::Command;

        let temp_png_file = tempfile::Builder::new()
            .suffix(".png")
            .tempfile()
            .map_err(|e| anyhow::anyhow!("Failed to create temp file: {}", e))?;

        let temp_path = temp_png_file.path();

        let status = Command::new("djxl")
            .arg(shared_utils::safe_path_arg(path).as_ref())
            .arg(temp_path)
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to execute djxl: {}", e))?;

        if !status.success() {
            return Err(anyhow::anyhow!("djxl failed to decode JXL file"));
        }

        let img = image::open(temp_path)
            .map_err(|e| anyhow::anyhow!("Failed to open decoded PNG: {}", e))?;

        Ok(img)
    } else {
        Ok(image::open(path)?)
    }
}

fn print_analysis_human(analysis: &img_av1::ImageAnalysis) {
    println!("\n📊 Image Quality Analysis Report");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📁 File: {}", analysis.file_path);
    println!(
        "📷 Format: {} {}",
        analysis.format,
        if analysis.is_lossless {
            "(Lossless)"
        } else {
            "(Lossy)"
        }
    );
    println!("📐 Dimensions: {}x{}", analysis.width, analysis.height);
    println!(
        "💾 Size: {} bytes ({:.2} KB)",
        analysis.file_size,
        analysis.file_size as f64 / 1024.0
    );
    println!(
        "🎨 Bit depth: {}-bit {}",
        analysis.color_depth, analysis.color_space
    );
    if analysis.has_alpha {
        println!("🔍 Alpha channel: Yes");
    }
    if analysis.is_animated {
        println!("🎬 Animated: Yes");
    }

    println!("\n📈 Quality Analysis");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "🔒 Compression: {}",
        if analysis.is_lossless {
            "Lossless ✓"
        } else {
            "Lossy"
        }
    );
    println!(
        "📊 Entropy:   {:.2} ({})",
        analysis.features.entropy,
        if analysis.features.entropy > 7.0 {
            "High complexity"
        } else if analysis.features.entropy > 5.0 {
            "Medium complexity"
        } else {
            "Low complexity"
        }
    );
    println!(
        "📦 Compression ratio:   {:.1}%",
        analysis.features.compression_ratio * 100.0
    );

    if let Some(ref jpeg) = analysis.jpeg_analysis {
        println!("\n🎯 JPEGQuality Analysis (accuracy: ±1)");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!(
            "📊 Estimated quality: Q={} ({})",
            jpeg.estimated_quality, jpeg.quality_description
        );
        println!("🎯 Confidence:   {:.1}%", jpeg.confidence * 100.0);
        println!(
            "📋 Quantization table:   {}",
            if jpeg.is_standard_table {
                "IJG Standard ✓"
            } else {
                "Custom"
            }
        );

        if let Some(chroma_q) = jpeg.chrominance_quality {
            println!(
                "🔬 Luma quality: Q={} (SSE: {:.1})",
                jpeg.luminance_quality, jpeg.luminance_sse
            );
            if let Some(chroma_sse) = jpeg.chrominance_sse {
                println!("🔬 Chroma quality: Q={} (SSE: {:.1})", chroma_q, chroma_sse);
            }
        } else {
            println!("🔬 Luma SSE:  {:.1}", jpeg.luminance_sse);
        }

        if let Some(ref encoder) = jpeg.encoder_hint {
            println!("🏭 Encoder:   {}", encoder);
        }

        if jpeg.is_high_quality_original {
            println!("✨ Assessment: High quality original");
        }
    }

    if let Some(psnr) = analysis.psnr {
        println!("\n📐 Estimated metrics");
        println!("   PSNR: {:.2} dB", psnr);
        if let Some(ssim) = analysis.ssim {
            println!("   SSIM: {:.4}", ssim);
        }
    }
}

fn print_recommendation_human(rec: &img_av1::UpgradeRecommendation) {
    println!("\n💡 JXL Format Recommendation");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    if rec.recommended_format == rec.current_format {
        println!("ℹ️  {}", rec.reason);
    } else {
        println!("✅ {} → {}", rec.current_format, rec.recommended_format);
        println!("📝 Reason: {}", rec.reason);
        println!("🎯 Quality: {}", rec.quality_preservation);
        if rec.expected_size_reduction > 0.0 {
            println!("💾 Expected reduction: {:.1}%", rec.expected_size_reduction);
        }
        if !rec.command.is_empty() {
            println!("⚙️  Command: {}", rec.command);
        }
    }
}

/// 🔥 在"输出到相邻目录"模式下复制原始文件
/// 当文件被跳过时（短动画、无法压缩等），需要将原始文件复制到输出目录
fn copy_original_if_adjacent_mode(input: &Path, config: &AutoConvertConfig) -> anyhow::Result<()> {
    shared_utils::copy_on_skip_or_fail(
        input,
        config.output_dir.as_deref(),
        config.base_dir.as_deref(),
        config.verbose,
    )?;
    Ok(())
}

/// Smart auto-convert a single file based on format detection
fn auto_convert_single_file(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<ConversionOutput> {
    use img_av1::lossless_converter::{
        convert_jpeg_to_jxl, convert_to_av1_mp4, convert_to_av1_mp4_lossless,
        convert_to_av1_mp4_matched, convert_to_jxl, convert_to_jxl_matched, ConvertOptions,
    };

    // 🔥 v8.2.3: Fix extension BEFORE analysis/conversion
    let fixed_input = shared_utils::fix_extension_if_mismatch(input)?;
    let input = fixed_input.as_path();

    let analysis = analyze_image(input)?;

    let options = ConvertOptions {
        force: config.force,
        output_dir: config.output_dir.clone(),
        base_dir: config.base_dir.clone(),
        delete_original: config.delete_original,
        in_place: config.in_place,
        explore: config.explore,
        match_quality: config.match_quality,
        compress: config.compress,
        apple_compat: config.apple_compat,
        use_gpu: config.use_gpu,
        ultimate: config.ultimate,
        allow_size_tolerance: config.allow_size_tolerance,
        verbose: config.verbose,
        child_threads: config.child_threads,
        input_format: Some(analysis.format.clone()),
    };

    macro_rules! verbose_log {
        ($($arg:tt)*) => {
            if config.verbose {
                println!($($arg)*);
            }
        };
    }

    let make_skipped = |msg: &str| -> ConversionOutput {
        ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            message: msg.to_string(),
            original_size: analysis.file_size,
            output_size: None,
            size_reduction: None,
        }
    };

    // Smart conversion based on format and lossless status
    let result = match (
        analysis.format.as_str(),
        analysis.is_lossless,
        analysis.is_animated,
    ) {
        // Modern Formats Logic (WebP, AVIF, HEIC)
        // Rule: Avoid generational loss.
        // - If Lossy: SKIP (don't recompress lossy to lossy/jxl)
        // - If Lossless: CONVERT to JXL (better compression)
        ("WebP", true, false)
        | ("AVIF", true, false)
        | ("HEIC", true, false)
        | ("HEIF", true, false) => {
            verbose_log!("🔄 Modern Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)?
        }
        ("WebP", false, false)
        | ("AVIF", false, false)
        | ("HEIC", false, false)
        | ("HEIF", false, false) => {
            verbose_log!(
                "⏭️ Skipping modern lossy format (avoid generation loss): {}",
                input.display()
            );
            copy_original_if_adjacent_mode(input, config)?;
            return Ok(make_skipped("Skipping modern lossy format"));
        }

        // JPEG → JXL
        ("JPEG", _, false) => {
            if config.match_quality {
                verbose_log!("🔄 JPEG→JXL (MATCH QUALITY): {}", input.display());
                convert_to_jxl_matched(input, &options, &analysis)?
            } else {
                verbose_log!("🔄 JPEG→JXL lossless transcode: {}", input.display());
                convert_jpeg_to_jxl(input, &options)?
            }
        }
        // Legacy Static lossless (PNG, TIFF, BMP etc) → JXL
        (_, true, false) => {
            verbose_log!("🔄 Legacy Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)?
        }
        // Animated lossless → AV1 MP4 CRF 0 (visually lossless, only if >=3 seconds)
        (_, true, true) => {
            let duration = match analysis.duration_secs {
                Some(d) if d > 0.0 => d,
                _ => {
                    eprintln!(
                        "⚠️  Cannot get animation duration, skipping conversion: {}",
                        input.display()
                    );
                    eprintln!("   💡 Possible cause: ffprobe not installed or file format doesn't support duration detection");
                    eprintln!("   💡 Suggestion: install ffprobe: brew install ffmpeg");
                    copy_original_if_adjacent_mode(input, config)?;
                    return Ok(make_skipped("Cannot get animation duration"));
                }
            };
            if duration < 3.0 {
                verbose_log!(
                    "⏭️ Skipping short animation ({:.1}s < 3s): {}",
                    duration,
                    input.display()
                );
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(make_skipped("Skipping short animation"));
            }

            if config.lossless {
                verbose_log!(
                    "🔄 Animated lossless→AV1 MP4 (LOSSLESS, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_av1_mp4_lossless(input, &options)?
            } else {
                verbose_log!(
                    "🔄 Animated lossless→AV1 MP4 (CRF 0, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_av1_mp4(input, &options)?
            }
        }
        // Animated lossy → AV1 MP4 with match_quality (only if >=3 seconds)
        (_, false, true) => {
            let duration = match analysis.duration_secs {
                Some(d) if d > 0.0 => d,
                _ => {
                    eprintln!(
                        "⚠️  Cannot get animation duration, skipping conversion: {}",
                        input.display()
                    );
                    eprintln!("   💡 Possible cause: ffprobe not installed or file format doesn't support duration detection");
                    copy_original_if_adjacent_mode(input, config)?;
                    return Ok(make_skipped("Cannot get animation duration"));
                }
            };
            if duration < 3.0 {
                verbose_log!(
                    "⏭️ Skipping short animation ({:.1}s < 3s): {}",
                    duration,
                    input.display()
                );
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(make_skipped("Skipping short animation"));
            }

            if config.lossless {
                verbose_log!(
                    "🔄 Animated lossy→AV1 MP4 (LOSSLESS, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_av1_mp4_lossless(input, &options)?
            } else {
                verbose_log!(
                    "🔄 Animated lossy→AV1 MP4 (MATCH QUALITY, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_av1_mp4_matched(input, &options, &analysis)?
            }
        }
        // Legacy Static lossy (non-JPEG, non-Modern) → JXL
        (format, false, false) => {
            if format == "WebP" || format == "AVIF" || format == "HEIC" || format == "HEIF" {
                verbose_log!("⏭️ Skipping modern lossy format: {}", input.display());
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(make_skipped("Skipping modern lossy format"));
            }

            if config.match_quality {
                verbose_log!("🔄 Legacy Lossy→JXL (MATCH QUALITY): {}", input.display());
                convert_to_jxl_matched(input, &options, &analysis)?
            } else {
                verbose_log!("🔄 Legacy Lossy→JXL (Quality 100): {}", input.display());
                convert_to_jxl(input, &options, 0.1)?
            }
        }
    };

    // 🔥 将 ConversionResult 转换为 ConversionOutput
    let output = ConversionOutput {
        original_path: result.input_path.clone(),
        output_path: result.output_path.clone().unwrap_or(result.input_path),
        skipped: result.skipped,
        message: result.message.clone(),
        original_size: result.input_size,
        output_size: result.output_size,
        size_reduction: result.size_reduction.map(|r| r as f32),
    };

    if output.skipped {
        verbose_log!("⏭️ {}", output.message);
    } else {
        verbose_log!("✅ {}", output.message);
    }

    Ok(output)
}

/// Smart auto-convert a directory with parallel processing and progress bar
fn auto_convert_directory(input: &Path, config: &AutoConvertConfig) -> anyhow::Result<()> {
    // 🔥 Safety check: prevent accidental damage to system directories
    if config.delete_original || config.in_place {
        if let Err(e) = check_dangerous_directory(input) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

    // 🔥 v6.9.15: 克隆 config 并设置 base_dir 以保留目录结构
    let mut config_with_base = config.clone();
    if config_with_base.output_dir.is_some() && config_with_base.base_dir.is_none() {
        config_with_base.base_dir = Some(input.to_path_buf());
    }

    let thread_config = shared_utils::thread_manager::get_balanced_thread_config(
        shared_utils::thread_manager::WorkloadType::Image,
    );
    let pool_size = thread_config.parallel_tasks;
    config_with_base.child_threads = thread_config.child_threads;

    let config = &config_with_base;

    let start_time = Instant::now();

    // 🔥 v8.2.5: 必须在 collect_files 之前保存！collect_files 遍历目录会更新 atime
    let saved_dir_timestamps = shared_utils::save_directory_timestamps(input).ok();

    // 🔥 v7.5: 使用文件排序功能，优先处理小文件
    let files = shared_utils::collect_files_small_first(
        input,
        shared_utils::SUPPORTED_IMAGE_EXTENSIONS,
        config.recursive,
    );

    let total = files.len();
    if total == 0 {
        println!("📂 No image files found in {}", input.display());

        // 🔥 v7.4.9: 即使没有文件，也要保留目录元数据
        if let Some(output_dir) = config.output_dir.as_ref() {
            if let Some(ref base_dir) = config.base_dir {
                shared_utils::preserve_directory_metadata_with_log(base_dir, output_dir);
            }
        }

        return Ok(());
    }

    if config.verbose {
        println!("📂 Found {} files to process", total);
    }
    if config.lossless && config.verbose {
        println!("⚠️  Mathematical lossless mode: ENABLED (VERY SLOW!)");
    }

    // Atomic counters for thread-safe counting
    let success = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let actual_input_bytes = std::sync::atomic::AtomicU64::new(0);
    let actual_output_bytes = std::sync::atomic::AtomicU64::new(0);

    // 🔥 Progress bar with ETA
    let pb = shared_utils::UnifiedProgressBar::new(total as u64, "Converting");

    // 🔥 v7.3.2: 启用安静模式，避免并行线程的进度条互相干扰
    shared_utils::progress_mode::enable_quiet_mode();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(pool_size)
        .build()
        .unwrap_or_else(|_| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .expect("Failed to create fallback thread pool")
        });

    if config.verbose {
        println!(
            "🔧 Thread Strategy: {} parallel tasks x {} threads/task (CPU cores: {})",
            pool_size,
            thread_config.child_threads,
            num_cpus::get()
        );
    }

    // Process files in parallel using custom thread pool
    pool.install(|| {
        files.par_iter().for_each(|path| {
            match auto_convert_single_file(path, config) {
                Ok(result) => {
                    if result.skipped {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        success.fetch_add(1, Ordering::Relaxed);
                        actual_input_bytes.fetch_add(result.original_size, Ordering::Relaxed);
                        if let Some(out_size) = result.output_size {
                            actual_output_bytes.fetch_add(out_size, Ordering::Relaxed);
                        }
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("Skipped") || msg.contains("skip") {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        eprintln!("❌ Conversion failed {}: {}", path.display(), e);
                        failed.fetch_add(1, Ordering::Relaxed);

                        if let Some(ref output_dir) = config.output_dir {
                            let _ = shared_utils::copy_on_skip_or_fail(
                                path,
                                Some(output_dir),
                                config.base_dir.as_deref(),
                                config.verbose,
                            );
                        }
                    }
                }
            }
            let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
            pb.set_position(current as u64);
            pb.set_message(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            );
        });
    });

    pb.finish_with_message("Complete!");

    // 🔥 v7.3.2: 恢复正常模式
    shared_utils::progress_mode::disable_quiet_mode();

    let success_count = success.load(Ordering::Relaxed);
    let skipped_count = skipped.load(Ordering::Relaxed);
    let failed_count = failed.load(Ordering::Relaxed);

    let mut result = BatchResult::new();
    result.succeeded = success_count;
    result.failed = failed_count;
    result.skipped = skipped_count;
    result.total = total;

    let final_input_bytes = actual_input_bytes.load(Ordering::Relaxed);
    let final_output_bytes = actual_output_bytes.load(Ordering::Relaxed);

    print_summary_report(
        &result,
        start_time.elapsed(),
        final_input_bytes,
        final_output_bytes,
        "Image Conversion",
    );

    // 🔥 v7.4.9: 保留目录元数据（权限、xattr）
    if let Some(ref output_dir) = config.output_dir {
        if let Some(ref base_dir) = config.base_dir {
            shared_utils::preserve_directory_metadata_with_log(base_dir, output_dir);
        }
    }

    // 🔥 v8.2.5: 用处理前保存的时间戳恢复
    if let Some(ref saved) = saved_dir_timestamps {
        if let Some(ref output_dir) = config.output_dir {
            if let Some(ref base_dir) = config.base_dir {
                shared_utils::apply_saved_timestamps_to_dst(saved, base_dir, output_dir);
            }
        }
        shared_utils::restore_directory_timestamps(saved);
        println!("✅ Directory timestamps restored");
    }

    Ok(())
}
