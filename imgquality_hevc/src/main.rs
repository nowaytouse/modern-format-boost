use clap::{Parser, Subcommand, ValueEnum};
use imgquality_hevc::lossless_converter::{convert_to_gif_apple_compat, is_high_quality_animated};
use imgquality_hevc::{analyze_image, get_recommendation};
use imgquality_hevc::{
    calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description,
};
use rayon::prelude::*;
use serde_json::json;
use shared_utils::{check_dangerous_directory, print_summary_report, BatchResult};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use walkdir::WalkDir;

/// 检查动态图片是否为高质量（用于决定转 HEVC 还是 GIF）
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
    /// Analyze image quality parameters
    Analyze {
        /// Input file or directory
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Recursive directory scan
        #[arg(short, long)]
        recursive: bool,

        /// Output format
        #[arg(short, long, value_enum, default_value = "human")]
        output: OutputFormat,

        /// Include upgrade recommendation
        #[arg(short = 'r', long)]
        recommend: bool,
    },

    /// Auto-convert based on format detection (JPEG→JXL, PNG→JXL, Animated→HEVC MP4)
    ///
    /// 🔥 动态图片/视频转换默认使用智能质量匹配：
    /// - 二分搜索找到最优 CRF
    /// - SSIM 裁判验证确保质量 (≥0.95)
    /// - 输出大于输入时自动跳过
    Auto {
        /// Input file or directory
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Output directory (default: same as input)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Force conversion even if already processed
        #[arg(short, long)]
        force: bool,

        /// Recursive directory scan
        #[arg(short, long)]
        recursive: bool,

        /// Delete original after successful conversion
        #[arg(long)]
        delete_original: bool,

        /// In-place conversion: convert and delete original file
        /// Effectively "replaces" the original with the new format
        /// Example: image.png → image.jxl (original .png deleted)
        #[arg(long)]
        in_place: bool,

        /// Use mathematical lossless AVIF/HEVC (⚠️ VERY SLOW, huge files)
        /// Disables smart quality matching for video
        #[arg(long)]
        lossless: bool,

        /// Explore smaller file sizes for animated→video conversion ONLY.
        /// Alone: Binary search for smaller output (no quality validation).
        /// With --match-quality: Precise quality match (binary search + SSIM validation).
        /// Does NOT affect static images (JPEG/PNG always use lossless conversion).
        #[arg(long)]
        explore: bool,

        /// Match input quality level for animated→video conversion ONLY.
        /// Alone: Single encode with AI-predicted CRF + SSIM validation.
        /// With --explore: Precise quality match (binary search + SSIM validation).
        /// Does NOT affect static images (JPEG/PNG always use lossless conversion).
        #[arg(long)]
        match_quality: bool,

        /// 🔥 Require compression for animated→video conversion ONLY.
        /// Alone: Just ensure output < input (even 1KB smaller counts).
        /// With --match-quality: output < input + SSIM validation.
        /// With --explore --match-quality: Precise quality match + must compress.
        /// Does NOT affect static images (JPEG/PNG always use lossless conversion).
        #[arg(long)]
        compress: bool,

        /// 🍎 Apple compatibility mode: Convert non-Apple-compatible animated formats to HEVC
        /// When enabled, animated WebP (VP8/VP9) will be converted to HEVC MP4
        /// instead of being skipped as "modern format"
        #[arg(long, default_value_t = false)]
        apple_compat: bool,

        /// Uses adaptive wall limit based on CRF range, continues until no more quality gains
        /// ⚠️ MUST be used with --explore --match-quality --compress
        #[arg(long, default_value_t = false)]
        ultimate: bool,

        /// Verbose output (show skipped files and success messages)
        #[arg(short, long)]
        verbose: bool,
    },

    /// Verify conversion quality
    Verify {
        /// Original file
        original: PathBuf,

        /// Converted file
        converted: PathBuf,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable output
    Human,
    /// JSON output (for API use)
    Json,
}

/// 计算目录中指定扩展名文件的总大小
#[allow(dead_code)]
fn calculate_directory_size_by_extensions(
    dir: &PathBuf,
    extensions: &[&str],
    recursive: bool,
) -> u64 {
    let walker = if recursive {
        WalkDir::new(dir).follow_links(true)
    } else {
        WalkDir::new(dir).max_depth(1)
    };

    walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            if let Some(ext) = e.path().extension() {
                extensions.contains(&ext.to_str().unwrap_or("").to_lowercase().as_str())
            } else {
                false
            }
        })
        .filter_map(|e| std::fs::metadata(e.path()).ok())
        .map(|m| m.len())
        .sum()
}

fn main() -> anyhow::Result<()> {
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

        Commands::Auto {
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
            ultimate,
            verbose,
        } => {
            // in_place implies delete_original
            let should_delete = delete_original || in_place;

            // 🔥 v6.2: 使用模块化的 flag 验证器（含 ultimate 支持）
            if let Err(e) = shared_utils::validate_flags_result_with_ultimate(
                explore,
                match_quality,
                compress,
                ultimate,
            ) {
                eprintln!("{}", e);
                std::process::exit(1);
            }

            if lossless {
                eprintln!("⚠️  Mathematical lossless mode: ENABLED (VERY SLOW!)");
                eprintln!("   Smart quality matching: DISABLED");
            } else if verbose {
                // 显示探索模式信息
                let flag_mode = shared_utils::validate_flags_result_with_ultimate(
                    explore,
                    match_quality,
                    compress,
                    ultimate,
                )
                .unwrap();
                eprintln!("🎬 {} (for animated→video)", flag_mode.description_cn());
                eprintln!("📷 Static images: Always lossless (JPEG→JXL, PNG→JXL)");
            }
            if apple_compat {
                eprintln!("🍎 Apple Compatibility: ENABLED (animated WebP → HEVC)");
            }
            if in_place {
                eprintln!(
                    "🔄 In-place mode: ENABLED (original files will be deleted after conversion)"
                );
            }
            if ultimate {
                eprintln!("🔥 Ultimate Explore: ENABLED (search until SSIM saturates)");
            }
            let config = AutoConvertConfig {
                output_dir: output.clone(),
                base_dir: None, // 🔥 v6.9.15: Will be set in auto_convert_directory
                force,
                delete_original: should_delete,
                in_place,
                lossless,
                explore,
                match_quality,
                compress,
                apple_compat,
                use_gpu: true, // 🔥 v6.2: Always use GPU for coarse search
                ultimate,      // 🔥 v6.2: 极限探索模式
                verbose,
            };

            if input.is_file() {
                auto_convert_single_file(&input, &config)?;
            } else if input.is_dir() {
                auto_convert_directory(&input, &config, recursive)?;
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
    let image_extensions = [
        "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif",
    ];

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
            if image_extensions.contains(&ext.to_str().unwrap_or("").to_lowercase().as_str()) {
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
    // Check extension
    let is_jxl = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase() == "jxl")
        .unwrap_or(false);

    if is_jxl {
        use std::process::Command;
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let temp_path = std::env::temp_dir().join(format!("imgquality_verify_{}.png", timestamp));

        // Decode JXL to PNG using djxl
        let status = Command::new("djxl")
            .arg(path)
            .arg(&temp_path)
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to execute djxl: {}", e))?;

        if !status.success() {
            return Err(anyhow::anyhow!("djxl failed to decode JXL file"));
        }

        // Load the temp PNG
        let img = image::open(&temp_path).map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            anyhow::anyhow!("Failed to open decoded PNG: {}", e)
        })?;

        // Cleanup
        let _ = std::fs::remove_file(&temp_path);

        Ok(img)
    } else {
        Ok(image::open(path)?)
    }
}

fn print_analysis_human(analysis: &imgquality_hevc::ImageAnalysis) {
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

    // Quality analysis section
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

    // JPEG specific analysis with enhanced details
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

        // Show both luma and chroma quality if available
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

        // Show encoder hint if detected
        if let Some(ref encoder) = jpeg.encoder_hint {
            println!("🏭 Encoder:   {}", encoder);
        }

        if jpeg.is_high_quality_original {
            println!("✨ Assessment: High quality original");
        }
    }

    // Legacy PSNR/SSIM
    if let Some(psnr) = analysis.psnr {
        println!("\n📐 Estimated metrics");
        println!("   PSNR: {:.2} dB", psnr);
        if let Some(ssim) = analysis.ssim {
            println!("   SSIM: {:.4}", ssim);
        }
    }
}

fn print_recommendation_human(rec: &imgquality_hevc::UpgradeRecommendation) {
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

/// Auto-convert configuration
#[derive(Clone)] // 🔥 v6.9.15: 需要 Clone 以设置 base_dir
struct AutoConvertConfig {
    output_dir: Option<PathBuf>,
    /// 🔥 v6.9.15: Base directory for preserving relative paths
    base_dir: Option<PathBuf>,
    force: bool,
    delete_original: bool,
    in_place: bool,
    lossless: bool,
    explore: bool,
    match_quality: bool,
    /// 🔥 v4.6: 压缩模式
    compress: bool,
    /// 🍎 Apple compatibility mode
    apple_compat: bool,
    /// 🔥 v4.15: Use GPU acceleration (default: true)
    use_gpu: bool,
    /// 🔥 v6.2: 极限探索模式
    ultimate: bool,
    /// Verbose output
    verbose: bool,
}

/// 🔥 v6.5.2: 在"输出到相邻目录"模式下复制原始文件
/// 当文件被跳过时（短动画、无法压缩等），需要将原始文件复制到输出目录
/// 🔥 v6.9.11: 同时合并XMP边车文件（如果存在）
/// 🔥 v7.4.2: 使用 smart_file_copier 模块
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
///
/// 🔥 动态图片/视频转换默认使用智能质量匹配（非 lossless 模式时）：
/// - 二分搜索找到最优 CRF
/// - SSIM 裁判验证确保质量 (≥0.95)
/// - 输出大于输入时自动跳过
fn auto_convert_single_file(input: &Path, config: &AutoConvertConfig) -> anyhow::Result<()> {
    use imgquality_hevc::lossless_converter::{
        convert_jpeg_to_jxl, convert_to_hevc_mkv_lossless, convert_to_hevc_mp4_matched,
        convert_to_jxl, ConvertOptions,
    };

    let analysis = analyze_image(input)?;

    let options = ConvertOptions {
        force: config.force,
        output_dir: config.output_dir.clone(),
        base_dir: config.base_dir.clone(), // 🔥 v6.9.15: 保留目录结构
        delete_original: config.delete_original,
        in_place: config.in_place,
        explore: config.explore,
        match_quality: config.match_quality,
        compress: config.compress,
        apple_compat: config.apple_compat,
        use_gpu: config.use_gpu,
        ultimate: config.ultimate, // 🔥 v6.2: 极限探索模式
        verbose: config.verbose,
    };

    // Helper macro for verbose logging
    macro_rules! verbose_log {
        ($($arg:tt)*) => {
            if config.verbose {
                println!($($arg)*);
            }
        };
    }

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
        // 🍎 Apple compat mode: animated WebP (VP8/VP9) will be converted to HEVC
        ("WebP", true, false)
        | ("AVIF", true, false)
        | ("HEIC", true, false)
        | ("HEIF", true, false) => {
            verbose_log!("🔄 Modern Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)? // Mathematical lossless
        }
        // 🍎 Apple compat mode: Skip static lossy modern formats, but animated will be handled below
        ("WebP", false, false)
        | ("AVIF", false, false)
        | ("HEIC", false, false)
        | ("HEIF", false, false) => {
            verbose_log!(
                "⏭️ Skipping modern lossy format (avoid generation loss): {}",
                input.display()
            );
            // 🔥 v6.5.2: 相邻目录模式下，复制原始文件到输出目录
            copy_original_if_adjacent_mode(input, config)?;
            return Ok(());
        }

        // JPEG → JXL (always lossless transcode, match_quality does NOT apply to static images)
        ("JPEG", _, false) => {
            // 🔥 JPEG 始终使用无损转码（保留 DCT 系数，零质量损失）
            // match_quality 仅用于动图转视频，不影响静态图片
            verbose_log!("🔄 JPEG→JXL lossless transcode: {}", input.display());
            convert_jpeg_to_jxl(input, &options)?
        }
        // Legacy Static lossless (PNG, TIFF, BMP etc) → JXL
        (_, true, false) => {
            verbose_log!("🔄 Legacy Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)?
        }
        // Animated → HEVC MP4 or GIF (based on duration and quality)
        // 🔥 默认使用智能质量匹配：二分搜索 + SSIM 裁判验证
        // 🍎 Apple compat mode:
        //   - 把现代动态格式（WebP/AVIF）转换为 Apple 兼容格式
        //   - 长动画(>=3s) 或 高质量 → HEVC MP4
        //   - 短动画(<3s) 且 非高质量 → GIF (Bayer 256色)
        // 🔥 v5.75: GIF 和其他动态图片一样处理！
        //   - duration >= 3s → 转换为 HEVC 视频
        //   - duration < 3s → 跳过（太短不值得转换）
        //   - GIF 不需要特殊 flag，默认就会转换（只要满足时长条件）
        (format, is_lossless, true) => {
            // 🍎 Check if this is a modern animated format (NOT including GIF!)
            // GIF 本身就是 Apple 兼容格式，不属于"现代格式"
            let is_modern_animated = matches!(format, "WebP" | "AVIF" | "HEIC" | "HEIF" | "JXL");
            if is_modern_animated && !is_lossless && !config.apple_compat {
                verbose_log!(
                    "⏭️ Skipping modern lossy animated format (avoid generation loss): {}",
                    input.display()
                );
                verbose_log!(
                    "   💡 Use --apple-compat to convert to HEVC for Apple device compatibility"
                );
                // 🔥 v6.5.2: 相邻目录模式下，复制原始文件到输出目录
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(());
            }

            // 获取时长
            let duration = match analysis.duration_secs {
                Some(d) if d > 0.0 => d,
                _ => {
                    eprintln!(
                        "⚠️  Cannot get animation duration, skipping conversion: {}",
                        input.display()
                    );
                    eprintln!("   💡 Possible cause: ffprobe not installed or file format doesn't support duration detection");
                    // 🔥 v6.5.2: 相邻目录模式下，复制原始文件到输出目录
                    copy_original_if_adjacent_mode(input, config)?;
                    return Ok(());
                }
            };

            // 获取尺寸判断是否高质量
            let is_high_quality = if let Ok((w, h)) = shared_utils::probe_video(input)
                .map(|p| (p.width, p.height))
                .or_else(|_| image::image_dimensions(input).map_err(|_| ()))
            {
                convert_to_gif_apple_compat_check_quality(w, h)
            } else {
                false // 无法获取尺寸时假设非高质量
            };

            // 🍎 Apple 兼容模式下的现代动态图片处理策略
            if config.apple_compat && is_modern_animated {
                if duration >= 3.0 || is_high_quality {
                    // 长动画或高质量 → HEVC MP4
                    verbose_log!(
                        "🍎 Animated {}→HEVC MP4 (Apple Compat, {:.1}s, {}): {}",
                        format,
                        duration,
                        if is_high_quality {
                            "高质量"
                        } else {
                            "长动画"
                        },
                        input.display()
                    );
                    convert_to_hevc_mp4_matched(input, &options, &analysis)?
                } else {
                    // 短动画且非高质量 → GIF (Bayer 256色)
                    verbose_log!(
                        "🍎 Animated {}→GIF (Apple Compat, {:.1}s, Bayer 256 colors): {}",
                        format,
                        duration,
                        input.display()
                    );
                    convert_to_gif_apple_compat(input, &options, None)?
                }
            } else if duration < 3.0 {
                // 非 Apple 兼容模式下，短动画跳过
                verbose_log!(
                    "⏭️ Skipping short animation ({:.1}s < 3s): {}",
                    duration,
                    input.display()
                );
                // 🔥 v6.5.2: 相邻目录模式下，复制原始文件到输出目录
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(());
            } else if config.lossless {
                // 用户显式要求数学无损
                verbose_log!(
                    "🔄 Animated→HEVC MKV (LOSSLESS, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_hevc_mkv_lossless(input, &options)?
            } else {
                // 🔥 默认：智能质量匹配（二分搜索 + SSIM 验证）
                verbose_log!(
                    "🔄 Animated→HEVC MP4 (SMART QUALITY, {:.1}s): {}",
                    duration,
                    input.display()
                );
                convert_to_hevc_mp4_matched(input, &options, &analysis)?
            }
        }
        // Legacy Static lossy (non-JPEG, non-Modern) → JXL
        // This handles cases like BMP (if not detected as lossless somehow) or other obscure formats
        // 🔥 match_quality 仅用于动图转视频，不影响静态图片
        (format, false, false) => {
            // Redundant safecheck for WebP/AVIF/HEIC just in case pattern matching missed
            if format == "WebP" || format == "AVIF" || format == "HEIC" || format == "HEIF" {
                verbose_log!("⏭️ Skipping modern lossy format: {}", input.display());
                // 🔥 v6.5.2: 相邻目录模式下，复制原始文件到输出目录
                copy_original_if_adjacent_mode(input, config)?;
                return Ok(());
            }

            // 🔥 静态有损图片使用高质量转换（distance 0.1 ≈ Q100）
            // match_quality 仅用于动图转视频
            verbose_log!("🔄 Legacy Lossy→JXL (Quality 100): {}", input.display());
            convert_to_jxl(input, &options, 0.1)?
        }
    };

    if result.skipped {
        verbose_log!("⏭️ {}", result.message);
    } else {
        // 🔥 修复：message 已经包含了正确的 size reduction/increase 信息
        verbose_log!("✅ {}", result.message);
    }

    Ok(())
}

/// Smart auto-convert a directory with parallel processing and progress bar
///
/// 🔥 动态图片/视频转换默认使用智能质量匹配（非 lossless 模式时）
fn auto_convert_directory(
    input: &Path,
    config: &AutoConvertConfig,
    recursive: bool,
) -> anyhow::Result<()> {
    // 🔥 Safety check: prevent accidental damage to system directories
    if config.delete_original || config.in_place {
        if let Err(e) = check_dangerous_directory(input) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

    // 🔥 v6.9.15: 克隆 config 并设置 base_dir 以保留目录结构
    let mut config_with_base = config.clone();
    if config_with_base.output_dir.is_some() {
        config_with_base.base_dir = Some(input.to_path_buf());
    }
    let config = &config_with_base;

    let start_time = Instant::now();
    let image_extensions = [
        "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif", "avif",
    ];

    // 🔥 v7.5: 使用文件排序功能，优先处理小文件
    // - 快速看到进度反馈
    // - 小文件处理快，可以更早发现问题
    // - 大文件留到后面，避免长时间卡住
    let files = shared_utils::collect_files_small_first(&input, &image_extensions, recursive);

    let total = files.len();
    if total == 0 {
        println!("📂 No image files found in {}", input.display());

        // 🔥 v7.4.9: 即使没有文件，也要保留目录元数据
        if let Some(output_dir) = config.output_dir.as_ref() {
            if let Some(ref base_dir) = config.base_dir {
                println!("\n📁 Preserving directory metadata...");
                if let Err(e) = shared_utils::preserve_directory_metadata(base_dir, output_dir) {
                    eprintln!("⚠️ Failed to preserve directory metadata: {}", e);
                } else {
                    println!("✅ Directory metadata preserved");
                }
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
    // 🔥 修复：追踪实际转换的输入/输出大小
    let actual_input_bytes = std::sync::atomic::AtomicU64::new(0);
    let actual_output_bytes = std::sync::atomic::AtomicU64::new(0);

    // 🔥 Progress bar with ETA
    let pb = shared_utils::create_progress_bar(total as u64, "Converting");

    // 🔥 v7.3.2: 启用安静模式，避免并行线程的进度条互相干扰
    shared_utils::progress_mode::enable_quiet_mode();

    // 🔥 性能优化：限制并发数，避免系统卡顿
    // - 使用 CPU 核心数的一半，留出资源给系统和编码器内部线程
    // - 最少 1 个，最多 4 个并发任务
    let num_cpus = num_cpus::get();
    let max_threads = (num_cpus / 2).clamp(1, 4);

    // 创建自定义线程池
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(max_threads)
        .build()
        .unwrap_or_else(|_| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .unwrap()
        });

    if config.verbose {
        println!(
            "🔧 Using {} parallel threads (CPU cores: {})",
            max_threads, num_cpus
        );
    }

    // Process files in parallel using custom thread pool
    pool.install(|| {
        files.par_iter().for_each(|path| {
            // 获取输入文件大小
            let input_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

            match auto_convert_single_file(path, config) {
                Ok(_) => {
                    // 🔥 修复：检查是否真的生成了输出文件
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    let parent_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                    let out_dir = config.output_dir.as_ref().unwrap_or(&parent_dir);

                    // 检查可能的输出文件
                    // 🔥 v6.4.8: 添加 MOV 格式（苹果兼容模式）
                    let possible_outputs = [
                        out_dir.join(format!("{}.jxl", stem)),
                        out_dir.join(format!("{}.mp4", stem)),
                        out_dir.join(format!("{}.mov", stem)),
                        out_dir.join(format!("{}.mkv", stem)),
                    ];

                    let output_size: u64 = possible_outputs
                        .iter()
                        .filter_map(|p| std::fs::metadata(p).ok())
                        .map(|m| m.len())
                        .next()
                        .unwrap_or(0);

                    if output_size > 0 {
                        // 真正成功的转换
                        success.fetch_add(1, Ordering::Relaxed);
                        actual_input_bytes.fetch_add(input_size, Ordering::Relaxed);
                        actual_output_bytes.fetch_add(output_size, Ordering::Relaxed);
                    } else {
                        // 跳过的文件（没有生成输出）- 已在 auto_convert_single_file 中复制
                        skipped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("Skipped") || msg.contains("skip") {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        eprintln!("❌ Conversion failed {}: {}", path.display(), e);
                        failed.fetch_add(1, Ordering::Relaxed);

                        // 🔥 v7.4.4: 使用 smart_file_copier 保留目录结构
                        if let Some(ref output_dir) = config.output_dir {
                            let _ = shared_utils::copy_on_skip_or_fail(
                                path,
                                Some(output_dir),
                                config.base_dir.as_deref(),
                                false,
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

    // Build result for summary report
    let mut result = BatchResult::new();
    result.succeeded = success_count;
    result.failed = failed_count;
    result.skipped = skipped_count;
    result.total = total;

    // 🔥 修复：使用实际追踪的输入/输出大小
    let final_input_bytes = actual_input_bytes.load(Ordering::Relaxed);
    let final_output_bytes = actual_output_bytes.load(Ordering::Relaxed);

    // 🔥 Print detailed summary report
    print_summary_report(
        &result,
        start_time.elapsed(),
        final_input_bytes,
        final_output_bytes,
        "Image Conversion",
    );

    // 🔥 v6.9.13: 无遗漏设计 - 复制不支持的文件
    if let Some(ref output_dir) = config.output_dir {
        println!("\n📦 Copying unsupported files...");
        let copy_result = shared_utils::copy_unsupported_files(input, output_dir, recursive);
        if copy_result.copied > 0 {
            println!("📦 Copied {} unsupported files", copy_result.copied);
        }
        if copy_result.failed > 0 {
            eprintln!("❌ Failed to copy {} files", copy_result.failed);
        }

        // 🔥 验证输出完整性
        println!("\n🔍 Verifying output completeness...");
        let verify = shared_utils::verify_output_completeness(input, output_dir, recursive);
        println!("{}", verify.message);
        if !verify.passed {
            eprintln!("⚠️  Some files may be missing from output!");
        }

        // 🔥 v7.4.9: 保留目录元数据（时间戳、权限、xattr）
        if let Some(ref base_dir) = config.base_dir {
            println!("\n📁 Preserving directory metadata...");
            if let Err(e) = shared_utils::preserve_directory_metadata(base_dir, output_dir) {
                eprintln!("⚠️ Failed to preserve directory metadata: {}", e);
            } else {
                println!("✅ Directory metadata preserved");
            }
        }
    }

    Ok(())
}
