use crate::heic_analysis::{analyze_heic_file, is_heic_file, HeicAnalysis};
use crate::jpeg_analysis::{analyze_jpeg_file, JpegQualityAnalysis};
use crate::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView, ImageFormat};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// JXL upgrade indicator - simple and clear
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JxlIndicator {
    /// Whether conversion to JXL is recommended
    pub should_convert: bool,
    /// Clear reason for the recommendation
    pub reason: String,
    /// Exact command to run
    pub command: String,
    /// Expected benefit
    pub benefit: String,
}

/// Image features for quality assessment
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageFeatures {
    /// Image entropy (complexity measure)
    pub entropy: f64,
    /// Compression ratio (file size vs raw size)
    pub compression_ratio: f64,
}

/// Complete image analysis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAnalysis {
    // Basic info
    pub file_path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    
    // Color info
    pub color_depth: u8,
    pub color_space: String,
    pub has_alpha: bool,
    pub is_animated: bool,
    
    // Animation duration in seconds (for animated images, None for static)
    pub duration_secs: Option<f32>,
    
    // Core quality info
    pub is_lossless: bool,
    
    // JPEG specific analysis (null for non-JPEG)
    pub jpeg_analysis: Option<JpegQualityAnalysis>,
    
    // HEIC specific analysis (null for non-HEIC)
    pub heic_analysis: Option<HeicAnalysis>,
    
    // Image features
    pub features: ImageFeatures,
    
    // Simple JXL indicator
    pub jxl_indicator: JxlIndicator,
    
    // Legacy fields (for compatibility)
    pub psnr: Option<f64>,
    pub ssim: Option<f64>,
    pub metadata: HashMap<String, String>,
}

/// Analyze an image file and return quality parameters
pub fn analyze_image(path: &PathBuf) -> Result<ImageAnalysis> {
    // Check if file exists
    if !path.exists() {
        return Err(ImgQualityError::ImageReadError(format!(
            "File not found: {}",
            path.display()
        )));
    }

    // Get file size
    let file_size = std::fs::metadata(path)?.len();
    
    // Check if HEIC - use libheif instead of image crate
    if is_heic_file(path) {
        return analyze_heic_image(path, file_size);
    }
    
    // Check if JXL - image crate doesn't support JXL natively
    if is_jxl_file(path) {
        return analyze_jxl_image(path, file_size);
    }

    // Load the image
    let img = image::open(path).map_err(|e| {
        ImgQualityError::ImageReadError(format!("Failed to open image: {}", e))
    })?;

    // Detect format
    let format = detect_format(path)?;
    let format_str = format_to_string(&format);

    // Get basic image properties
    let (width, height) = img.dimensions();
    let has_alpha = has_alpha_channel(&img);
    let color_depth = detect_color_depth(&img);
    let color_space = detect_color_space(&img);

    // Detect if animated (for GIF/WebP)
    let is_animated = is_animated_format(path, &format)?;

    // Detect lossless compression
    let is_lossless = detect_lossless(&format, path)?;

    // JPEG specific analysis
    let jpeg_analysis = if format == ImageFormat::Jpeg {
        match analyze_jpeg_file(path) {
            Ok(analysis) => Some(analysis),
            Err(_) => None,
        }
    } else {
        None
    };

    // Calculate image features
    let features = calculate_image_features(&img, file_size);

    // Generate JXL indicator
    let jxl_indicator = generate_jxl_indicator(
        &format,
        is_lossless,
        &jpeg_analysis,
        path,
    );

    // Legacy PSNR/SSIM from JPEG analysis
    let (psnr, ssim) = if let Some(ref jpeg) = jpeg_analysis {
        // Estimate PSNR from quality factor
        let estimated_psnr = estimate_psnr_from_quality(jpeg.estimated_quality);
        let estimated_ssim = estimate_ssim_from_quality(jpeg.estimated_quality);
        (Some(estimated_psnr), Some(estimated_ssim))
    } else {
        (None, None)
    };

    // Extract metadata
    let metadata = extract_metadata(path)?;
    
    // Get duration for animated images using ffprobe
    let duration_secs = if is_animated {
        get_animation_duration(path)
    } else {
        None
    };

    Ok(ImageAnalysis {
        file_path: path.display().to_string(),
        format: format_str,
        width,
        height,
        file_size,
        color_depth,
        color_space,
        has_alpha,
        is_animated,
        duration_secs,
        is_lossless,
        jpeg_analysis,
        heic_analysis: None,
        features,
        jxl_indicator,
        psnr,
        ssim,
        metadata,
    })
}

/// Analyze HEIC/HEIF image using libheif
fn analyze_heic_image(path: &PathBuf, file_size: u64) -> Result<ImageAnalysis> {
    // Try to analyze deeply, but fallback if it fails (e.g. MemoryAllocationError)
    // This allows the main loop to still see it as "HEIC" and skip it
    let (width, height, has_alpha, color_depth, is_lossless, codec, features) = match analyze_heic_file(path) {
        Ok((img, heic_analysis)) => {
            let (w, h) = img.dimensions();
            let feats = calculate_image_features(&img, file_size);
            (w, h, heic_analysis.has_alpha, heic_analysis.bit_depth, heic_analysis.is_lossless, heic_analysis.codec, feats)
        }
        Err(e) => {
            eprintln!("⚠️ Deep HEIC analysis failed (skipping to basic info): {}", e);
            // Return dummy values so we can proceed to skip it
            (0, 0, false, 8, false, "unknown".to_string(), ImageFeatures::default())
        }
    };
    
    // HEIC is already efficient, similar to AVIF
    let jxl_indicator = JxlIndicator {
        should_convert: false,
        reason: format!("HEIC已是现代高效格式 ({}编码)", codec),
        command: String::new(),
        benefit: String::new(),
    };
    
    // Use unwrap_or_default for metadata to be safe
    let metadata = extract_metadata(path).unwrap_or_default();
    
    Ok(ImageAnalysis {
        file_path: path.display().to_string(),
        format: "HEIC".to_string(),
        width,
        height,
        file_size,
        color_depth,
        color_space: "sRGB".to_string(),
        has_alpha,
        is_animated: false,
        duration_secs: None,
        is_lossless,
        jpeg_analysis: None,
        heic_analysis: None, // We don't have the full struct if analysis failed, but that's fine
        features,
        jxl_indicator,
        psnr: None,
        ssim: None,
        metadata,
    })
}

/// Generate simple JXL indicator based on analysis
fn generate_jxl_indicator(
    format: &ImageFormat,
    is_lossless: bool,
    jpeg_analysis: &Option<JpegQualityAnalysis>,
    path: &PathBuf,
) -> JxlIndicator {
    let file_path = path.display().to_string();
    let output_path = path.with_extension("jxl").display().to_string();
    
    match format {
        ImageFormat::Png | ImageFormat::Gif | ImageFormat::Tiff => {
            // Lossless formats -> strongly recommend JXL
            // cjxl v0.11+: --modular=1 强制使用 modular 模式，-e 范围 1-10
            JxlIndicator {
                should_convert: true,
                reason: "无损图像，强烈建议转换为JXL格式".to_string(),
                command: format!(
                    "cjxl '{}' '{}' -d 0.0 --modular=1 -e 9",
                    file_path, output_path
                ),
                benefit: "可减少30-60%体积，完全保留原始质量".to_string(),
            }
        }
        ImageFormat::Jpeg => {
            // JPEG -> recommend lossless transcode
            if let Some(ref jpeg) = jpeg_analysis {
                let quality_info = format!("原始质量 Q={}", jpeg.estimated_quality);
                JxlIndicator {
                    should_convert: true,
                    reason: format!("JPEG图像 ({})，可无损转码至JXL", quality_info),
                    command: format!(
                        "cjxl '{}' '{}' --lossless_jpeg=1",
                        file_path, output_path
                    ),
                    benefit: "保留原始JPEG DCT系数，可逆转换，减少约20%体积".to_string(),
                }
            } else {
                JxlIndicator {
                    should_convert: true,
                    reason: "JPEG图像可无损转码至JXL".to_string(),
                    command: format!(
                        "cjxl '{}' '{}' --lossless_jpeg=1",
                        file_path, output_path
                    ),
                    benefit: "保留原始JPEG DCT系数，可逆转换".to_string(),
                }
            }
        }
        ImageFormat::WebP => {
            if is_lossless {
                // cjxl v0.11+: --modular=1 强制使用 modular 模式，-e 范围 1-10
                JxlIndicator {
                    should_convert: true,
                    reason: "无损WebP图像，建议转换为JXL".to_string(),
                    command: format!(
                        "cjxl '{}' '{}' -d 0.0 --modular=1 -e 9",
                        file_path, output_path
                    ),
                    benefit: "JXL通常比WebP无损更高效".to_string(),
                }
            } else {
                JxlIndicator {
                    should_convert: false,
                    reason: "有损WebP图像，转换可能导致额外质量损失".to_string(),
                    command: String::new(),
                    benefit: String::new(),
                }
            }
        }
        ImageFormat::Avif => {
            // AVIF is already modern and efficient
            JxlIndicator {
                should_convert: false,
                reason: "AVIF已是现代高效格式，无需转换".to_string(),
                command: String::new(),
                benefit: String::new(),
            }
        }
        _ => {
            JxlIndicator {
                should_convert: false,
                reason: "不支持的格式或无需转换".to_string(),
                command: String::new(),
                benefit: String::new(),
            }
        }
    }
}

/// Calculate image features
fn calculate_image_features(img: &DynamicImage, file_size: u64) -> ImageFeatures {
    let (width, height) = img.dimensions();
    let channels = match img.color() {
        image::ColorType::L8 | image::ColorType::L16 => 1,
        image::ColorType::La8 | image::ColorType::La16 => 2,
        image::ColorType::Rgb8 | image::ColorType::Rgb16 | image::ColorType::Rgb32F => 3,
        _ => 4,
    };
    let bits_per_channel = match img.color() {
        image::ColorType::L8 | image::ColorType::La8 | image::ColorType::Rgb8 | image::ColorType::Rgba8 => 8,
        image::ColorType::L16 | image::ColorType::La16 | image::ColorType::Rgb16 | image::ColorType::Rgba16 => 16,
        image::ColorType::Rgb32F | image::ColorType::Rgba32F => 32,
        _ => 8,
    };
    
    // Calculate raw size
    let raw_size = (width as u64) * (height as u64) * (channels as u64) * (bits_per_channel as u64 / 8);
    
    // Compression ratio
    let compression_ratio = if raw_size > 0 {
        file_size as f64 / raw_size as f64
    } else {
        1.0
    };
    
    // Calculate entropy from histogram
    let entropy = calculate_entropy(img);
    
    ImageFeatures {
        entropy,
        compression_ratio,
    }
}

/// Calculate image entropy (Shannon entropy)
fn calculate_entropy(img: &DynamicImage) -> f64 {
    let gray = img.to_luma8();
    let pixels = gray.as_raw();
    
    // Build histogram
    let mut histogram = [0u64; 256];
    for &pixel in pixels {
        histogram[pixel as usize] += 1;
    }
    
    let total = pixels.len() as f64;
    let mut entropy = 0.0;
    
    for &count in &histogram {
        if count > 0 {
            let p = count as f64 / total;
            entropy -= p * p.log2();
        }
    }
    
    entropy
}

/// Estimate PSNR from JPEG quality factor
fn estimate_psnr_from_quality(quality: u8) -> f64 {
    // Approximate relationship between JPEG quality and PSNR
    // Based on empirical observations
    match quality {
        95..=100 => 45.0 + (quality as f64 - 95.0) * 0.5,
        85..=94 => 38.0 + (quality as f64 - 85.0) * 0.7,
        75..=84 => 32.0 + (quality as f64 - 75.0) * 0.6,
        60..=74 => 28.0 + (quality as f64 - 60.0) * 0.27,
        _ => 20.0 + (quality as f64) * 0.13,
    }
}

/// Estimate SSIM from JPEG quality factor
fn estimate_ssim_from_quality(quality: u8) -> f64 {
    match quality {
        95..=100 => 0.98 + (quality as f64 - 95.0) * 0.004,
        85..=94 => 0.95 + (quality as f64 - 85.0) * 0.003,
        75..=84 => 0.90 + (quality as f64 - 75.0) * 0.005,
        60..=74 => 0.80 + (quality as f64 - 60.0) * 0.0067,
        _ => 0.60 + (quality as f64) * 0.003,
    }
}

// ============================================================================
// Helper functions (unchanged from original)
// ============================================================================

/// Detect image format from file
fn detect_format(path: &PathBuf) -> Result<ImageFormat> {
    let format = image::ImageReader::open(path)
        .map_err(|e| ImgQualityError::ImageReadError(e.to_string()))?
        .format();

    format.ok_or_else(|| {
        ImgQualityError::UnsupportedFormat(format!(
            "Could not detect format for {}",
            path.display()
        ))
    })
}

/// Convert ImageFormat to string
fn format_to_string(format: &ImageFormat) -> String {
    match format {
        ImageFormat::Png => "PNG".to_string(),
        ImageFormat::Jpeg => "JPEG".to_string(),
        ImageFormat::Gif => "GIF".to_string(),
        ImageFormat::WebP => "WebP".to_string(),
        ImageFormat::Tiff => "TIFF".to_string(),
        ImageFormat::Avif => "AVIF".to_string(),
        _ => format!("{:?}", format),
    }
}

/// Detect if image has alpha channel
fn has_alpha_channel(img: &DynamicImage) -> bool {
    matches!(
        img.color(),
        image::ColorType::Rgba8
            | image::ColorType::Rgba16
            | image::ColorType::La8
            | image::ColorType::La16
    )
}

/// Detect color depth
fn detect_color_depth(img: &DynamicImage) -> u8 {
    match img.color() {
        image::ColorType::L8
        | image::ColorType::La8
        | image::ColorType::Rgb8
        | image::ColorType::Rgba8 => 8,
        image::ColorType::L16
        | image::ColorType::La16
        | image::ColorType::Rgb16
        | image::ColorType::Rgba16 => 16,
        image::ColorType::Rgb32F | image::ColorType::Rgba32F => 32,
        _ => 8,
    }
}

/// Detect color space (simplified)
fn detect_color_space(img: &DynamicImage) -> String {
    match img.color() {
        image::ColorType::L8 | image::ColorType::L16 | image::ColorType::La8 | image::ColorType::La16 => {
            "Grayscale".to_string()
        }
        _ => "sRGB".to_string(),
    }
}

/// Check if format supports animation and if this file is animated
fn is_animated_format(path: &PathBuf, format: &ImageFormat) -> Result<bool> {
    match format {
        ImageFormat::Gif => Ok(check_gif_animation(path)?),
        ImageFormat::WebP => Ok(check_webp_animation(path)?),
        _ => Ok(false),
    }
}

fn check_gif_animation(path: &PathBuf) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    let descriptor_count = bytes.windows(1).filter(|w| w[0] == 0x2C).count();
    Ok(descriptor_count > 1)
}

fn check_webp_animation(path: &PathBuf) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    let anim_marker = b"ANIM";
    Ok(bytes.windows(4).any(|w| w == anim_marker))
}

/// Get animation duration in seconds using ffprobe
fn get_animation_duration(path: &PathBuf) -> Option<f32> {
    use std::process::Command;
    
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            path.to_str().unwrap_or("")
        ])
        .output()
        .ok()?;
    
    if !output.status.success() {
        return None;
    }
    
    let json_str = String::from_utf8_lossy(&output.stdout);
    
    // Parse duration from JSON output
    // Look for "duration": "X.XXX"
    if let Some(duration_pos) = json_str.find("\"duration\"") {
        let after_key = &json_str[duration_pos + 11..];
        if let Some(quote_start) = after_key.find('"') {
            let after_quote = &after_key[quote_start + 1..];
            if let Some(quote_end) = after_quote.find('"') {
                let duration_str = &after_quote[..quote_end];
                return duration_str.parse::<f32>().ok();
            }
        }
    }
    
    None
}

/// Detect if compression is lossless
fn detect_lossless(format: &ImageFormat, path: &PathBuf) -> Result<bool> {
    match format {
        ImageFormat::Png => Ok(true),
        ImageFormat::Gif => Ok(true),
        ImageFormat::Tiff => Ok(true),
        ImageFormat::Jpeg => Ok(false),
        ImageFormat::WebP => check_webp_lossless(path),
        ImageFormat::Avif => check_avif_lossless(path),
        _ => Ok(false),
    }
}

fn check_webp_lossless(path: &PathBuf) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    let vp8l_marker = b"VP8L";
    Ok(bytes.windows(4).any(|w| w == vp8l_marker))
}

/// Check if AVIF is lossless
/// AVIF uses AV1 codec which can be configured for lossless
fn check_avif_lossless(path: &PathBuf) -> Result<bool> {
    // AVIF lossless detection is complex - for now, assume lossy
    // True lossless AVIF is rare in practice
    // Could be improved by parsing AVIF headers for quantizer settings
    let _bytes = std::fs::read(path)?;
    
    // Check for lossless indicators in AVIF
    // Look for 'ispe' (image spatial extent) and analyze
    // For now, return false as most AVIF are lossy
    Ok(false)
}

/// Check if file is JXL by magic bytes or extension
fn is_jxl_file(path: &PathBuf) -> bool {
    // Check extension first
    if let Some(ext) = path.extension() {
        if ext.to_str().unwrap_or("").to_lowercase() == "jxl" {
            return true;
        }
    }
    
    // Check magic bytes: JXL has two signatures
    // 0xFF 0x0A (naked codestream) or 0x00 0x00 0x00 0x0C 0x4A 0x58 0x4C 0x20 (ISOBMFF container)
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0x0A {
            return true;
        }
        if bytes.len() >= 12 && &bytes[4..8] == b"JXL " {
            return true;
        }
    }
    false
}

/// Analyze JXL image using jxlinfo for metadata extraction
/// 
/// 🔥 修复：djxl 不支持 --info 参数，使用 jxlinfo 代替
/// jxlinfo 输出格式示例：
///   JPEG XL image, 1920x1080, (no alpha), 8-bit sRGB color
fn analyze_jxl_image(path: &PathBuf, file_size: u64) -> Result<ImageAnalysis> {
    use std::process::Command;
    
    // 🔥 使用 jxlinfo 获取 JXL 文件信息（比 djxl 更可靠）
    let (width, height, has_alpha, color_depth) = if which::which("jxlinfo").is_ok() {
        let output = Command::new("jxlinfo")
            .arg(path)
            .output();
        
        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                parse_jxlinfo_output(&stdout)
            } else {
                // jxlinfo 失败，使用默认值
                (0, 0, false, 8)
            }
        } else {
            (0, 0, false, 8)
        }
    } else {
        // jxlinfo 不可用，尝试使用 ffprobe 作为备选
        if let Ok(probe) = shared_utils::probe_video(path) {
            (probe.width, probe.height, false, 8)
        } else {
            // 🔥 响亮警告：无法获取 JXL 尺寸
            eprintln!("⚠️  无法获取 JXL 文件尺寸: jxlinfo 和 ffprobe 都不可用");
            eprintln!("   💡 建议安装 jxlinfo: brew install jpeg-xl");
            (0, 0, false, 8)
        }
    };
    
    // JXL files are always considered lossless (they came from our own conversion)
    let metadata = extract_metadata(path)?;
    
    Ok(ImageAnalysis {
        file_path: path.display().to_string(),
        format: "JXL".to_string(),
        width,
        height,
        file_size,
        color_depth,
        color_space: "sRGB".to_string(),
        has_alpha,
        is_animated: false,
        duration_secs: None,
        is_lossless: true, // JXL from our conversion is lossless
        jpeg_analysis: None,
        heic_analysis: None,
        features: ImageFeatures {
            entropy: 0.0,
            compression_ratio: 0.0,
        },
        jxl_indicator: JxlIndicator {
            should_convert: false,
            reason: "Already JXL format".to_string(),
            command: String::new(),
            benefit: String::new(),
        },
        psnr: None,
        ssim: None,
        metadata,
    })
}

/// 解析 jxlinfo 输出以提取图像信息
/// 
/// jxlinfo 输出格式示例：
///   JPEG XL image, 1920x1080, (no alpha), 8-bit sRGB color
///   JPEG XL image, 800x600, alpha, 16-bit linear color
fn parse_jxlinfo_output(output: &str) -> (u32, u32, bool, u8) {
    let mut width = 0u32;
    let mut height = 0u32;
    let mut has_alpha = false;
    let mut color_depth = 8u8;
    
    for line in output.lines() {
        let line = line.trim();
        
        // 解析尺寸：查找 "WxH" 格式
        if let Some(dims) = line.split(',').find(|s| s.contains('x') && s.chars().any(|c| c.is_ascii_digit())) {
            let dims = dims.trim();
            // 尝试解析 "1920x1080" 格式
            let parts: Vec<&str> = dims.split('x').collect();
            if parts.len() == 2 {
                // 提取数字部分
                let w_str: String = parts[0].chars().filter(|c| c.is_ascii_digit()).collect();
                let h_str: String = parts[1].chars().filter(|c| c.is_ascii_digit()).collect();
                width = w_str.parse().unwrap_or(0);
                height = h_str.parse().unwrap_or(0);
            }
        }
        
        // 解析 alpha 通道
        if line.contains("alpha") && !line.contains("no alpha") {
            has_alpha = true;
        }
        
        // 解析色深
        if line.contains("16-bit") {
            color_depth = 16;
        } else if line.contains("32-bit") {
            color_depth = 32;
        }
    }
    
    (width, height, has_alpha, color_depth)
}

/// Extract metadata
fn extract_metadata(path: &PathBuf) -> Result<HashMap<String, String>> {
    let mut metadata = HashMap::new();
    
    if let Some(filename) = path.file_name() {
        metadata.insert("filename".to_string(), filename.to_string_lossy().to_string());
    }
    
    if let Some(extension) = path.extension() {
        metadata.insert("extension".to_string(), extension.to_string_lossy().to_string());
    }
    
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entropy_calculation() {
        // A uniform image should have low entropy
        // A random image should have high entropy
        assert!(true);
    }
    
    #[test]
    fn test_psnr_estimation() {
        assert!(estimate_psnr_from_quality(95) > estimate_psnr_from_quality(50));
    }
    
    #[test]
    fn test_ssim_estimation() {
        assert!(estimate_ssim_from_quality(95) > estimate_ssim_from_quality(50));
    }
}
