use clap::{Parser, Subcommand, ValueEnum};
use imgquality_av1::{analyze_image, get_recommendation};
use imgquality_av1::{calculate_psnr, calculate_ssim, psnr_quality_description, ssim_quality_description};
use rayon::prelude::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use walkdir::WalkDir;
use shared_utils::{check_dangerous_directory, print_summary_report, BatchResult};

/// Configuration for auto-convert operations
struct AutoConvertConfig<'a> {
    output_dir: Option<&'a Path>,
    force: bool,
    recursive: bool,
    delete_original: bool,
    in_place: bool,
    lossless: bool,
    explore: bool,
    match_quality: bool,
    compress: bool,
    /// 🔥 v4.15: Use GPU acceleration (default: true)
    use_gpu: bool,
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

    /// Auto-convert based on format detection (JPEG→JXL, PNG→JXL, Animated→AV1 MP4)
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

        /// Use mathematical lossless AVIF/AV1 (⚠️ VERY SLOW, huge files)
        #[arg(long)]
        lossless: bool,

        /// Explore smaller file sizes for animated→video conversion ONLY.
        /// Alone: Binary search for smaller output (no quality validation).
        /// With --match-quality: Precise quality match (binary search + SSIM validation).
        #[arg(long)]
        explore: bool,

        /// Match input quality level for animated→video conversion (auto-calculate CRF)
        #[arg(long)]
        match_quality: bool,

        /// 🔥 Require compression for animated→video conversion ONLY.
        /// Alone: Just ensure output < input (even 1KB smaller counts).
        /// With --match-quality: output < input + SSIM validation.
        #[arg(long)]
        compress: bool,

        /// 🔥 v4.15: Force CPU encoding (libaom) instead of GPU
        /// Hardware encoding may have lower quality ceiling. Use --cpu for maximum SSIM
        #[arg(long, default_value_t = false)]
        cpu: bool,
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
fn calculate_directory_size_by_extensions(dir: &PathBuf, extensions: &[&str], recursive: bool) -> u64 {
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
            cpu,
        } => {
            // in_place implies delete_original
            let should_delete = delete_original || in_place;
            
            // 🔥 v4.6: 使用模块化的 flag 验证器
            if let Err(e) = shared_utils::validate_flags_result(explore, match_quality, compress) {
                eprintln!("{}", e);
                std::process::exit(1);
            }
            
            if lossless {
                eprintln!("⚠️  Mathematical lossless mode: ENABLED (VERY SLOW!)");
            } else {
                let flag_mode = shared_utils::validate_flags_result(explore, match_quality, compress).unwrap();
                eprintln!("🎬 {} (for animated→video)", flag_mode.description_cn());
            }
            if in_place {
                eprintln!("🔄 In-place mode: ENABLED (original files will be deleted after conversion)");
            }
            if cpu {
                eprintln!("🖥️  CPU Encoding: ENABLED (libaom for maximum SSIM)");
            }
            let config = AutoConvertConfig {
                output_dir: output.as_deref(),
                force,
                recursive,
                delete_original: should_delete,
                in_place,
                lossless,
                explore,
                match_quality,
                compress,
                use_gpu: !cpu,  // 🔥 v4.15: CPU mode = no GPU
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

        Commands::Verify { original, converted } => {
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
    let image_extensions = ["png", "jpg", "jpeg", "webp", "gif", "tiff", "tif"];
    
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
        println!("{}", json!({
            "total": count,
            "results": results
        }));
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
    println!("   Original size:  {} bytes ({:.2} KB)", 
        original_analysis.file_size, original_analysis.file_size as f64 / 1024.0);
    println!("   Converted size: {} bytes ({:.2} KB)", 
        converted_analysis.file_size, converted_analysis.file_size as f64 / 1024.0);
    
    let reduction = 100.0 * (1.0 - converted_analysis.file_size as f64 / original_analysis.file_size as f64);
    println!("   Size reduction: {:.2}%", reduction);

    // Load images for quality comparison
    let orig_img = load_image_safe(original)?;
    let conv_img = load_image_safe(converted)?;
    
    println!("\n📏 Quality Metrics:");
    if let Some(psnr) = calculate_psnr(&orig_img, &conv_img) {
        if psnr.is_infinite() {
            println!("   PSNR: ∞ dB (Identical - mathematically lossless)");
        } else {
            println!("   PSNR: {:.2} dB ({})", psnr, psnr_quality_description(psnr));
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
    let is_jxl = path.extension()
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

fn print_analysis_human(analysis: &imgquality_av1::ImageAnalysis) {
    println!("\n📊 Image Quality Analysis Report");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📁 File: {}", analysis.file_path);
    println!("📷 Format: {} {}", analysis.format, 
        if analysis.is_lossless { "(Lossless)" } else { "(Lossy)" });
    println!("📐 Dimensions: {}x{}", analysis.width, analysis.height);
    println!("💾 Size: {} bytes ({:.2} KB)", 
        analysis.file_size, 
        analysis.file_size as f64 / 1024.0);
    println!("🎨 Bit depth: {}-bit {}", analysis.color_depth, analysis.color_space);
    if analysis.has_alpha {
        println!("🔍 Alpha channel: Yes");
    }
    if analysis.is_animated {
        println!("🎬 Animated: Yes");
    }
    
    // Quality analysis section
    println!("\n📈 Quality Analysis");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("🔒 Compression: {}", if analysis.is_lossless { "Lossless ✓" } else { "Lossy" });
    println!("📊 Entropy:   {:.2} ({})", 
        analysis.features.entropy,
        if analysis.features.entropy > 7.0 { "High complexity" } 
        else if analysis.features.entropy > 5.0 { "Medium complexity" } 
        else { "Low complexity" });
    println!("📦 Compression ratio:   {:.1}%", analysis.features.compression_ratio * 100.0);
    
    // JPEG specific analysis with enhanced details
    if let Some(ref jpeg) = analysis.jpeg_analysis {
        println!("\n🎯 JPEGQuality Analysis (accuracy: ±1)");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("📊 Estimated quality: Q={} ({})", jpeg.estimated_quality, jpeg.quality_description);
        println!("🎯 Confidence:   {:.1}%", jpeg.confidence * 100.0);
        println!("📋 Quantization table:   {}", 
            if jpeg.is_standard_table { "IJG Standard ✓" } else { "Custom" });
        
        // Show both luma and chroma quality if available
        if let Some(chroma_q) = jpeg.chrominance_quality {
            println!("🔬 Luma quality: Q={} (SSE: {:.1})", jpeg.luminance_quality, jpeg.luminance_sse);
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

fn print_recommendation_human(rec: &imgquality_av1::UpgradeRecommendation) {
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

/// Smart auto-convert a single file based on format detection
fn auto_convert_single_file(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<()> {
    use imgquality_av1::lossless_converter::{
        convert_to_jxl, convert_jpeg_to_jxl,
        convert_to_av1_mp4, convert_to_av1_mp4_lossless,
        convert_to_av1_mp4_matched, convert_to_jxl_matched,
        ConvertOptions,
    };
    
    let analysis = analyze_image(input)?;
    
    let options = ConvertOptions {
        force: config.force,
        output_dir: config.output_dir.map(|p| p.to_path_buf()),
        delete_original: config.delete_original,
        in_place: config.in_place,
        explore: config.explore,
        match_quality: config.match_quality,
        compress: config.compress,
        apple_compat: false,  // imgquality_av1 不需要 Apple 兼容模式
        use_gpu: config.use_gpu,  // 🔥 v4.15: Pass GPU control
    };
    
    // Smart conversion based on format and lossless status
    let result = match (analysis.format.as_str(), analysis.is_lossless, analysis.is_animated) {
        // Modern Formats Logic (WebP, AVIF, HEIC)
        // Rule: Avoid generational loss. 
        // - If Lossy: SKIP (don't recompress lossy to lossy/jxl)
        // - If Lossless: CONVERT to JXL (better compression)
        ("WebP", true, false) | ("AVIF", true, false) | ("HEIC", true, false) | ("HEIF", true, false) => {
            println!("🔄 Modern Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)? // Mathematical lossless
        }
        ("WebP", false, _) | ("AVIF", false, _) | ("HEIC", false, _) | ("HEIF", false, _) => {
            println!("⏭️ Skipping modern lossy format (avoid generation loss): {}", input.display());
            return Ok(());
        }

        // JPEG → JXL
        ("JPEG", _, false) => {
            if config.match_quality {
                // Match quality mode: use lossy JXL with matched distance for better compression
                println!("🔄 JPEG→JXL (MATCH QUALITY): {}", input.display());
                convert_to_jxl_matched(input, &options, &analysis)?
            } else {
                // Default: lossless transcode (preserves DCT coefficients, no quality loss)
                println!("🔄 JPEG→JXL lossless transcode: {}", input.display());
                convert_jpeg_to_jxl(input, &options)?
            }
        }
        // Legacy Static lossless (PNG, TIFF, BMP etc) → JXL
        (_, true, false) => {
            println!("🔄 Legacy Lossless→JXL: {}", input.display());
            convert_to_jxl(input, &options, 0.0)?
        }
        // Animated lossless → AV1 MP4 CRF 0 (visually lossless, only if >=3 seconds)
        // 🔥 无损源保持高质量：默认 CRF 0，用户可选 --lossless 数学无损
        (_, true, true) => {
            // Check duration - only convert animations >=3 seconds
            // 🔥 质量宣言：时长未知时使用保守策略（跳过），并响亮警告
            let duration = match analysis.duration_secs {
                Some(d) if d > 0.0 => d,
                _ => {
                    eprintln!("⚠️  Cannot get animation duration, skipping conversion: {}", input.display());
                    eprintln!("   💡 Possible cause: ffprobe not installed or file format doesn't support duration detection");
                    return Ok(());
                }
            };
            if duration < 3.0 {
                println!("⏭️ Skipping short animation ({:.1}s < 3s): {}", duration, input.display());
                return Ok(());
            }
            
            if config.lossless {
                // 用户显式要求数学无损
                println!("🔄 Animated lossless→AV1 MP4 (LOSSLESS, {:.1}s): {}", duration, input.display());
                convert_to_av1_mp4_lossless(input, &options)?
            } else {
                // 🔥 无损源默认使用 CRF 0（视觉无损），不使用 match_quality
                // match_quality 仅用于有损源
                println!("🔄 Animated lossless→AV1 MP4 (CRF 0, {:.1}s): {}", duration, input.display());
                convert_to_av1_mp4(input, &options)?
            }
        }
        // Animated lossy → AV1 MP4 with match_quality (only if >=3 seconds)
        // 🔥 有损源使用 match_quality 以获得更好的空间效率
        (_, false, true) => {
            // 🔥 质量宣言：时长未知时使用保守策略（跳过），并响亮警告
            let duration = match analysis.duration_secs {
                Some(d) if d > 0.0 => d,
                _ => {
                    eprintln!("⚠️  Cannot get animation duration, skipping conversion: {}", input.display());
                    eprintln!("   💡 Possible cause: ffprobe not installed or file format doesn't support duration detection");
                    return Ok(());
                }
            };
            if duration < 3.0 {
                println!("⏭️ Skipping short animation ({:.1}s < 3s): {}", duration, input.display());
                return Ok(());
            }
            
            if config.lossless {
                // 用户显式要求数学无损
                println!("🔄 Animated lossy→AV1 MP4 (LOSSLESS, {:.1}s): {}", duration, input.display());
                convert_to_av1_mp4_lossless(input, &options)?
            } else {
                // 🔥 有损源默认使用 match_quality
                println!("🔄 Animated lossy→AV1 MP4 (MATCH QUALITY, {:.1}s): {}", duration, input.display());
                convert_to_av1_mp4_matched(input, &options, &analysis)?
            }
        }
        // Legacy Static lossy (non-JPEG, non-Modern) → JXL
        // This handles cases like BMP (if not detected as lossless somehow) or other obscure formats
        (format, false, false) => {
             // Redundant safecheck for WebP/AVIF/HEIC just in case pattern matching missed
            if format == "WebP" || format == "AVIF" || format == "HEIC" || format == "HEIF" {
                println!("⏭️ Skipping modern lossy format: {}", input.display());
                return Ok(());
            }
            
            if config.match_quality {
                println!("🔄 Legacy Lossy→JXL (MATCH QUALITY): {}", input.display());
                convert_to_jxl_matched(input, &options, &analysis)?
            } else {
                println!("🔄 Legacy Lossy→JXL (Quality 100): {}", input.display());
                convert_to_jxl(input, &options, 0.1)?
            }
        }
    };
    
    if result.skipped {
        println!("⏭️ {}", result.message);
    } else {
        // 🔥 修复：message 已经包含了正确的 size reduction/increase 信息
        println!("✅ {}", result.message);
    }
    
    Ok(())
}

/// Smart auto-convert a directory with parallel processing and progress bar
fn auto_convert_directory(
    input: &Path,
    config: &AutoConvertConfig,
) -> anyhow::Result<()> {
    // 🔥 Safety check: prevent accidental damage to system directories
    if config.delete_original || config.in_place {
        if let Err(e) = check_dangerous_directory(input) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
    
    let start_time = Instant::now();
    let image_extensions = ["png", "jpg", "jpeg", "webp", "gif", "tiff", "tif", "heic", "avif"];
    
    let walker = if config.recursive {
        WalkDir::new(input).follow_links(true)
    } else {
        WalkDir::new(input).max_depth(1)
    };

    // Collect all file paths first for parallel processing
    let files: Vec<PathBuf> = walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            if let Some(ext) = e.path().extension() {
                image_extensions.contains(&ext.to_str().unwrap_or("").to_lowercase().as_str())
            } else {
                false
            }
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    let total = files.len();
    if total == 0 {
        println!("📂 No image files found in {}", input.display());
        return Ok(());
    }
    
    println!("📂 Found {} files to process", total);
    if config.lossless {
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

    // 🔥 性能优化：限制并发数，避免系统卡顿
    // - 使用 CPU 核心数的一半，留出资源给系统和编码器内部线程
    // - 最少 1 个，最多 4 个并发任务
    let num_cpus = num_cpus::get();
    let max_threads = (num_cpus / 2).clamp(1, 4);
    
    // 创建自定义线程池
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(max_threads)
        .build()
        .unwrap_or_else(|_| rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
    
    println!("🔧 Using {} parallel threads (CPU cores: {})", max_threads, num_cpus);
    
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
                    let out_dir = config.output_dir.unwrap_or(&parent_dir);
                    
                    // 检查可能的输出文件
                    let possible_outputs = [
                        out_dir.join(format!("{}.jxl", stem)),
                        out_dir.join(format!("{}.mp4", stem)),
                    ];
                    
                    let output_size: u64 = possible_outputs.iter()
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
                        // 跳过的文件（没有生成输出）
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
                    }
                }
            }
            let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
            pb.set_position(current as u64);
            pb.set_message(path.file_name().unwrap_or_default().to_string_lossy().to_string());
        });
    });

    pb.finish_with_message("Complete!");

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
    print_summary_report(&result, start_time.elapsed(), final_input_bytes, final_output_bytes, "Image Conversion");

    Ok(())
}

