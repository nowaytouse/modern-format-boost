use crate::ffprobe_json::ColorInfo;
use crate::image_heic_analysis::{analyze_heic_file, is_heic_file, HeicAnalysis};
use crate::image_jpeg_analysis::{analyze_jpeg_file, JpegQualityAnalysis};
use crate::image_detection::{PrecisionMetadata, detect_image};
use crate::img_errors::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView, ImageFormat};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use crate::log_eprintln;
use crate::types::{ProcessHistory, VisualPerception};

/// Minimum duration (seconds) for converting animated images to HEVC video.
/// Shorter animations are skipped (no conversion to video).
pub const ANIMATED_MIN_DURATION_FOR_VIDEO_SECS: f32 = 4.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JxlIndicator {
    pub should_convert: bool,
    pub reason: String,
    pub command: String,
    pub benefit: String,
}

impl Default for JxlIndicator {
    fn default() -> Self {
        Self {
            should_convert: false,
            reason: "Initial".into(),
            command: String::new(),
            benefit: String::new(),
        }
    }
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

    /// HDR metadata extracted from image (bit depth, color transfer, primaries, mastering display)
    pub hdr_info: Option<ColorInfo>,

    pub precision: PrecisionMetadata,

    /// 🛠️ New Dimension: Processing history for cache invalidation logic
    pub history: ProcessHistory,

    /// 🔬 New Dimension: Visual perception data (Auxiliary analysis)
    pub perception: VisualPerception,
}

impl Default for ImageAnalysis {
    fn default() -> Self {
        Self {
            file_path: String::new(),
            format: "unknown".into(),
            width: 0,
            height: 0,
            file_size: 0,
            color_depth: 8,
            color_space: "unknown".into(),
            has_alpha: false,
            is_animated: false,
            duration_secs: None,
            is_lossless: false,
            jpeg_analysis: None,
            heic_analysis: None,
            features: ImageFeatures::default(),
            jxl_indicator: JxlIndicator::default(),
            psnr: None,
            ssim: None,
            metadata: HashMap::new(),
            hdr_info: None,
            precision: PrecisionMetadata::default(),
            history: ProcessHistory::default(),
            perception: VisualPerception::default(),
        }
    }
}

/// Analyzes an image file. Format detection order (by path/content): HEIC → JXL → AVIF → image crate (PNG/JPEG/WebP/GIF/TIFF).
/// Quality is then derived via detect_lossless / detect_compression per format; no conversion is done here.
pub fn analyze_image(path: &Path) -> Result<ImageAnalysis> {
    analyze_image_with_cache(path, None)
}

/// Analyzes an image file with optional SQLite caching.
pub fn analyze_image_with_cache(path: &Path, cache: Option<&crate::analysis_cache::AnalysisCache>) -> Result<ImageAnalysis> {
    // Fast-path for JPEGs: Bypass the SQLite cache entirely because:
    // 1. JPEG analysis (DQT markers only) is faster than SQLite/Hashing overhead.
    // 2. We don't need pixel-level features for JPEG->JXL lossless transcoding.
    let is_jpeg_hint = path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .map(|e| e == "jpg" || e == "jpeg")
        .unwrap_or(false);

    if is_jpeg_hint && cache.is_some() {
        if let Ok(analysis) = analyze_image_internal(path) {
            return Ok(analysis);
        }
    }

    if let Some(cache) = cache {
        match cache.get_analysis(path) {
            Ok(Some(cached)) => {
                if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                    log_eprintln!("🔍 [Cache] Hit: {}", path.display());
                }
                return Ok(cached);
            },
            Err(e) => {
                if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                    log_eprintln!("⚠️ [Cache] Retrieval error: {}", e);
                }
            },
            _ => {}
        }
    }

    let analysis = analyze_image_internal(path)?;
    
    if let Some(cache) = cache {
        if let Err(e) = cache.store_analysis(path, &analysis) {
            if std::env::var("IMGQUALITY_DEBUG").is_ok() {
                log_eprintln!("⚠️ [Cache] Store error: {}", e);
            }
        }
    }
    
    Ok(analysis)
}

fn analyze_image_internal(path: &Path) -> Result<ImageAnalysis> {
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
                log_eprintln!(
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
                log_eprintln!(
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

    let mut reader = image::ImageReader::open(path)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to open file: {}", e)))?
        .with_guessed_format()
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to guess format: {}", e)))?;
    {
        use image::Limits;
        let mut limits = Limits::default();
        limits.max_alloc = Some(2 * 1024 * 1024 * 1024);
        reader.limits(limits);
    }

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
        
        // Fast-path: If it's a JPEG, skip decode() entirely.
        if format == ImageFormat::Jpeg {
            return analyze_jpeg_fast_path(path, file_size);
        }

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

            log_eprintln!(
                 "⚠️  [Smart Fix] Extension mismatch: '{}' (disguised as .{}) -> actually {}, will process as actual format",
                 path.display(),
                 ext_str,
                 format_str
             );

            apple_warning = format!(
                 "⚠️ Extension mismatch (.{} vs {}). This will prevent Apple Photos import. Run repair_apple_photos.sh to fix.",
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

    let is_lossless = detect_lossless(&format, path).unwrap_or_else(|_| pixel_fallback_lossless(path));

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

    // Extract HDR metadata using ffprobe
    let hdr_info = extract_hdr_info(path);

    let precision = if let Ok(detection) = detect_image(path) {
        detection.precision
    } else {
        PrecisionMetadata::default()
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
        hdr_info,
        precision,
        history: crate::common_utils::get_current_history(),
        perception: Default::default(),
    })
}

impl ImageAnalysis {
    /// Returns a human-readable quality summary label (e.g. "Q=95 Excellence", "Lossless").
    pub fn quality_summary(&self) -> String {
        if let Some(ref jpeg) = self.jpeg_analysis {
            format!("Q={} {}", jpeg.estimated_quality, jpeg.quality_description)
        } else if let Some(ref heic) = self.heic_analysis {
            if heic.is_lossless {
                "Lossless".to_string()
            } else {
                format!("{} {}", heic.codec, if heic.bit_depth > 8 { "HDR" } else { "SD" })
            }
        } else if self.is_lossless {
            "Lossless".to_string()
        } else {
            "Lossy".to_string()
        }
    }
}

fn analyze_heic_image(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    let (width, height, has_alpha, color_depth, is_lossless, codec, features, heic_analysis_opt) =
        match analyze_heic_file(path) {
            Ok((img, heic_analysis)) => {
                // Skip HEIC files with HDR or Dolby Vision
                if heic_analysis.is_hdr || heic_analysis.is_dolby_vision {
                    let reason = if heic_analysis.is_dolby_vision {
                        "HEIC with Dolby Vision - skipping to preserve HDR metadata"
                    } else {
                        "HEIC with HDR - skipping to preserve HDR metadata"
                    };
                    return Err(ImgQualityError::SkipFile(reason.to_string()));
                }

                let (w, h) = img.dimensions();
                let feats = calculate_image_features(&img, file_size);
                let is_lossless = crate::image_detection::detect_compression(
                    &crate::image_detection::DetectedFormat::HEIC,
                    path,
                )
                .map(|c| c == crate::image_detection::CompressionType::Lossless)
                .unwrap_or_else(|_| pixel_fallback_lossless(path));
                (
                    w,
                    h,
                    heic_analysis.has_alpha,
                    heic_analysis.bit_depth,
                    is_lossless,
                    heic_analysis.codec.clone(),
                    feats,
                    Some(heic_analysis),
                )
            }
            Err(e) => {
                log_eprintln!(
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
                    None,
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

    // Extract HDR metadata using ffprobe
    let hdr_info = extract_hdr_info(path);

    // HEIC/HEIF animation detection for metadata correctness.
    // Routing-wise, HEIC/HEIF is intercepted by is_apple_native before this matters,
    // but correct is_animated ensures downstream consumers get truthful metadata.
    let is_animated = crate::image_detection::is_isobmff_animated_sequence(path);
    let duration_secs = if is_animated {
        get_animation_duration(path)
    } else {
        None
    };

    Ok(ImageAnalysis {
        file_path: path.display().to_string(),
        format: "HEIC".to_string(),
        width,
        height,
        file_size,
        color_depth,
        color_space: "sRGB".to_string(),
        has_alpha,
        is_animated,
        duration_secs,
        is_lossless,
        jpeg_analysis: None,
        heic_analysis: heic_analysis_opt,
        features,
        jxl_indicator,
        psnr: None,
        ssim: None,
        metadata,
        hdr_info,
        precision: if let Ok(d) = detect_image(path) { d.precision } else { PrecisionMetadata::default() },
        history: crate::common_utils::get_current_history(),
        perception: Default::default(),
    })
}

/// Specialized fast path for JPEG files to avoid expensive pixel decoding.
/// JPEG->JXL transcoding only needs quantization tables, not raw pixels.
fn analyze_jpeg_fast_path(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    let jpeg_analysis = analyze_jpeg_file(path).ok();
    
    // Use fast metadata parsing to get dimensions without decoding pixels
    let (width, height) = if let Ok(reader) = image::ImageReader::open(path) {
        if let Ok(dim) = reader.into_dimensions() {
            dim
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    let metadata = extract_metadata(path).unwrap_or_default();
    let jxl_indicator = generate_jxl_indicator(&ImageFormat::Jpeg, false, &jpeg_analysis, path);

    let (psnr, ssim) = if let Some(ref jpeg) = jpeg_analysis {
        (
            Some(estimate_psnr_from_quality(jpeg.estimated_quality)),
            Some(estimate_ssim_from_quality(jpeg.estimated_quality))
        )
    } else {
        (None, None)
    };

    Ok(ImageAnalysis {
        file_path: path.display().to_string(),
        format: "JPEG".to_string(),
        width,
        height,
        file_size,
        color_depth: 8,
        color_space: "sRGB".to_string(),
        has_alpha: false,
        is_animated: false,
        duration_secs: None,
        is_lossless: false,
        jpeg_analysis,
        heic_analysis: None,
        features: ImageFeatures {
            entropy: 0.0, // Skipped for performance
            compression_ratio: 0.0,
        },
        jxl_indicator,
        psnr,
        ssim,
        metadata,
        hdr_info: extract_hdr_info(path),
        precision: PrecisionMetadata {
            bit_depth: Some(8),
            is_lossless_deterministic: false,
            ..Default::default()
        },
        history: crate::common_utils::get_current_history(),
        perception: Default::default(),
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
        ImageFormat::Avif => {
            if is_lossless {
                JxlIndicator {
                    should_convert: true,
                    reason: "Lossless AVIF; recommend converting to JXL".to_string(),
                    command: format!(
                        "cjxl '{}' '{}' -d 0.0 --modular=1 -e 9",
                        file_path, output_path
                    ),
                    benefit: "JXL modular mode is typically more efficient than AVIF lossless".to_string(),
                }
            } else {
                JxlIndicator {
                    should_convert: false,
                    reason: "AVIF is already a modern efficient format; no conversion needed".to_string(),
                    command: String::new(),
                    benefit: String::new(),
                }
            }
        }
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
        ImageFormat::Bmp => "BMP".to_string(),
        ImageFormat::Ico => "ICO".to_string(),
        ImageFormat::Pnm => "PNM".to_string(),
        ImageFormat::Tga => "TGA".to_string(),
        ImageFormat::Hdr => "HDR".to_string(),
        ImageFormat::Farbfeld => "Farbfeld".to_string(),
        ImageFormat::OpenExr => "OpenEXR".to_string(),
        ImageFormat::Dds => "DDS".to_string(),
        ImageFormat::Qoi => "QOI".to_string(),
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
        ImageFormat::Png => Ok(check_png_animation(path)?),
        _ => Ok(false),
    }
}

fn check_png_animation(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    
    // Stage 1: Structural Walk (Official Spec)
    let (structural_is_animated, structural_count) = crate::image_detection::parse_apng_frames(&bytes);
    if structural_is_animated && structural_count > 1 {
        return Ok(true);
    }

    // Stage 2: Feature Scan (Loose Bitstream Search)
    // Scan for acTL [61 63 54 4C] or fcTL [66 63 54 4C] markers
    let has_actl = bytes.windows(4).any(|w| w == b"acTL");
    let has_fctl = bytes.windows(4).any(|w| w == b"fcTL");

    if (has_actl || has_fctl) && !structural_is_animated {
        // [Disagreement] Deep Internal Validation
        if deep_research_png_animation(&bytes) {
             log_eprintln!("🎞️  [Deep Research: APNG] Structural walk failed but internal byte-research confirmed fcTL markers: {}", path.display());
             return Ok(true);
        }

        // Final tie-breaker for ambiguous cases
        if let Some(duration) = get_animation_duration(path) {
            if duration > 0.0 {
                 log_eprintln!("🎞️  [Joint Audit: APNG] Structural walk failed but bitstream hints and duration confirm animation: {}", path.display());
                 return Ok(true);
            }
        }
    }

    Ok(false)
}

fn check_gif_animation(path: &Path) -> Result<bool> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
    let bytes = std::fs::read(path)?;
    
    // Stage 1: Structural Count (spec-compliant chunk walking)
    let structural_count = crate::image_formats::gif::count_frames_from_bytes(&bytes);
    if structural_count > 1 {
        return Ok(true);
    }

    // Stage 2: Feature Scan (Signal B)
    // Look for GCE markers [0x21, 0xF9, 0x04] globally
    let gce_marker = &[0x21, 0xF9, 0x04];
    let gce_hints = bytes.windows(3).filter(|w| *w == gce_marker).count() as u32;

    if gce_hints > structural_count {
        // [Disagreement] Internal Deep Research
        if deep_research_gif_animation(&bytes, gce_hints) {
             log_eprintln!("🎞️  [Deep Research: GIF] Structural scan saw {} frames, but internal byte-research confirmed {} valid GCE markers: {}", structural_count, gce_hints, path.display());
             return Ok(true);
        }

        // Final Tie-breaker: if byte-scan and structural-scan disagree on frame count,
        // check external duration to settle the dispute.
        if let Some(duration) = try_ffprobe_json(path) {
            if duration > 0.0 {
                log_eprintln!("🎞️  [Joint Audit: GIF] Structural scan missed animation ({} frames), but GCE hints ({}) and duration confirm it: {}", structural_count, gce_hints, path.display());
                return Ok(true);
            }
        }
    }
    
    Ok(structural_count > 1)
}

fn check_webp_animation(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    
    // Stage 1: RIFF-structural frame counting
    let structural_count = crate::image_formats::webp::count_frames_from_bytes(&bytes);
    if structural_count > 1 {
        return Ok(true);
    }

    // Stage 2: Feature Scanning
    // Look for global 'ANIM' or 'ANMF' chunks that might have been skipped in structural walk
    let has_anim = bytes.windows(4).any(|w| w == b"ANIM");
    let has_anmf = bytes.windows(4).any(|w| w == b"ANMF");

    if (has_anim || has_anmf) && structural_count <= 1 {
        // [Disagreement] Internal Deep Research
        // Count consistent animation frame chunks (ANMF for WebP Extended)
        let mut confirmed_frames = 0;
        let mut p = 0;
        while p + 8 < bytes.len() {
            if &bytes[p..p+4] == b"ANMF" {
                confirmed_frames += 1;
            }
            p += 1;
        }

        if confirmed_frames > 1 {
             log_eprintln!("🎞️  [Deep Research: WebP] Structural scan missed frames, but internal byte-research confirmed {} ANMF chunks: {}", confirmed_frames, path.display());
             return Ok(true);
        }

        // Final fallback tie-breaker
        if let Some(duration) = get_animation_duration(path) {
            if duration > 0.01 {
                log_eprintln!("🎞️  [Joint Audit: WebP] Byte markers found but structural walk failed; duration confirmed animation: {}", path.display());
                return Ok(true);
            }
        }
    }

    Ok(structural_count > 1)
}

/// Internal Deep Research: GIF
/// Validates if GCE markers are consistent with GIF block structure.
fn deep_research_gif_animation(bytes: &[u8], gce_hints: u32) -> bool {
    if gce_hints <= 1 { return false; }
    
    // Look for GCE patterns and verify if they are followed by valid block terminators
    // GCE = [21 F9 04 ... 00]
    let mut confirmed = 0;
    let mut i = 0;
    while i + 6 < bytes.len() {
        if bytes[i..i+3] == [0x21, 0xF9, 0x04] && bytes[i+7] == 0x00 {
            confirmed += 1;
        }
        i += 1;
    }
    
    confirmed > 1
}

/// Internal Deep Research: APNG
/// Validates if fcTL/acTL markers are consistent.
fn deep_research_png_animation(bytes: &[u8]) -> bool {
    // APNG uses fcTL (Frame Control Chunk)
    let mut confirmed_fctl = 0;
    let mut i = 8; // skip signature
    while i + 8 < bytes.len() {
        if &bytes[i+4..i+8] == b"fcTL" {
            confirmed_fctl += 1;
        }
        i += 1;
    }
    confirmed_fctl > 0 // Even 1 fcTL usually means it's an APNG (the first frame might have it)
}

/// Public entry for retrying animation duration (e.g. from main when analysis.duration_secs is None).
/// Tries ffprobe, ImageMagick, WebP native parse, and GIF frame-count estimate.
pub fn get_animation_duration_for_path(path: &Path) -> Option<f32> {
    get_animation_duration(path)
}

fn get_animation_duration(path: &Path) -> Option<f32> {
    // Special handling for JXL: FFmpeg's jpegxl_anim decoder is incomplete
    // Convert to temporary APNG first, then probe duration
    if path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .as_deref()
        == Some("jxl")
    {
        if let Some(duration) = try_jxl_via_apng(path) {
            return Some(duration);
        }
    }

    if let Some(duration) = try_ffprobe_json(path) {
        return Some(duration);
    }

    if let Some(duration) = try_ffprobe_default(path) {
        return Some(duration);
    }

    if let Some(duration) = try_imagemagick_identify(path) {
        return Some(duration);
    }

    if path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .as_deref()
        == Some("webp")
    {
        if let Ok(data) = std::fs::read(path) {
            if let Some(secs) = crate::image_formats::webp::duration_secs_from_bytes(&data) {
                return Some(secs);
            }
        }
    }

    if let Some(ext) = path.extension() {
        if ext.to_str().unwrap_or("").to_lowercase() == "gif" {
            if let Some(frame_count) = try_get_frame_count(path) {
                if frame_count <= 1 {
                    log_eprintln!("🔍 Detected static GIF (1 frame): {}", path.display());
                    return Some(0.0);
                }
            }
        }
    }

    None
}

fn try_jxl_via_apng(path: &Path) -> Option<f32> {
    use std::process::Command;
    
    // Check if djxl is available
    if which::which("djxl").is_err() {
        log_eprintln!("⚠️  djxl not found; cannot process animated JXL");
        return None;
    }
    
    // Create temporary APNG file
    let temp_apng = tempfile::Builder::new()
        .suffix(".apng")
        .tempfile()
        .ok()?;
    let temp_apng_path = temp_apng.path();
    
    // Convert JXL to APNG using djxl
    let djxl_result = Command::new("djxl")
        .arg(crate::safe_path_arg(path).as_ref())
        .arg(crate::safe_path_arg(temp_apng_path).as_ref())
        .output()
        .ok()?;
    
    if !djxl_result.status.success() || !temp_apng_path.exists() {
        log_eprintln!("⚠️  djxl conversion failed for JXL");
        return None;
    }
    
    log_eprintln!("🔧 JXL detected, converted to temporary APNG for duration detection");
    
    // APNG doesn't have duration in format metadata, we need to calculate from frames and fps
    // Use ffprobe with -count_frames to get nb_read_frames
    let probe_output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-count_frames",
            "-show_entries", "stream=nb_read_frames,r_frame_rate",
            "-of", "json",
            "--",
        ])
        .arg(crate::safe_path_arg(temp_apng_path).as_ref())
        .output()
        .ok()?;
    
    if probe_output.status.success() {
        let json_str = String::from_utf8_lossy(&probe_output.stdout);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let Some(stream) = json["streams"].as_array().and_then(|s| s.first()) {
                let nb_frames = stream["nb_read_frames"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                
                let r_frame_rate = stream["r_frame_rate"]
                    .as_str()
                    .unwrap_or("0/1");
                
                // Parse frame rate (format: "num/den")
                let fps = if let Ok(rate) = crate::ffprobe::parse_frame_rate(r_frame_rate) {
                    rate as f32
                } else {
                    0.0
                };
                
                if nb_frames > 0 && fps > 0.0 {
                    let duration = nb_frames as f32 / fps;
                    log_eprintln!("📊 JXL animation: {} frames @ {:.2} fps = {:.2}s", 
                        nb_frames, fps, duration);
                    return Some(duration);
                }
            }
        }
    }
    
    // Fallback: try ffprobe methods
    // temp_apng will be automatically cleaned up when dropped
    try_ffprobe_json(temp_apng_path)
        .or_else(|| try_ffprobe_default(temp_apng_path))
}

fn try_ffprobe_json(path: &Path) -> Option<f32> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "--"])
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
            "--",
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

/// Returns (duration_secs, frame_count) from ImageMagick `identify -format "%T"`.
/// Works for any format ImageMagick can read and that has per-frame delay (e.g. GIF, WebP, AVIF, JXL, APNG).
/// Use as fallback when ffprobe has no stream/format duration. Emits a warning log when used.
pub fn get_animation_duration_and_frames_imagemagick(path: &Path) -> Option<(f64, u64)> {
    use std::process::Command;

    log_eprintln!(
        "⚠️  [Duration Fallback] Using ImageMagick identify for animation duration: {}",
        path.display()
    );

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
        .ok();

    let output = match output {
        Some(output) => output,
        None => {
            log_eprintln!(
                "⚠️  [Duration Fallback] Failed to spawn ImageMagick identify for {}",
                path.display()
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log_eprintln!(
            "⚠️  [Duration Fallback] ImageMagick identify failed for {}: {}",
            path.display(),
            stderr.trim()
        );
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
        log_eprintln!(
            "⚠️  [Duration Fallback] ImageMagick identify returned 0 frames for {}",
            path.display()
        );
        return None;
    }

    let duration_secs = total_cs as f64 / 100.0;
    log_eprintln!(
        "📊  [Duration Fallback] ImageMagick animation detected: {} frames, {:.2}s ({})",
        frame_count,
        duration_secs,
        path.display()
    );
    Some((duration_secs, frame_count as u64))
}

fn try_imagemagick_identify(path: &Path) -> Option<f32> {
    if let Some((duration_secs, frame_count)) = get_animation_duration_and_frames_imagemagick(path)
    {
        let msg = format!(
            "📊 ImageMagick: animation detected ({} frames, {:.2}s)",
            frame_count, duration_secs
        );
        crate::progress_mode::emit_stderr(&msg);
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
            "--",
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

/// Determines if the image is stored in a lossless way for conversion decisions.
/// Uses image_detection::detect_compression for PNG, TIFF, WebP, AVIF (and HEIC/JXL in their own analyzers).
fn detect_lossless(format: &ImageFormat, path: &Path) -> Result<bool> {
    use crate::image_detection::{
        detect_compression, detect_format_from_bytes, CompressionType, DetectedFormat,
    };

    match format {
        ImageFormat::Png => {
            let detected_format = detect_format_from_bytes(path)?;
            let compression = detect_compression(&detected_format, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        // GIF uses palette quantization — inherently lossless for its own 256-color space.
        // Returning true preserves the palette exactly in JXL/AVIF lossless modes.
        ImageFormat::Gif => Ok(true),
        ImageFormat::Tiff => {
            let compression = detect_compression(&DetectedFormat::TIFF, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        ImageFormat::Jpeg => Ok(false),
        ImageFormat::WebP => check_webp_lossless(path),
        ImageFormat::Avif => {
            let compression = detect_compression(&DetectedFormat::AVIF, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        // BMP, ICO, Pnm, Tga, Hdr, Farbfeld, OpenExr are all uncompressed/lossless pixel formats.
        // DDS/Qoi can be either; treat conservatively as lossless to avoid lossy re-encoding.
        ImageFormat::Bmp
        | ImageFormat::Ico
        | ImageFormat::Pnm
        | ImageFormat::Tga
        | ImageFormat::Hdr
        | ImageFormat::Farbfeld
        | ImageFormat::OpenExr
        | ImageFormat::Dds
        | ImageFormat::Qoi => Ok(true),
        // Any unknown future format: be conservative — don't assume lossless.
        _ => Ok(false),
    }
}

fn check_webp_lossless(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    Ok(crate::image_formats::webp::is_lossless_from_bytes(&bytes))
}

/// Pixel-level fallback for is_lossless when format-level detection returns Err or is unavailable.
/// Decodes the image and logs classification metrics as a diagnostic warning.
/// Does **not** attempt routing decisions anymore; returns false conservatively when used.
fn pixel_fallback_lossless(path: &Path) -> bool {
    log_eprintln!(
        "⚠️  [Lossless Fallback] Format-level detection failed; using pixel-level heuristic for {}",
        path.display()
    );

    match crate::image_quality_detector::analyze_image_quality_from_path(path) {
        Some(analysis) => {
            log_eprintln!(
                "   📊 Fallback analysis: content_type={} complexity={:.3} edge_density={:.3} color_diversity={:.3}",
                analysis.content_type.name,
                analysis.complexity,
                analysis.edge_density,
                analysis.color_diversity,
            );
            // 保守策略：不再根据旧 RoutingDecision 决定是否视为无损，统一视为有损
            false
        }
        None => {
            log_eprintln!(
                "⚠️  [Lossless Fallback] Pixel-level analysis failed for {}; treating as lossy",
                path.display()
            );
            false
        }
    }
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
    use crate::image_detection::{detect_animation, DetectedFormat};
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
            log_eprintln!("⚠️  Cannot get JXL file dimensions: both jxlinfo and ffprobe unavailable");
            log_eprintln!("   💡 Suggestion: install jxlinfo: brew install jpeg-xl");
            (0, 0, false, 8)
        }
    };

    let metadata = extract_metadata(path)?;

    let is_lossless = crate::image_detection::detect_compression(
        &crate::image_detection::DetectedFormat::JXL,
        path,
    )
    .map(|c| c == crate::image_detection::CompressionType::Lossless)
    .unwrap_or_else(|_| pixel_fallback_lossless(path));

    // Extract HDR metadata using ffprobe
    let hdr_info = extract_hdr_info(path);

    // Detect animation via ffprobe/jxlinfo
    let (is_animated, _frame_count, _fps) = detect_animation(path, &DetectedFormat::JXL)
        .unwrap_or((false, 1, None));
    let duration_secs = if is_animated {
        get_animation_duration(path)
    } else {
        None
    };

    Ok(ImageAnalysis {
        file_path: path.display().to_string(),
        format: "JXL".to_string(),
        width,
        height,
        file_size,
        color_depth,
        color_space: "sRGB".to_string(),
        has_alpha,
        is_animated,
        duration_secs,
        is_lossless,
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
        hdr_info,
        precision: if let Ok(d) = detect_image(path) { d.precision } else { PrecisionMetadata::default() },
        history: crate::common_utils::get_current_history(),
        perception: Default::default(),
    })
}

fn analyze_avif_image(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    use crate::image_detection::{detect_animation, detect_compression, CompressionType, DetectedFormat};

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
    } else if let Ok(img) = crate::image_detection::open_image_with_limits(path) {
        let (w, h) = img.dimensions();
        (w, h, has_alpha_channel(&img), detect_color_depth(&img))
    } else {
        (0u32, 0u32, false, 8u8)
    };

    let is_lossless = match detect_compression(&DetectedFormat::AVIF, path) {
        Ok(ct) => ct == CompressionType::Lossless,
        Err(_) => pixel_fallback_lossless(path),
    };

    // Extract HDR metadata using ffprobe
    let hdr_info = extract_hdr_info(path);

    // Detect animation via ISOBMFF ftyp brand (avis/msf1)
    let (is_animated, _frame_count, _fps) = detect_animation(path, &DetectedFormat::AVIF)
        .unwrap_or((false, 1, None));
    let duration_secs = if is_animated {
        get_animation_duration(path)
    } else {
        None
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
        is_animated,
        duration_secs,
        is_lossless,
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
        hdr_info,
        precision: if let Ok(d) = detect_image(path) { d.precision } else { PrecisionMetadata::default() },
        history: crate::common_utils::get_current_history(),
        perception: Default::default(),
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

/// Extract HDR metadata from image using ffprobe.
/// Returns None if ffprobe fails or image is SDR.
fn extract_hdr_info(path: &Path) -> Option<ColorInfo> {
    let color_info = crate::ffprobe_json::extract_color_info(path);

    // Only return HDR info if it's actually HDR or has meaningful metadata
    if color_info.is_hdr()
        || color_info.bit_depth.is_some_and(|d| d > 8)
        || matches!(color_info.color_primaries.as_deref(), Some("bt2020"))
    {
        Some(color_info)
    } else {
        None
    }
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
