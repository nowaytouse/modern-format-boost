use crate::image_heic_analysis::{analyze_heic_file, is_heic_file, HeicAnalysis};
use crate::image_jpeg_analysis::{analyze_jpeg_file, JpegQualityAnalysis};
use crate::img_errors::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView, ImageFormat};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JxlIndicator {
    pub should_convert: bool,
    pub reason: String,
    pub command: String,
    pub benefit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageFeatures {
    pub entropy: f64,
    pub compression_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAnalysis {
    pub file_path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,

    pub color_depth: u8,
    pub color_space: String,
    pub has_alpha: bool,
    pub is_animated: bool,

    pub duration_secs: Option<f32>,

    pub is_lossless: bool,

    pub jpeg_analysis: Option<JpegQualityAnalysis>,

    pub heic_analysis: Option<HeicAnalysis>,

    pub features: ImageFeatures,

    pub jxl_indicator: JxlIndicator,

    pub psnr: Option<f64>,
    pub ssim: Option<f64>,
    pub metadata: HashMap<String, String>,
}

pub fn analyze_image(path: &Path) -> Result<ImageAnalysis> {
    if !path.exists() {
        return Err(ImgQualityError::ImageReadError(format!(
            "File not found: {}",
            path.display()
        )));
    }

    let file_size = std::fs::metadata(path)?.len();

    if is_heic_file(path) {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if !["heic", "heif", "hif"].contains(&ext_str.as_str()) {
                eprintln!(
                    "⚠️  [Smart Fix] Extension mismatch: '{}' (disguised as .{}) -> actually HEIC, will process as actual format",
                    path.display(),
                    ext_str
                );
            }
        }
        return analyze_heic_image(path, file_size);
    }

    if is_jxl_file(path) {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if ext_str != "jxl" {
                eprintln!(
                    "⚠️  [Smart Fix] Extension mismatch: '{}' (disguised as .{}) -> actually JXL, will process as actual format",
                    path.display(),
                    ext_str
                );
            }
        }
        return analyze_jxl_image(path, file_size);
    }

    // AVIF: image crate fails on some variants (e.g. tachimanga output); fall back to ffprobe
    if path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase() == "avif")
        .unwrap_or(false)
    {
        return analyze_avif_image(path, file_size);
    }

    let reader = image::ImageReader::open(path)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to open file: {}", e)))?
        .with_guessed_format()
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to guess format: {}", e)))?;

    let format = reader.format().ok_or_else(|| {
        ImgQualityError::UnsupportedFormat(format!(
            "Could not detect format for {}",
            path.display()
        ))
    })?;
    let format_str = format_to_string(&format);

    let mut extension_mismatch = false;
    let mut real_extension_suggestion = String::new();
    let mut apple_warning = String::new();

    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        let (is_valid, suggested) = match format {
            ImageFormat::Jpeg => (["jpg", "jpeg", "jpe"].contains(&ext_str.as_str()), "jpg"),
            ImageFormat::Png => (ext_str == "png", "png"),
            ImageFormat::WebP => (ext_str == "webp", "webp"),
            ImageFormat::Gif => (ext_str == "gif", "gif"),
            ImageFormat::Tiff => (["tiff", "tif"].contains(&ext_str.as_str()), "tiff"),
            ImageFormat::Avif => (ext_str == "avif", "avif"),
            _ => (true, ""),
        };

        if !is_valid && !suggested.is_empty() {
            extension_mismatch = true;
            real_extension_suggestion = suggested.to_string();

            eprintln!(
                 "⚠️  [Smart Fix] Extension mismatch: '{}' (disguised as .{}) -> actually {}, will process as actual format",
                 path.display(),
                 ext_str,
                 format_str
             );

            apple_warning = format!(
                 "⚠️ Extension mismatch (.{} vs {})。This will prevent Apple Photos import. Run repair_apple_photos.sh to fix.",
                 ext_str, format_str
             );
        }
    }

    let img = reader
        .decode()
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to decode image: {}", e)))?;

    let (width, height) = img.dimensions();
    let has_alpha = has_alpha_channel(&img);
    let color_depth = detect_color_depth(&img);
    let color_space = detect_color_space(&img);

    let is_animated = is_animated_format(path, &format)?;

    let is_lossless = detect_lossless(&format, path)?;

    let jpeg_analysis = if format == ImageFormat::Jpeg {
        analyze_jpeg_file(path).ok()
    } else {
        None
    };

    let features = calculate_image_features(&img, file_size);

    let jxl_indicator = generate_jxl_indicator(&format, is_lossless, &jpeg_analysis, path);

    let (psnr, ssim) = if let Some(ref jpeg) = jpeg_analysis {
        let estimated_psnr = estimate_psnr_from_quality(jpeg.estimated_quality);
        let estimated_ssim = estimate_ssim_from_quality(jpeg.estimated_quality);
        (Some(estimated_psnr), Some(estimated_ssim))
    } else {
        (None, None)
    };

    let mut metadata = extract_metadata(path)?;

    if extension_mismatch {
        metadata.insert("extension_mismatch".to_string(), "true".to_string());
        metadata.insert(
            "real_extension".to_string(),
            real_extension_suggestion.clone(),
        );
        metadata.insert(
            "apple_compatibility_warning".to_string(),
            apple_warning.clone(),
        );
        metadata.insert(
            "format_warning".to_string(),
            format!("Content is actually {}", format_str),
        );
    }

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

fn analyze_heic_image(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    let (width, height, has_alpha, color_depth, is_lossless, codec, features) =
        match analyze_heic_file(path) {
            Ok((img, heic_analysis)) => {
                let (w, h) = img.dimensions();
                let feats = calculate_image_features(&img, file_size);
                (
                    w,
                    h,
                    heic_analysis.has_alpha,
                    heic_analysis.bit_depth,
                    heic_analysis.is_lossless,
                    heic_analysis.codec,
                    feats,
                )
            }
            Err(e) => {
                eprintln!(
                    "⚠️ Deep HEIC analysis failed (skipping to basic info): {}",
                    e
                );
                (
                    0,
                    0,
                    false,
                    8,
                    false,
                    "unknown".to_string(),
                    ImageFeatures::default(),
                )
            }
        };

    let jxl_indicator = JxlIndicator {
        should_convert: false,
        reason: format!("HEIC is already a modern efficient format ({})", codec),
        command: String::new(),
        benefit: String::new(),
    };

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
        heic_analysis: None,
        features,
        jxl_indicator,
        psnr: None,
        ssim: None,
        metadata,
    })
}

fn generate_jxl_indicator(
    format: &ImageFormat,
    is_lossless: bool,
    jpeg_analysis: &Option<JpegQualityAnalysis>,
    path: &Path,
) -> JxlIndicator {
    let file_path = path.display().to_string();
    let output_path = path.with_extension("jxl").display().to_string();

    match format {
        ImageFormat::Png | ImageFormat::Gif | ImageFormat::Tiff => JxlIndicator {
            should_convert: true,
            reason: "Lossless image; strongly recommend converting to JXL".to_string(),
            command: format!(
                "cjxl '{}' '{}' -d 0.0 --modular=1 -e 9",
                file_path, output_path
            ),
            benefit: "30-60% size reduction while preserving full quality".to_string(),
        },
        ImageFormat::Jpeg => {
            if let Some(ref jpeg) = jpeg_analysis {
                let quality_info = format!("original quality Q={}", jpeg.estimated_quality);
                JxlIndicator {
                    should_convert: true,
                    reason: format!("JPEG ({}), lossless transcode to JXL", quality_info),
                    command: format!("cjxl '{}' '{}' --lossless_jpeg=1", file_path, output_path),
                    benefit:
                        "Keeps original JPEG DCT coefficients, reversible, ~20% size reduction"
                            .to_string(),
                }
            } else {
                JxlIndicator {
                    should_convert: true,
                    reason: "JPEG can be losslessly transcoded to JXL".to_string(),
                    command: format!("cjxl '{}' '{}' --lossless_jpeg=1", file_path, output_path),
                    benefit: "Keeps original JPEG DCT coefficients, reversible".to_string(),
                }
            }
        }
        ImageFormat::WebP => {
            if is_lossless {
                JxlIndicator {
                    should_convert: true,
                    reason: "Lossless WebP; recommend converting to JXL".to_string(),
                    command: format!(
                        "cjxl '{}' '{}' -d 0.0 --modular=1 -e 9",
                        file_path, output_path
                    ),
                    benefit: "JXL is typically more efficient than lossless WebP".to_string(),
                }
            } else {
                JxlIndicator {
                    should_convert: false,
                    reason: "Lossy WebP; conversion may cause additional quality loss".to_string(),
                    command: String::new(),
                    benefit: String::new(),
                }
            }
        }
        ImageFormat::Avif => JxlIndicator {
            should_convert: false,
            reason: "AVIF is already a modern efficient format; no conversion needed".to_string(),
            command: String::new(),
            benefit: String::new(),
        },
        _ => JxlIndicator {
            should_convert: false,
            reason: "Unsupported format or no conversion needed".to_string(),
            command: String::new(),
            benefit: String::new(),
        },
    }
}

fn calculate_image_features(img: &DynamicImage, file_size: u64) -> ImageFeatures {
    let (width, height) = img.dimensions();
    let channels = match img.color() {
        image::ColorType::L8 | image::ColorType::L16 => 1,
        image::ColorType::La8 | image::ColorType::La16 => 2,
        image::ColorType::Rgb8 | image::ColorType::Rgb16 | image::ColorType::Rgb32F => 3,
        _ => 4,
    };
    let bits_per_channel = match img.color() {
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
    };

    let raw_size =
        (width as u64) * (height as u64) * (channels as u64) * (bits_per_channel as u64 / 8);

    let compression_ratio = if raw_size > 0 {
        file_size as f64 / raw_size as f64
    } else {
        1.0
    };

    let entropy = calculate_entropy(img);

    ImageFeatures {
        entropy,
        compression_ratio,
    }
}

fn calculate_entropy(img: &DynamicImage) -> f64 {
    let gray = img.to_luma8();
    let pixels = gray.as_raw();

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

fn estimate_psnr_from_quality(quality: u8) -> f64 {
    match quality {
        95..=100 => 45.0 + (quality as f64 - 95.0) * 0.5,
        85..=94 => 38.0 + (quality as f64 - 85.0) * 0.7,
        75..=84 => 32.0 + (quality as f64 - 75.0) * 0.6,
        60..=74 => 28.0 + (quality as f64 - 60.0) * 0.27,
        _ => 20.0 + (quality as f64) * 0.13,
    }
}

fn estimate_ssim_from_quality(quality: u8) -> f64 {
    match quality {
        95..=100 => 0.98 + (quality as f64 - 95.0) * 0.004,
        85..=94 => 0.95 + (quality as f64 - 85.0) * 0.003,
        75..=84 => 0.90 + (quality as f64 - 75.0) * 0.005,
        60..=74 => 0.80 + (quality as f64 - 60.0) * 0.0067,
        _ => 0.60 + (quality as f64) * 0.003,
    }
}

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

fn has_alpha_channel(img: &DynamicImage) -> bool {
    matches!(
        img.color(),
        image::ColorType::Rgba8
            | image::ColorType::Rgba16
            | image::ColorType::La8
            | image::ColorType::La16
    )
}

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

fn detect_color_space(img: &DynamicImage) -> String {
    match img.color() {
        image::ColorType::L8
        | image::ColorType::L16
        | image::ColorType::La8
        | image::ColorType::La16 => "Grayscale".to_string(),
        _ => "sRGB".to_string(),
    }
}

fn is_animated_format(path: &Path, format: &ImageFormat) -> Result<bool> {
    match format {
        ImageFormat::Gif => Ok(check_gif_animation(path)?),
        ImageFormat::WebP => Ok(check_webp_animation(path)?),
        _ => Ok(false),
    }
}

fn check_gif_animation(path: &Path) -> Result<bool> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
    let bytes = std::fs::read(path)?;
    Ok(crate::image_formats::gif::is_animated_from_bytes(&bytes))
}

fn check_webp_animation(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    Ok(crate::image_formats::webp::is_animated_from_bytes(&bytes))
}

fn get_animation_duration(path: &Path) -> Option<f32> {
    if let Some(duration) = try_ffprobe_json(path) {
        return Some(duration);
    }

    if let Some(duration) = try_ffprobe_default(path) {
        return Some(duration);
    }

    if let Some(duration) = try_imagemagick_identify(path) {
        return Some(duration);
    }

    if let Some(ext) = path.extension() {
        if ext.to_str().unwrap_or("").to_lowercase() == "gif" {
            if let Some(frame_count) = try_get_frame_count(path) {
                if frame_count <= 1 {
                    eprintln!("🔍 Detected static GIF (1 frame): {}", path.display());
                    return Some(0.0);
                } else {
                    let estimated_duration = frame_count as f32 / 10.0;
                    eprintln!(
                        "📊 Estimated duration from frame count: {:.2}s ({} frames)",
                        estimated_duration, frame_count
                    );
                    return Some(estimated_duration);
                }
            }
        }
    }

    None
}

fn try_ffprobe_json(path: &Path) -> Option<f32> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_format"])
        .arg(crate::safe_path_arg(path).as_ref())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8_lossy(&output.stdout);

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

fn try_ffprobe_default(path: &Path) -> Option<f32> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(crate::safe_path_arg(path).as_ref())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    duration_str.parse::<f32>().ok()
}

/// Returns (duration_secs, frame_count) from ImageMagick identify (WebP/GIF animation).
/// Use as fallback when ffprobe has no stream/format duration. Does not log.
pub fn get_animation_duration_and_frames_imagemagick(path: &Path) -> Option<(f64, u64)> {
    use std::process::Command;

    let safe_path = crate::safe_path_arg(path);
    let output = Command::new("magick")
        .args(["identify", "-format", "%T\n"])
        .arg(safe_path.as_ref())
        .output()
        .or_else(|_| {
            Command::new("identify")
                .args(["-format", "%T\n"])
                .arg(safe_path.as_ref())
                .output()
        })
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut total_cs = 0u32;
    let mut frame_count = 0u32;

    for line in stdout.lines() {
        if let Ok(delay_cs) = line.trim().parse::<u32>() {
            total_cs += delay_cs;
            frame_count += 1;
        }
    }

    if frame_count == 0 {
        return None;
    }

    let duration_secs = total_cs as f64 / 100.0;
    Some((duration_secs, frame_count as u64))
}

fn try_imagemagick_identify(path: &Path) -> Option<f32> {
    if let Some((duration_secs, frame_count)) = get_animation_duration_and_frames_imagemagick(path)
    {
        eprintln!(
            "📊 ImageMagick: WebP/GIF animation detected ({} frames, {:.2}s)",
            frame_count, duration_secs
        );
        return Some(duration_secs as f32);
    }
    None
}

fn try_get_frame_count(path: &Path) -> Option<u32> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_packets",
            "-show_entries",
            "stream=nb_read_packets",
            "-of",
            "csv=p=0",
        ])
        .arg(crate::safe_path_arg(path).as_ref())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let count_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    count_str.parse::<u32>().ok()
}

fn detect_lossless(format: &ImageFormat, path: &Path) -> Result<bool> {
    match format {
        ImageFormat::Png => {
            use crate::image_detection::{
                detect_compression, detect_format_from_bytes, CompressionType,
            };

            let detected_format = detect_format_from_bytes(path)?;
            let compression = detect_compression(&detected_format, path)?;

            Ok(compression == CompressionType::Lossless)
        }
        ImageFormat::Gif => Ok(true),
        ImageFormat::Tiff => Ok(true),
        ImageFormat::Jpeg => Ok(false),
        ImageFormat::WebP => check_webp_lossless(path),
        ImageFormat::Avif => check_avif_lossless(path),
        _ => Ok(false),
    }
}

fn check_webp_lossless(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    Ok(crate::image_formats::webp::is_lossless_from_bytes(&bytes))
}

fn check_avif_lossless(path: &Path) -> Result<bool> {
    let _bytes = std::fs::read(path)?;

    Ok(false)
}

fn is_jxl_file(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        if ext.to_str().unwrap_or("").to_lowercase() == "jxl" {
            return true;
        }
    }

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

fn analyze_jxl_image(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    use std::process::Command;

    let (width, height, has_alpha, color_depth) = if which::which("jxlinfo").is_ok() {
        let output = Command::new("jxlinfo")
            .arg(crate::safe_path_arg(path).as_ref())
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                parse_jxlinfo_output(&stdout)
            } else {
                (0, 0, false, 8)
            }
        } else {
            (0, 0, false, 8)
        }
    } else {
        if let Ok(probe) = crate::probe_video(path) {
            (probe.width, probe.height, false, 8)
        } else {
            eprintln!("⚠️  Cannot get JXL file dimensions: both jxlinfo and ffprobe unavailable");
            eprintln!("   💡 Suggestion: install jxlinfo: brew install jpeg-xl");
            (0, 0, false, 8)
        }
    };

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
        is_lossless: true,
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

fn analyze_avif_image(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    // Use ffprobe directly for AVIF: the `image` crate's AVIF decoder rejects many
    // valid files (10-bit, HDR color spaces, certain profiles). ffprobe handles them
    // correctly and also provides pix_fmt for accurate alpha and bit-depth detection.
    let (width, height, has_alpha, color_depth) = if let Ok(probe) = crate::probe_video(path) {
        let pix_fmt = probe.pix_fmt.to_lowercase();
        let alpha = pix_fmt.contains("yuva")
            || pix_fmt.contains("rgba")
            || pix_fmt.contains("gbrap")
            || pix_fmt.starts_with("p4");
        let depth = if probe.bit_depth > 0 {
            probe.bit_depth
        } else {
            8
        };
        (probe.width, probe.height, alpha, depth)
    } else if let Ok(img) = image::open(path) {
        let (w, h) = img.dimensions();
        (w, h, has_alpha_channel(&img), detect_color_depth(&img))
    } else {
        (0u32, 0u32, false, 8u8)
    };
    let metadata = extract_metadata(path).unwrap_or_default();
    Ok(ImageAnalysis {
        file_path: path.display().to_string(),
        format: "AVIF".to_string(),
        width,
        height,
        file_size,
        color_depth,
        color_space: "sRGB".to_string(),
        has_alpha,
        is_animated: false,
        duration_secs: None,
        is_lossless: false,
        jpeg_analysis: None,
        heic_analysis: None,
        features: ImageFeatures {
            entropy: 0.0,
            compression_ratio: 0.0,
        },
        jxl_indicator: JxlIndicator {
            should_convert: false,
            reason: "AVIF is already a modern efficient format; no conversion needed".to_string(),
            command: String::new(),
            benefit: String::new(),
        },
        psnr: None,
        ssim: None,
        metadata,
    })
}

fn parse_jxlinfo_output(output: &str) -> (u32, u32, bool, u8) {
    let mut width = 0u32;
    let mut height = 0u32;
    let mut has_alpha = false;
    let mut color_depth = 8u8;

    for line in output.lines() {
        let line = line.trim();

        if let Some(dims) = line
            .split(',')
            .find(|s| s.contains('x') && s.chars().any(|c| c.is_ascii_digit()))
        {
            let dims = dims.trim();
            let parts: Vec<&str> = dims.split('x').collect();
            if parts.len() == 2 {
                let w_str: String = parts[0].chars().filter(|c| c.is_ascii_digit()).collect();
                let h_str: String = parts[1].chars().filter(|c| c.is_ascii_digit()).collect();
                width = w_str.parse().unwrap_or(0);
                height = h_str.parse().unwrap_or(0);
            }
        }

        if line.contains("alpha") && !line.contains("no alpha") {
            has_alpha = true;
        }

        if line.contains("16-bit") {
            color_depth = 16;
        } else if line.contains("32-bit") {
            color_depth = 32;
        }
    }

    (width, height, has_alpha, color_depth)
}

fn extract_metadata(path: &Path) -> Result<HashMap<String, String>> {
    let mut metadata = HashMap::new();

    if let Some(filename) = path.file_name() {
        metadata.insert(
            "filename".to_string(),
            filename.to_string_lossy().to_string(),
        );
    }

    if let Some(extension) = path.extension() {
        metadata.insert(
            "extension".to_string(),
            extension.to_string_lossy().to_string(),
        );
    }

    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_psnr_estimation() {
        let psnr_high = estimate_psnr_from_quality(95);
        let psnr_mid = estimate_psnr_from_quality(75);
        let psnr_low = estimate_psnr_from_quality(50);

        assert!(psnr_high > psnr_mid);
        assert!(psnr_mid > psnr_low);
        assert!(psnr_high >= 40.0);
        assert!(psnr_low >= 25.0);
    }

    #[test]
    fn test_ssim_estimation() {
        let ssim_high = estimate_ssim_from_quality(95);
        let ssim_mid = estimate_ssim_from_quality(75);
        let ssim_low = estimate_ssim_from_quality(50);

        assert!(ssim_high > ssim_mid);
        assert!(ssim_mid > ssim_low);
        assert!(ssim_high >= 0.95);
        assert!(ssim_low >= 0.70);
    }

    #[test]
    fn test_quality_boundaries() {
        let psnr_max = estimate_psnr_from_quality(100);
        let psnr_min = estimate_psnr_from_quality(1);

        assert!(psnr_max > psnr_min);
        assert!(psnr_max.is_finite());
        assert!(psnr_min.is_finite());
    }
}
