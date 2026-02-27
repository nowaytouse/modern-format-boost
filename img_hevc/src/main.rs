use clap::{Parser, Subcommand, ValueEnum};
use img_hevc::lossless_converter::{convert_to_gif_apple_compat, is_high_quality_animated};
use img_hevc::{analyze_image, get_recommendation};
use img_hevc::{
    calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description,
};
use rayon::prelude::*;
use serde_json::json;
use shared_utils::{check_dangerous_directory, print_summary_report, BatchResult};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use walkdir::WalkDir;

fn convert_to_gif_apple_compat_check_quality(width: u32, height: u32) -> bool {
    is_high_quality_animated(width, height)
}

#[derive(Parser)]
#[command(name = "imgquality")]
#[command(version, about = "Image quality analyzer and format upgrade tool", long_about = None)]
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

        #[arg(short, long, value_enum, default_value = "human")]
        output: OutputFormat,

        #[arg(short = 'R', long)]
        recommend: bool,
    },

    #[command(name = "run")]
    Run {
        #[arg(short, long)]
        output: Option<PathBuf>,

        #[arg(long)]
        base_dir: Option<PathBuf>,

        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(short, long)]
        force: bool,

        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        #[arg(long)]
        delete_original: bool,

        #[arg(long)]
        in_place: bool,

        #[arg(long)]
        lossless: bool,

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

        #[arg(long, default_value_t = true)]
        allow_size_tolerance: bool,

        #[arg(long)]
        no_allow_size_tolerance: bool,

        #[arg(short, long)]
        verbose: bool,

        /// Write full verbose log to this file (regardless of --verbose flag).
        #[arg(long, value_name = "PATH")]
        log_file: Option<PathBuf>,

        /// Resume from last run: skip files already in progress file (default).
        #[arg(long, default_value_t = true)]
        resume: bool,

        /// Start fresh: ignore previous progress file, process all files.
        #[arg(long)]
        no_resume: bool,
    },

    Verify {
        original: PathBuf,

        converted: PathBuf,
    },

    RestoreTimestamps {
        #[arg(value_name = "SOURCE_DIR")]
        source: PathBuf,

        #[arg(value_name = "OUTPUT_DIR")]
        output: PathBuf,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Human,
    Json,
}

fn main() -> anyhow::Result<()> {
    let _ = shared_utils::logging::init_logging(
        "img_hevc",
        shared_utils::logging::LogConfig::default(),
    );

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
            allow_size_tolerance,
            no_allow_size_tolerance,
            verbose,
            base_dir,
            log_file,
            resume: resume_flag,
            no_resume,
        } => {
            let resume = resume_flag && !no_resume;
            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let should_delete = delete_original || in_place;

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
                eprintln!("🎬 {} (for animated→video)", flag_mode.description_en());
                eprintln!("📷 Static images: Always lossless (JPEG→JXL, PNG→JXL)");
            }
            shared_utils::progress_mode::set_verbose_mode(verbose);
            if let Some(ref lf) = log_file {
                if let Err(e) = shared_utils::progress_mode::set_log_file(lf) {
                    eprintln!("⚠️  Could not open log file {}: {}", lf.display(), e);
                }
            }
            // Run 时若未指定 --log-file，自动写入当前目录的 img_hevc_run.log（质量/进度始终有据可查）
            if let Err(e) = shared_utils::progress_mode::set_default_run_log_file("img_hevc") {
                eprintln!("⚠️  Could not open default log file: {}", e);
            }
            if apple_compat {
                eprintln!("🍎 Apple Compatibility: ENABLED (animated WebP → HEVC)");
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
            let config = AutoConvertConfig {
                output_dir: output.clone(),
                base_dir: base_dir.clone(),
                force,
                delete_original: should_delete,
                in_place,
                lossless,
                explore,
                match_quality,
                compress,
                apple_compat,
                use_gpu: true,
                ultimate,
                allow_size_tolerance,
                verbose,
                child_threads: 0,
            };

            let workload = if input.is_dir() {
                shared_utils::thread_manager::WorkloadType::Image
            } else {
                shared_utils::thread_manager::WorkloadType::Video
            };
            let thread_config = shared_utils::thread_manager::get_balanced_thread_config(workload);
            let mut config = config;
            config.child_threads = thread_config.child_threads;

            if input.is_file() {
                auto_convert_single_file(&input, &config)?;
            } else if input.is_dir() {
                let progress_path = output.as_ref().unwrap_or(&input).join(".mfb_processed");
                if resume {
                    if let Err(e) = shared_utils::load_processed_list(&progress_path) {
                        if config.verbose {
                            eprintln!("⚠️  Could not load progress file: {}", e);
                        }
                    } else if config.verbose && progress_path.exists() {
                        println!("📂 Resume: loading progress from {}", progress_path.display());
                    }
                } else {
                    shared_utils::clear_processed_list();
                    let _ = std::fs::remove_file(&progress_path);
                    if config.verbose {
                        println!("📂 Fresh run: previous progress cleared");
                    }
                }
                auto_convert_directory(&input, &config, recursive)?;
                if let Err(e) = shared_utils::save_processed_list(&progress_path) {
                    if config.verbose {
                        eprintln!("⚠️  Could not save progress file: {}", e);
                    }
                }
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

fn print_analysis_human(analysis: &img_hevc::ImageAnalysis) {
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

fn print_recommendation_human(rec: &img_hevc::UpgradeRecommendation) {
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

#[derive(Clone)]
struct AutoConvertConfig {
    output_dir: Option<PathBuf>,
    base_dir: Option<PathBuf>,
    force: bool,
    delete_original: bool,
    in_place: bool,
    lossless: bool,
    explore: bool,
    match_quality: bool,
    compress: bool,
    apple_compat: bool,
    use_gpu: bool,
    ultimate: bool,
    allow_size_tolerance: bool,
    verbose: bool,
    child_threads: usize,
}

fn copy_original_if_adjacent_mode(input: &Path, config: &AutoConvertConfig) -> anyhow::Result<()> {
    shared_utils::copy_on_skip_or_fail(
        input,
        config.output_dir.as_deref(),
        config.base_dir.as_deref(),
        config.verbose,
    )?;
    Ok(())
}

use img_hevc::conversion_api::ConversionOutput;

fn convert_result_to_output(result: shared_utils::ConversionResult) -> ConversionOutput {
    let input_path = result.input_path.clone();
    ConversionOutput {
        original_path: result.input_path,
        output_path: result.output_path.unwrap_or(input_path),
        skipped: result.skipped,
        message: result.message,
        original_size: result.input_size,
        output_size: result.output_size,
        size_reduction: result.size_reduction.map(|r| r as f32),
    }
}

fn auto_convert_single_file(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<ConversionOutput> {
    use img_hevc::lossless_converter::{
        convert_jpeg_to_jxl, convert_to_hevc_mkv_lossless, convert_to_hevc_mp4_matched,
        convert_to_jxl, ConvertOptions,
    };

    let fixed_input = shared_utils::fix_extension_if_mismatch(input)?;
    let input = fixed_input.as_path();

    let _label = input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    shared_utils::progress_mode::set_log_context(&_label);
    let _log_guard = shared_utils::progress_mode::LogContextGuard;

    // Apple compat: HEIC/HEIF are already native — skip without running heavy analysis (avoids SecurityLimitExceeded etc.)
    if config.apple_compat && shared_utils::image_heic_analysis::is_heic_file(input) {
        let file_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        copy_original_if_adjacent_mode(input, config)?;
        return Ok(ConversionOutput {
            original_path: input.display().to_string(),
            output_path: input.display().to_string(),
            skipped: true,
            message: "HEIC/HEIF is Apple native, skipping".to_string(),
            original_size: file_size,
            output_size: None,
            size_reduction: None,
        });
    }

    let analysis = analyze_image(input)?;

    // Single source of truth for static skip: JXL + modern lossy (avoid generational loss).
    if !analysis.is_animated {
        let skip = shared_utils::should_skip_image_format(analysis.format.as_str(), analysis.is_lossless);
        if skip.should_skip {
            if config.verbose {
                println!("⏭️ {}: {}", skip.reason, input.display());
            }
            copy_original_if_adjacent_mode(input, config)?;
            return Ok(ConversionOutput {
                original_path: input.display().to_string(),
                output_path: input.display().to_string(),
                skipped: true,
                message: skip.reason,
                original_size: analysis.file_size,
                output_size: None,
                size_reduction: None,
            });
        }
    }

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
        child_threads: if config.child_threads > 0 {
            config.child_threads
        } else {
            2
        },
        input_format: Some(analysis.format.clone()),
    };

    // 完整接入图像质量分析：静态图始终做像素级分析，用于路由 + 质量输出（自动写入 run log）
    let pixel_analysis = if !analysis.is_animated {
        shared_utils::analyze_image_quality_from_path(input)
    } else {
        None
    };
    if let Some(ref q) = pixel_analysis {
        shared_utils::log_media_info_for_image_quality(q, input);
    }
    // 路由：像素级建议跳过则跳过（与 format 级互补）
    #[allow(deprecated)]
    if let Some(ref q) = pixel_analysis {
        let rd = &q.routing_decision;
        if rd.should_skip {
            let msg = rd
                .skip_reason
                .clone()
                .unwrap_or_else(|| "Pixel-based: skip".to_string());
            if config.verbose {
                println!("⏭️ {}: {}", msg, input.display());
            }
            copy_original_if_adjacent_mode(input, config)?;
            return Ok(ConversionOutput {
                original_path: input.display().to_string(),
                output_path: input.display().to_string(),
                skipped: true,
                message: msg,
                original_size: analysis.file_size,
                output_size: None,
                size_reduction: None,
            });
        }
    }

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

    // Dispatch order: (1) format filter already applied above (HEIC/HEIF Apple skip, JXL skip).
    // (2) Then by (format, is_lossless, is_animated): modern static → JXL or skip; JPEG → JXL; legacy lossless → JXL; animated → HEVC/GIF/skip; legacy lossy → JXL.
    let result = match (
        analysis.format.as_str(),
        analysis.is_lossless,
        analysis.is_animated,
    ) {
        ("WebP", true, false)
        | ("AVIF", true, false)
        | ("HEIC", true, false)
        | ("HEIF", true, false) => {
            verbose_log!("🔄 Modern Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)?
        }
        // Static modern lossy / JXL already handled by should_skip_image_format above.

        ("JPEG", _, false) => {
            verbose_log!("🔄 JPEG→JXL lossless transcode: {}", input.display());
            convert_jpeg_to_jxl(input, &options)?
        }
        (_, true, false) => {
            verbose_log!("🔄 Legacy Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)?
        }
        (format, is_lossless, true) => {
            let is_modern_animated = matches!(format, "WebP" | "AVIF" | "HEIC" | "HEIF" | "JXL");
            let is_apple_native = matches!(format, "HEIC" | "HEIF");

            let should_skip_modern = if is_modern_animated && !is_lossless {
                if config.apple_compat {
                    is_apple_native
                } else {
                    true
                }
            } else {
                false
            };

            if should_skip_modern {
                verbose_log!(
                    "⏭️ Skipping modern lossy animated format (avoid generation loss): {}",
                    input.display()
                );
                if is_apple_native && config.apple_compat {
                    verbose_log!("   💡 Reason: {} is already a native Apple format", format);
                } else {
                    verbose_log!(
                        "   💡 Use --apple-compat to convert to HEVC for Apple device compatibility"
                    );
                }
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(make_skipped("Skipping modern lossy animated format"));
            }

            let duration = match analysis.duration_secs {
                Some(d) if d > 0.0 => d,
                Some(0.0) => {
                    verbose_log!(
                        "⏭️ Detected static GIF (1 frame), treating as static image: {}",
                        input.display()
                    );
                    verbose_log!("🔄 Static GIF→JXL: {}", input.display());
                    let conv_result = convert_to_jxl(input, &options, 0.0)?;
                    return Ok(convert_result_to_output(conv_result));
                }
                _ => {
                    let retry = shared_utils::image_analyzer::get_animation_duration_for_path(input);
                    if let Some(d) = retry {
                        d
                    } else {
                        eprintln!(
                            "⚠️  Cannot get animation duration, skipping conversion: {}",
                            input.display()
                        );
                        eprintln!("   💡 Possible cause: ffprobe not installed or file format doesn't support duration detection");
                        eprintln!("   💡 Suggestion: install ffprobe: brew install ffmpeg");
                        copy_original_if_adjacent_mode(input, config)?;
                        return Ok(make_skipped("Cannot get animation duration"));
                    }
                }
            };

            let is_high_quality = if let Ok((w, h)) = shared_utils::probe_video(input)
                .map(|p| (p.width, p.height))
                .or_else(|_| image::image_dimensions(input).map_err(|_| ()))
            {
                convert_to_gif_apple_compat_check_quality(w, h)
            } else {
                false
            };

            const MIN_DURATION: f32 = shared_utils::image_analyzer::ANIMATED_MIN_DURATION_FOR_VIDEO_SECS;
            if config.apple_compat && is_modern_animated && !is_apple_native {
                if duration >= MIN_DURATION || is_high_quality {
                    verbose_log!(
                        "🍎 Animated {}→HEVC MP4 (Apple Compat, {:.1}s, {}): {}",
                        format,
                        duration,
                        if is_high_quality {
                            "High Quality"
                        } else {
                            "Long Animation"
                        },
                        input.display()
                    );
                    convert_to_hevc_mp4_matched(input, &options, &analysis)?
                } else {
                    verbose_log!(
                        "🍎 Animated {}→GIF (Apple Compat, {:.1}s < {:.1}s, Bayer 256 colors): {}",
                        format,
                        duration,
                        MIN_DURATION,
                        input.display()
                    );
                    convert_to_gif_apple_compat(input, &options, None)?
                }
            } else if duration < MIN_DURATION {
                verbose_log!(
                    "⏭️ Skipping short animation ({:.1}s < {:.1}s): {}",
                    duration,
                    MIN_DURATION,
                    input.display()
                );
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(make_skipped("Skipping short animation"));
            } else if config.lossless {
                verbose_log!(
                    "🔄 Animated→HEVC MKV (LOSSLESS, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_hevc_mkv_lossless(input, &options)?
            } else {
                verbose_log!(
                    "🔄 Animated→HEVC MP4 (SMART QUALITY, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_hevc_mp4_matched(input, &options, &analysis)?
            }
        }
        (_, false, false) => {
            // Modern lossy static already skipped above; only legacy lossy reach here.
            // 路由：像素级建议无损则用 0.0，否则 0.1
            #[allow(deprecated)]
            let jxl_distance = match &pixel_analysis {
                Some(q) => {
                    let rd = &q.routing_decision;
                    if rd.use_lossless {
                        0.0
                    } else {
                        0.1
                    }
                }
                None => 0.1,
            };
            verbose_log!(
                "🔄 Legacy Lossy→JXL ({}): {}",
                if jxl_distance == 0.0 {
                    "Lossless"
                } else {
                    "Quality 100"
                },
                input.display()
            );
            convert_to_jxl(input, &options, jxl_distance)?
        }
    };

    let output = convert_result_to_output(result);

    if output.skipped {
        verbose_log!("⏭️ {}", output.message);
    } else {
        verbose_log!("✅ {}", output.message);
    }

    Ok(output)
}

fn auto_convert_directory(
    input: &Path,
    config: &AutoConvertConfig,
    recursive: bool,
) -> anyhow::Result<()> {
    if config.delete_original || config.in_place {
        if let Err(e) = check_dangerous_directory(input) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

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

    let saved_dir_timestamps = shared_utils::save_directory_timestamps(input).ok();

    let files = shared_utils::collect_files_small_first(
        input,
        shared_utils::IMAGE_EXTENSIONS_FOR_CONVERT,
        recursive,
    );

    let total = files.len();
    if total == 0 {
        println!("📂 No image files found in {}", input.display());

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

    let success = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let actual_input_bytes = std::sync::atomic::AtomicU64::new(0);
    let actual_output_bytes = std::sync::atomic::AtomicU64::new(0);

    let pb = shared_utils::UnifiedProgressBar::new(total as u64, "Converting");

    shared_utils::progress_mode::enable_quiet_mode();

    let max_threads = pool_size;
    let child_threads = thread_config.child_threads;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(max_threads)
        .build()
        .or_else(|_| rayon::ThreadPoolBuilder::new().num_threads(2).build())
        .map_err(|e| anyhow::anyhow!("Failed to create thread pool: {}", e))?;

    if config.verbose {
        println!(
            "🔧 Thread Strategy: {} parallel tasks x {} threads/task (CPU cores: {})",
            max_threads,
            child_threads,
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        );
        if let Some(hint) = shared_utils::thread_manager::memory_cap_hint() {
            println!("   💡 {}", hint);
        }
    }

    pool.install(|| {
        files.par_iter().for_each(|path| {
            match auto_convert_single_file(path, config) {
                Ok(result) => {
                    if result.skipped {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        success.fetch_add(1, Ordering::Relaxed);
                        shared_utils::progress_mode::image_processed_success();
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
                        shared_utils::progress_mode::image_processed_failure();

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

    shared_utils::progress_mode::disable_quiet_mode();
    shared_utils::progress_mode::xmp_merge_finalize();
    shared_utils::progress_mode::flush_log_file();

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

    if let Some(ref output_dir) = config.output_dir {
        if let Some(ref base_dir) = config.base_dir {
            shared_utils::preserve_directory_metadata_with_log(base_dir, output_dir);
        }
    }

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
