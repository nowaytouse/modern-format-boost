//! Detection API Module
//!
//! Pure analysis layer - detects image properties without trusting file
//! extensions. Uses magic bytes and actual file content for accurate format
//! detection.
//!
//! Enhanced PNG Quantization Detection
//!
//! PNG quantization detection is challenging because PNG format doesn't record
//! whether it was quantized. We use a multi-factor referee system:
//!
//! 1. **Structural Analysis**: IHDR color type, bit depth, PLTE/tRNS chunks
//! 2. **Metadata Analysis**: tEXt/iTXt chunks for tool signatures
//! 3. **Statistical Analysis**: Color distribution, gradient smoothness,
//!    dithering patterns
//! 4. **Heuristic Analysis**: File size vs dimensions ratio, compression
//!    efficiency
//!
//! Each factor contributes a weighted score, and the final decision is based on
//! the aggregate score with confidence level.
//!
//! ## Reliability and limitations
//!
//! - **PNG "lossy"** here means *palette-quantized* (e.g. pngquant, `TinyPNG`).
//!   16-bit and truecolor PNG without tool signature are treated as lossless.
//!   Indexed PNG uses a **conservative threshold
//!   (`DETECTION_LOSSY_THRESHOLD`)**: only scores ≥ threshold are marked lossy;
//!   gray zone [`DETECTION_LOSSLESS_THRESHOLD`, `DETECTION_LOSSY_THRESHOLD`] is
//!   treated as lossless to reduce false positives (e.g. natural palette art).
//!   Heuristic score includes **palette-index frequency entropy** for indexed
//!   images and **per-channel RGB entropy** for others. Tool signatures include
//!   zTXt decompression. We do *not* detect "PNG exported from a lossy source"
//!   (e.g. JPEG→PNG screenshot).
//! - **WebP**: VP8L vs VP8 chunk; animated WebP traverses all ANMF frames (any
//!   VP8→lossy).
//! - **TIFF**: Compression tag (259) across ALL IFDs; JPEG (6,7)→lossy,
//!   others→lossless. Supports both standard TIFF and `BigTIFF` (0x002B). No
//!   tag → assumed lossless.
//! - **AVIF**: every av1C record must prove 4:2:0/4:2:2 before a lossy verdict;
//!   4:4:4/monochrome remains unknown because pixel format does not prove AV1
//!   quantization state. Err when required structure is missing/malformed.
//! - **HEIC**: every hvcC record must prove 4:2:0/4:2:2, or a sole RExt/SCC
//!   record must prove PPS bypass disabled. Monochrome/4:4:4 with bypass
//!   permission remains unknown. Err when hvcC is malformed.
//! - **JXL**: Container jbrd box→JPEG-reconstruction semantics; VarDCT/XYB
//!   and jxlinfo-confirmed Modular lossy→lossy; hedged "possibly lossless"
//!   Modular streams remain unknown.
//! - **JPEG**: Always lossy; JXL transcoding does not require quality judgment.
//! - **EXR**: Parses compression attribute (NONE/RLE/ZIPS/ZIP/PIZ→lossless;
//!   PXR24/B44/B44A/DWAA/DWAB→lossy).
//! - **QOI, FLIF, PNM**: Treated as lossless. **JP2**: resolves main COD/COC
//!   plus first-tile overrides; an effective 9/7 component proves lossy, while
//!   all-reversible or incomplete evidence remains unknown/fails closed.
//! - **ICO**: Parses directory entries; embedded PNG checked for quantization
//!   (tRNS + indexed, tool signatures). BMP/DIB entries → lossless.
//! - **TGA, PSD, DDS**: Treated as lossless.
//! - **Format detection**: `mif1`/`msf1` major brand scans `compatible_brands`
//!   to disambiguate AVIF vs HEIC.
//!
//! ## Quality judgment reliability audit (conclusion)
//!
//! **Overall**: Format-by-format parsing + multi-dimension container/codestream
//! logic; Err only when key boxes/headers are missing (AVIF/HEIC/JXL). PNG uses
//! a scored heuristic with conservative gray zone; no format silently "guesses"
//! lossy when uncertain — either deterministic or Err.
//!
//! | Format | Reliability | Deterministic? | When uncertain |
//! |--------|-------------|----------------|----------------|
//! | PNG    | Medium–High | No (score)     | Gray zone [0.40,0.58] → lossless; palette-index entropy + zTXt signatures. |
//! | WebP   | High        | Yes (VP8L/VP8)| Animated: traverses all ANMF frames. |
//! | TIFF   | High        | Yes (tag 259) | All IFDs + `BigTIFF`. No tag → lossless. |
//! | JPEG   | N/A         | Yes (always)  | Always lossy. |
//! | AVIF   | High        | Multi (av1C)  | 4:4:4 without coded-lossless proof → Unknown. |
//! | HEIC   | High        | Multi (hvcC/PPS) | PPS bypass permission without all-CU proof → Unknown. |
//! | JXL    | High        | Multi (jbrd/VarDCT/jxlinfo)| Modular "possibly lossless" → Unknown. |
//! | GIF    | Assumed     | N/A           | Treated as lossless. |
//! | EXR    | High        | Yes (attr)    | Parses compression attr. No attr → lossless. |
//! | QOI/FLIF/PNM | Assumed | N/A        | Treated as lossless. |
//! | JP2    | High        | Positive (effective first-tile COD/COC)| Reversible/incomplete evidence → Unknown or Err. |
//! | ICO    | Medium      | Partial       | Embedded PNG checked for quantization. |
//! | TGA/PSD/DDS | Assumed | N/A         | Treated as lossless. |
//!
//! **Call chain**: `analyze_image` → format (HEIC/JXL/AVIF/…) →
//! `detect_lossless` / `detect_compression` → `Result<CompressionType>`.\
//! **Error propagation**: AVIF/HEIC/JXL `Err` propagates via `?` in
//! `analyze_heic_image`, `analyze_jxl_image`, and `detect_lossless`; conversion
//! path fails loudly with path in message.

use crate::Rational;
use crate::io_utils::ByteSliceExt;
use crate::tool_builders::JxlinfoBuilder;
use crate::tooling::builder_base::ToolBuilder;
use crate::unified_error::{ImgQualityError, Result};
use crate::{DjxlBuilder, FfmpegBuilder, FfprobeBuilder};
use image::{DynamicImage, GenericImageView, ImageReaderOptions, Rgba};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Output;
use std::time::Duration;

/// Open an image with relaxed memory limits to handle very large JPEGs.
///
/// Increases `max_alloc` from default ~512MB to 2GB for legitimate large
/// images. Still protects against malicious images (2GB is reasonable for
/// 100MP+ images). Open image with security limits (max dimensions).
///
/// # Errors
/// Returns an error if the image exceeds limits or is corrupted.
pub fn open_image_with_limits(path: &Path) -> Result<DynamicImage> {
    let (img, format) = decode_image_with_limits(path)?;

    // PNG heuristic detection (opt-in via MFB_ENABLE_PNG_HEURISTIC).
    // Kept out of the core decoder to avoid recursive analysis when PNG
    // heuristics need decoded pixels themselves.
    if format == Some(image::ImageFormat::Png) && super::png_validation::png_heuristic_enabled() {
        match analyze_png_quantization(path) {
            Ok(analysis) => {
                tracing::debug!(
                    is_quantized = analysis.is_quantized,
                    confidence = ?analysis.confidence,
                    detected_tool = ?analysis.detected_tool,
                    "PNG heuristic analysis completed"
                );
            }
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "png_heuristic_analysis_failed",
                    path,
                    format!("PNG heuristic analysis failed after decode: {err}"),
                );
            }
        }
    }

    Ok(img)
}

fn open_image_with_limits_without_png_heuristic(path: &Path) -> Result<DynamicImage> {
    decode_image_with_limits(path).map(|(img, _)| img)
}

fn decode_image_with_limits(path: &Path) -> Result<(DynamicImage, Option<image::ImageFormat>)> {
    use image::Limits;
    let mut limits = Limits::default();
    limits.max_alloc = Some(crate::constants::MAX_IMAGE_DECODE_ALLOC_BYTES);

    // Use magic bytes detection instead of relying on file extension
    // This handles cases like .jpe, missing extensions, or incorrect extensions
    let format = match infer::get_from_path(path) {
        Ok(Some(kind)) => match kind.mime_type() {
            // Standard formats supported by image crate
            "image/jpeg" => Some(image::ImageFormat::Jpeg),
            "image/png" => Some(image::ImageFormat::Png),
            "image/gif" => Some(image::ImageFormat::Gif),
            "image/webp" => Some(image::ImageFormat::WebP),
            "image/tiff" => Some(image::ImageFormat::Tiff),
            "image/bmp" => Some(image::ImageFormat::Bmp),
            "image/x-icon" => Some(image::ImageFormat::Ico),
            // Modern formats (if image crate supports them)
            "image/avif" => Some(image::ImageFormat::Avif),
            // Note: HEIC/HEIF, JXL, OpenEXR, JPEG 2000, PSD, etc. are handled separately
            _ => None, // Fall back to extension-based detection for unsupported formats
        },
        _ => None, // Fall back to extension-based detection
    };

    let mut reader = ImageReaderOptions::open(path)?;
    reader.limits(limits);
    if let Some(detected_format) = format {
        reader.set_format(detected_format);
    }
    let img = reader.decode().map_err(ImgQualityError::from)?;

    // Use detected format from magic bytes
    Ok((img, format))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageType {
    Static,
    Animated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    Lossless,
    Lossy,
    /// Structure parsed, but the codec evidence available to this project
    /// cannot prove either lossless or lossy semantics. `Unknown` is a
    /// third state: it must never be consumed as `Lossy` or `Lossless`.
    /// Admission to lossy-re-encode or lossless/copy routes requires
    /// positive evidence; unproven compression stays fail-closed.
    Unknown,
    /// JPEG XL carrying a `jbrd` JPEG bitstream reconstruction box: reversible
    /// to the exact original JPEG. Its own semantics — not plain `Lossless`
    /// (the reconstructed JPEG is usually lossy) and never `Lossy` as a
    /// source claim (the transcode itself is bit-exact).
    JpegReconstruction,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrecisionMetadata {
    pub bit_depth: Option<u8>,
    pub palette_size: Option<usize>,
    pub color_type: Option<u8>, // Format-specific (e.g. PNG color type)
    pub is_lossless_deterministic: bool,
    pub quality_estimate: Option<u8>,
    pub chroma_subsampling: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PngQuantizationAnalysis {
    pub is_quantized: bool,

    pub confidence: Option<f64>,

    pub quality_estimate: Option<u8>,

    pub factor_scores: PngQuantizationFactors,

    pub detected_tool: Option<String>,

    pub explanation: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PngQuantizationFactors {
    pub indexed_with_alpha: f64,

    pub large_palette: f64,

    pub tool_signature: f64,

    pub dithering_detected: f64,

    pub color_count_anomaly: f64,

    pub gradient_banding: f64,

    pub size_efficiency_anomaly: f64,

    pub entropy_anomaly: f64,

    pub color_frequency_distribution: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectedFormat {
    PNG,
    JPEG,
    GIF,
    WebP,
    HEIC,
    HEIF,
    AVIF,
    JXL,
    TIFF,
    BMP,
    // Additional formats — "can not use, but can't not have"
    QOI,
    JP2,
    ICO,
    TGA,
    EXR,
    FLIF,
    PSD,
    PNM,
    DDS,
    // Video Formats (detected during image scanning to avoid false positives)
    MP4,
    MOV,
    MKV,
    WEBM,
    Unknown(String),
}

impl DetectedFormat {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::PNG => "PNG",
            Self::JPEG => "JPEG",
            Self::GIF => "GIF",
            Self::WebP => "WebP",
            Self::HEIC => "HEIC",
            Self::HEIF => "HEIF",
            Self::AVIF => "AVIF",
            Self::JXL => "JXL",
            Self::TIFF => "TIFF",
            Self::BMP => "BMP",
            Self::QOI => "QOI",
            Self::JP2 => "JP2",
            Self::ICO => "ICO",
            Self::TGA => "TGA",
            Self::EXR => "EXR",
            Self::FLIF => "FLIF",
            Self::PSD => "PSD",
            Self::PNM => "PNM",
            Self::DDS => "DDS",
            Self::MP4 => "MP4",
            Self::MOV => "MOV",
            Self::MKV => "MKV",
            Self::WEBM => "WebM",
            Self::Unknown(s) => s,
        }
    }

    #[must_use]
    pub const fn is_modern_format(&self) -> bool {
        matches!(
            self,
            Self::WebP | Self::HEIC | Self::HEIF | Self::AVIF | Self::JXL | Self::JP2
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    pub file_path: String,

    pub format: DetectedFormat,

    pub image_type: ImageType,

    pub compression: CompressionType,

    pub width: u32,
    pub height: u32,

    pub bit_depth: Option<u8>,

    pub has_alpha: bool,

    pub file_size: u64,
    pub frame_count: Option<u32>,
    pub fps: Option<f32>,

    pub duration: Option<f32>,

    pub estimated_quality: Option<u8>,

    pub entropy: Option<f64>,

    pub precision: PrecisionMetadata,
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
/// Detect image format by inspecting magic bytes.
///
/// # Errors
/// Returns an error if the file cannot be read or the format is unknown.
pub fn detect_format_from_bytes(path: &Path) -> Result<DetectedFormat> {
    use crate::image::format_detect::FormatKind;

    Ok(
        match crate::image::format_detect::detect_true_format(path)? {
            FormatKind::Jpeg => DetectedFormat::JPEG,
            FormatKind::Png => DetectedFormat::PNG,
            FormatKind::Heic => DetectedFormat::HEIC,
            FormatKind::Heif => DetectedFormat::HEIF,
            FormatKind::Avif => DetectedFormat::AVIF,
            FormatKind::WebP => DetectedFormat::WebP,
            FormatKind::Gif => DetectedFormat::GIF,
            FormatKind::Bmp => DetectedFormat::BMP,
            FormatKind::Jxl => DetectedFormat::JXL,
            FormatKind::Tiff => DetectedFormat::TIFF,
            FormatKind::Qoi => DetectedFormat::QOI,
            FormatKind::Jp2 => DetectedFormat::JP2,
            FormatKind::Ico => DetectedFormat::ICO,
            FormatKind::Exr => DetectedFormat::EXR,
            FormatKind::Flif => DetectedFormat::FLIF,
            FormatKind::Psd => DetectedFormat::PSD,
            FormatKind::Pnm => DetectedFormat::PNM,
            FormatKind::Dds => DetectedFormat::DDS,
            FormatKind::Mp4 => DetectedFormat::MP4,
            FormatKind::Mov => DetectedFormat::MOV,
            FormatKind::Mkv => DetectedFormat::MKV,
            FormatKind::Webm => DetectedFormat::WEBM,
            FormatKind::Unknown => DetectedFormat::Unknown("Unknown format".to_string()),
        },
    )
}

/// Extract FPS from AVIF using ffprobe fallback.
///
/// libavif does not provide timing information, so we use ffprobe to extract
/// frame rate for animated AVIF files.
fn extract_fps_from_ffprobe(path: &Path) -> Option<f32> {
    if !crate::ffmpeg_builder::FfprobeBuilder::check_available() {
        tracing::debug!("ffprobe not available for AVIF FPS extraction");
        return None;
    }

    let mut cmd = FfprobeBuilder::new()
        .input(path)
        .show_streams()
        .show_format()
        .print_format("json")
        .loglevel("error")
        .build();

    let output = match crate::process_runner::ManagedProcess::spawn(&mut cmd) {
        Ok(proc) => {
            match proc.wait_liveness_timeout(
                std::time::Duration::from_secs(30),
                crate::process_runner::animated_image_process_hard_timeout(),
                "ffprobe FPS extraction",
            ) {
                Ok(out) => out,
                Err(err) => {
                    tracing::debug!("ffprobe process wait timeout error: {err}");
                    return None;
                }
            }
        }
        Err(err) => {
            tracing::debug!("ffprobe process spawn failed: {err}");
            return None;
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!("ffprobe output parse failed: {err}");
            return None;
        }
    };
    let Some(streams) = json.get("streams").and_then(|s| s.as_array()) else {
        return None;
    };

    for stream in streams {
        if stream.get("codec_type").and_then(|v| v.as_str()) == Some("video") {
            let fps_str = match stream.get("r_frame_rate").and_then(|v| v.as_str()) {
                Some(s) => Some(s),
                None => stream.get("avg_frame_rate").and_then(|v| v.as_str()),
            };
            let Some(fps_str) = fps_str else {
                continue;
            };

            // Parse fps from "num/den" format
            let parts: Vec<&str> = fps_str.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = match parts[0].parse() {
                    Ok(n) => n,
                    Err(err) => {
                        tracing::debug!("Failed to parse numerator for FPS: {err}");
                        continue;
                    }
                };
                let den: f64 = match parts[1].parse() {
                    Ok(d) => d,
                    Err(err) => {
                        tracing::debug!("Failed to parse denominator for FPS: {err}");
                        continue;
                    }
                };
                if den > 0.0 && num > 0.0 {
                    let fps = num / den;
                    if fps.is_finite() && fps > 0.0 {
                        return Some(crate::numeric_cast::f64_to_f32_lossy(fps));
                    }
                }
            }
        }
    }

    None
}

pub fn detect_animation(
    path: &Path,
    format: &DetectedFormat,
) -> Result<(bool, Option<u32>, Option<f32>)> {
    // 🚀 Stage 1: Native Fast-Path for Simple Formats
    // GIF, WebP, and PNG have simple, deterministic byte-level frame structures.
    // We can rely on our native parsers for these to save the ffprobe overhead.
    match format {
        DetectedFormat::GIF => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let frame_count = match crate::image_formats::gif::count_frames_from_bytes(&data) {
                Ok(frame_count) => frame_count,
                Err(error) => {
                    if crate::image_formats::gif::is_animated_from_bytes(&data)? {
                        tracing::warn!(
                            target: "gif_animation_probe",
                            path = %path.display(),
                            error = %error,
                            "GIF has at least two decoded frames despite later corruption; exact frame count and timing are unavailable"
                        );
                        return Ok((true, None, None));
                    }
                    return Err(error);
                }
            };
            let fps = if frame_count > 1 {
                crate::image_formats::gif::timing_stats_from_bytes(&data)?
                    .filter(|stats| stats.fps.is_finite() && stats.fps > 0.0_f64)
                    .map(|stats| crate::numeric_cast::f64_to_f32_lossy(stats.fps))
            } else {
                None
            };
            return Ok((frame_count > 1, Some(frame_count), fps));
        }
        DetectedFormat::WebP => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let is_animated = crate::image_formats::webp::is_animated_from_bytes(&data);
            let frame_count = if is_animated {
                Some(crate::image_formats::webp::count_frames_from_bytes(&data)?)
            } else {
                None
            };
            let fps = if is_animated {
                crate::image_formats::webp::timing_stats_from_bytes(&data)?
                    .filter(|stats| stats.fps.is_finite() && stats.fps > 0.0_f64)
                    .map(|stats| crate::numeric_cast::f64_to_f32_lossy(stats.fps))
            } else {
                None
            };
            return Ok((is_animated, frame_count, fps));
        }
        DetectedFormat::PNG => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            let info = crate::image::png_validation::parse_apng_animation(&data)?;
            let frame_count = info.map(|info| info.frame_count);
            let is_animated = frame_count.is_some_and(|count| count > 1);
            let fps = if is_animated {
                apng_timing_stats_from_bytes(&data)
                    .filter(|stats| stats.fps.is_finite() && stats.fps > 0.0_f64)
                    .map(|stats| crate::numeric_cast::f64_to_f32_lossy(stats.fps))
            } else {
                None
            };
            return Ok((is_animated, frame_count, fps));
        }
        DetectedFormat::TIFF => {
            // TIFF does not support animation (no multi-frame sequence standard).
            // Immediately declare static without touching ffprobe.
            return Ok((false, None, None));
        }
        DetectedFormat::AVIF => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

            if is_isobmff_animated_sequence(path)? {
                let fps = extract_fps_from_ffprobe(path);
                return Ok((true, None, fps));
            }

            // Sequence brands absent is necessary but not sufficient: verify
            // the item structure with the authoritative libheif reader (same
            // strategy as HEIC below) instead of declaring static from brands
            // alone. Do not fabricate frame_count=1 (M248).
            let data = std::fs::read(path)?;
            match crate::image_heic_analysis::read_heif_context_with_project_limits(&data) {
                Ok(ctx) => {
                    let ids = ctx.image_ids();
                    if ids.len() > 1 {
                        let fc =
                            crate::numeric_cast::usize_to_u32_strict(ids.len(), "avif_item_count");
                        return Ok((true, fc, None));
                    }
                    return Ok((false, None, None));
                }
                Err(err) => {
                    tracing::debug!(
                        target: "libheif_probe",
                        path = %path.display(),
                        error = %err,
                        "libheif-rs failed to read AVIF for animation detection; falling through to ffprobe"
                    );
                    // Fall through to Stage 2 (ffprobe) for ambiguous/malformed containers.
                }
            }
        }
        DetectedFormat::HEIC | DetectedFormat::HEIF => {
            // libheif-rs is the authoritative HEIC/HEIF library — use it
            // directly before falling back to ffprobe.
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            match crate::image_heic_analysis::read_heif_context_with_project_limits(&data) {
                Ok(ctx) => {
                    let ids = ctx.image_ids();
                    let count = ids.len();
                    if count > 1 {
                        // Multiple top-level image items → sequence/burst/live.
                        let fc = crate::numeric_cast::usize_to_u32_strict(count, "heif_item_count");
                        return Ok((true, fc, None));
                    }
                    // count == 0 or 1: single item → static image.
                    // Do NOT fabricate frame_count=1 (M248).
                    return Ok((false, None, None));
                }
                Err(err) => {
                    tracing::debug!(
                        target: "libheif_probe",
                        path = %path.display(),
                        error = %err,
                        "libheif-rs failed to read HEIF/HEIC for animation detection; falling through to ffprobe"
                    );
                    // Fall through to Stage 2 (ffprobe) for ambiguous/malformed containers.
                }
            }
        }
        DetectedFormat::JXL => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            match ::jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(&data)) {
                Ok(image) => {
                    let metadata = &image.image_header().metadata;
                    let is_animated = metadata.animation.is_some();

                    // Derive FPS from JXL animation header ticks-per-second.
                    // tps = tps_numerator / tps_denominator; fps = tps.
                    let oxide_fps: Option<f32> = if is_animated {
                        metadata.animation.as_ref().and_then(|anim| {
                            let num = f64::from(anim.tps_numerator);
                            let den = f64::from(anim.tps_denominator);
                            if den > 0.0 && num > 0.0 {
                                let fps = num / den;
                                if fps.is_finite() && fps > 0.0 {
                                    Some(crate::numeric_cast::f64_to_f32_lossy(fps))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    };

                    if !is_animated {
                        return Ok((false, None, None));
                    }

                    // For animated JXL: use jxlinfo to get precise frame count.
                    // jxl-oxide header alone cannot count frames without full decode.
                    let (frame_count, fps) = jxlinfo_refine_jxl_animation(path, oxide_fps);
                    return Ok((true, frame_count, fps));
                }
                Err(err) => {
                    use crate::ToolBuilder;
                    tracing::debug!(
                        target: "jxl_oxide_probe",
                        path = %path.display(),
                        error = %err,
                        "jxl-oxide failed to parse JXL for animation detection; treating as static"
                    );
                    // jxl-oxide failed; fall back to jxlinfo for full check.
                    if JxlinfoBuilder::new().check_available()
                        && let Some(is_anim) =
                            detect_jxl_animation_via_jxlinfo(path).unwrap_or(None)
                    {
                        let (frame_count, fps) = if is_anim {
                            jxlinfo_refine_jxl_animation(path, None)
                        } else {
                            (None, None)
                        };
                        return Ok((is_anim, frame_count, fps));
                    }
                    return Ok((false, None, None));
                }
            }
        }
        _ => {} // Fall through for ISOBMFF and unknown formats
    }

    if is_definitely_static_non_animated_format(format) {
        return Ok((false, None, None));
    }

    if is_definitely_animated_container(format) {
        return Ok((true, None, None));
    }

    // 🚀 Stage 2: libavformat / ffprobe for Complex Containers
    // Third-party libraries like libavformat have years of fuzzing, fixes, and
    // edge-case coverage for complex ISOBMFF containers (AVIF, HEIC).
    // Hand-written box-level parsing is prone to false positives (e.g., seeing
    // 'avis' brand but missing 'hdlr' or 'iloc' links) and false negatives. We
    // trust ffprobe natively here.
    let mut fps = None;
    if crate::ffprobe::is_ffprobe_available() {
        match crate::ffprobe::probe_video(path) {
            Ok(probe) => {
                let probe_frames = probe
                    .frame_count
                    .and_then(|c| crate::numeric_cast::u64_to_u32_strict(c, "probe_frames"));
                if let Some(r_fps) = probe.frame_rate
                    && r_fps > 0.0_f64
                {
                    fps = Some(crate::numeric_cast::f64_to_f32_lossy(r_fps));
                }

                if let Some(fc) = probe_frames {
                    if fc > 1 {
                        return Ok((true, Some(fc), fps));
                    }
                    // fc == 1: ffprobe may be probing a cover/thumbnail stream
                    // on multi-item ISOBMFF. Do not declare
                    // static here — fall through to packet count / ISOBMFF
                    // checks.
                } else if probe
                    .duration
                    .is_some_and(|d| d > crate::constants::VIDEO_NEGLIGIBLE_DURATION_SECS)
                    && probe.format_name.contains("video")
                {
                    return Ok((true, None, fps));
                }
            }
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "animation_ffprobe_probe_failed",
                    path,
                    format!("ffprobe animation probe failed before structural fallback: {err}"),
                );
            }
        }

        // If metadata probe fails to find frame count (common for AVIF sequences),
        // we explicitly count the packets. This demuxes the file and is 100% accurate.
        // Note: JXL is handled entirely in Stage 1 via jxl-oxide+jxlinfo and never
        //       reaches this point.
        if matches!(
            format,
            DetectedFormat::AVIF | DetectedFormat::HEIC | DetectedFormat::HEIF
        ) && let Some(explicit_count) = crate::ffprobe::get_frame_count(path)
        {
            if explicit_count > 1 {
                let final_count =
                    crate::numeric_cast::u64_to_u32_strict(explicit_count, "explicit_count");
                return Ok((true, final_count, fps));
            }
            if explicit_count == 1
                && matches!(format, DetectedFormat::AVIF)
                && !crate::ffprobe::isobmff_cover_stream_ambiguous(path)
            {
                let sequence = is_isobmff_animated_sequence(path).map_err(|e| {
                    ImgQualityError::AnalysisError(
                        crate::infra::static_logs::messages::MSG_IMAGE_DETECTION_ISOBMFF_ANIM_FAIL
                            .replacen("{}", &path.display().to_string(), 1)
                            .replacen("{}", &e.to_string(), 1),
                    )
                })?;
                if !sequence {
                    return Ok((false, Some(1), fps));
                }
                // `avis` / `msf1` sequence brands: do not treat demux fc==1 on
                // stub/cover as static.
            }
            // HEIC/HEIF/JXL/AVIF: single demuxed packet or fc==1 may still be
            // cover — continue.
        }
    }

    // 🛡️ Stage 3: Ultimate Fallback (if ffprobe is missing or fails entirely)
    let mut is_animated = false;
    let mut frame_count = None;

    match format {
        DetectedFormat::AVIF | DetectedFormat::HEIC | DetectedFormat::HEIF => {
            is_animated = is_isobmff_animated_sequence(path).map_err(|e| {
                ImgQualityError::AnalysisError(
                    crate::infra::static_logs::messages::MSG_IMAGE_DETECTION_ISOBMFF_ANIM_FAIL
                        .replacen("{}", &path.display().to_string(), 1)
                        .replacen("{}", &e.to_string(), 1),
                )
            })?;
            if !is_animated && !crate::ffprobe::isobmff_cover_stream_ambiguous(path) {
                frame_count = crate::ffprobe::get_frame_count(path)
                    .and_then(|c| crate::numeric_cast::u64_to_u32_strict(c, "heif_frame_count"));
            }
        }
        DetectedFormat::JXL => {
            is_animated = is_jxl_animated_via_ffprobe(path)?;
            if !is_animated && !crate::ffprobe::isobmff_cover_stream_ambiguous(path) {
                frame_count = crate::ffprobe::get_frame_count(path)
                    .and_then(|c| crate::numeric_cast::u64_to_u32_strict(c, "jxl_frame_count"));
            }
        }
        _ => {}
    }

    Ok((is_animated, frame_count, fps))
}

/// Positive proof that an animatable-capable file is a **true** single static
/// item (`img` may proceed).
///
/// Examples that return `true`: a GIF container with exactly one raster and no
/// animation control extensions; a static HEIC/AVIF after ISOBMFF + demux
/// checks with no cover/thumbnail stream conflict.
///
/// Returns `false` when animation is confirmed, frame count &gt; 1, or
/// multi-stream cover ambiguity remains.
///
/// # Errors
/// Returns an error when container verification cannot be completed.
pub fn animatable_format_confirmed_static_only(
    path: &Path,
    format: &DetectedFormat,
    detected_animated: bool,
    frame_count: Option<u32>,
) -> Result<bool> {
    if detected_animated || frame_count.is_some_and(|c| c > 1) {
        return Ok(false);
    }

    match format {
        DetectedFormat::WebP => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            Ok(!crate::image_formats::webp::is_animated_from_bytes(&data))
        }
        DetectedFormat::GIF => gif_confirmed_static_only(path),
        DetectedFormat::PNG => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;
            Ok(crate::image::png_validation::parse_apng_animation(&data)?
                .is_none_or(|info| info.frame_count <= 1))
        }
        DetectedFormat::AVIF | DetectedFormat::HEIC | DetectedFormat::HEIF => {
            isobmff_confirmed_static_only(path)
        }
        DetectedFormat::JXL => {
            // Prioritize authoritative native library (jxl-oxide) for animation detection
            let data = std::fs::read(path).map_err(|e| {
                ImgQualityError::AnalysisError(format!(
                    "Failed to read JXL file for oxide probe: {e}"
                ))
            })?;

            match ::jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(&data)) {
                Ok(image) => {
                    let is_anim = image.image_header().metadata.animation.is_some();
                    if is_anim {
                        return Ok(false);
                    }
                    return Ok(true);
                }
                Err(e) => {
                    // Log the parse failure but allow fallback to ffprobe
                    tracing::warn!(
                        target: "jxl_oxide_probe",
                        path = %path.display(),
                        error = %e,
                        "jxl-oxide parsing failed, falling back to ffprobe"
                    );
                }
            }

            // Fall back to external toolchain (jxlinfo/djxl) if native library fails
            if is_jxl_animated_via_ffprobe(path)? {
                return Ok(false);
            }
            if crate::ffprobe::isobmff_cover_stream_ambiguous(path) {
                return Ok(false);
            }
            if let Some(explicit) = crate::ffprobe::get_frame_count(path) {
                return Ok(explicit <= 1);
            }
            Ok(false)
        }
        // Formats with no animation capability in-spec (JP2, BMP, QOI, ICO,
        // TGA, EXR, FLIF, PSD, PNM, DDS, JPEG) are confirmed static by
        // definition — returning false here would silently bar them from
        // static-only admission paths (e.g. tier-2 modern lossy import).
        // Video containers and unknown bytes stay fail-closed at false.
        _ => Ok(is_definitely_static_non_animated_format(format)),
    }
}

/// GIF with exactly one image and no graphic-control extension → static still
/// on `img`.
fn gif_confirmed_static_only(path: &Path) -> Result<bool> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
    let data = std::fs::read(path)?;
    let count = crate::image_formats::gif::count_frames_from_bytes(&data)?;
    if count != 1 {
        return Ok(false);
    }
    if crate::image_formats::gif::timing_stats_from_bytes(&data)?
        .is_some_and(|stats| stats.frame_count > 1)
    {
        return Ok(false);
    }
    // Lone raster may still carry GIF89a Graphic Control Extension
    // (delay/disposal); multifaceted static proof is frame count == 1 plus
    // penetration, not GCE presence.
    if let crate::media_penetration::PenetrationResult::Verified(real) =
        crate::media_penetration::detect_real_frame_count(path, Some(1))
        && real > 1
    {
        return Ok(false);
    }
    Ok(true)
}

fn isobmff_confirmed_static_only(path: &Path) -> Result<bool> {
    if is_isobmff_animated_sequence(path)? {
        return Ok(false);
    }
    if crate::ffprobe::isobmff_cover_stream_ambiguous(path) {
        return Ok(false);
    }
    if let Some(explicit) = crate::ffprobe::get_frame_count(path) {
        return Ok(explicit <= 1);
    }
    Ok(false)
}

#[must_use]
const fn is_definitely_static_non_animated_format(format: &DetectedFormat) -> bool {
    matches!(
        format,
        DetectedFormat::JPEG
            | DetectedFormat::BMP
            | DetectedFormat::QOI
            | DetectedFormat::JP2
            | DetectedFormat::ICO
            | DetectedFormat::TGA
            | DetectedFormat::EXR
            | DetectedFormat::FLIF
            | DetectedFormat::PSD
            | DetectedFormat::PNM
            | DetectedFormat::DDS
    )
}

#[must_use]
const fn is_definitely_animated_container(format: &DetectedFormat) -> bool {
    matches!(
        format,
        DetectedFormat::MP4 | DetectedFormat::MOV | DetectedFormat::MKV | DetectedFormat::WEBM
    )
}

/// Parse GIF palette size using the `gif` crate decoder.
///
/// Uses the `gif` crate to reliably decode GIF frames and extract the maximum
/// palette size across GCT and all per-frame LCTs. This replaces the previous
/// hand-rolled block traversal which was fragile against malformed block sizes.
///
/// # Errors
/// Returns an error if the GIF file is invalid or cannot be read.
pub fn parse_gif_precision_metadata(path: &Path) -> Result<PrecisionMetadata> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;

    if !data.starts_with(b"GIF87a") && !data.starts_with(b"GIF89a") {
        return Err(ImgQualityError::AnalysisError(
            "Not a valid GIF file".to_string(),
        ));
    }

    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::Indexed);
    let mut decoder = options
        .read_info(data.as_slice())
        .map_err(|e| ImgQualityError::AnalysisError(format!("GIF decode failed: {e}")))?;

    // Global color table — present in almost all GIFs; each entry is 3 bytes (RGB).
    let gct_colors = decoder.global_palette().map(|p| p.len() / 3);
    let mut max_palette = gct_colors;

    // Walk all frames to find the largest local color table.
    while let Ok(Some(frame)) = decoder.read_next_frame() {
        if let Some(lp) = &frame.palette {
            let lct_colors = lp.len() / 3;
            let merged = match max_palette {
                Some(m) => m.max(lct_colors),
                None => lct_colors,
            };
            max_palette = Some(merged);
        }
    }

    Ok(PrecisionMetadata {
        bit_depth: Some(8),
        palette_size: max_palette,
        is_lossless_deterministic: true,
        ..Default::default()
    })
}

fn measured_bit_depth_for_format(path: &Path, format: &DetectedFormat) -> Option<u8> {
    match format {
        DetectedFormat::PNG => match std::fs::read(path) {
            Ok(data) => {
                let mut cursor = std::io::Cursor::new(&data);
                match parse_png_structure(&mut cursor) {
                    Ok(info) => Some(info.bit_depth),
                    Err(err) => {
                        crate::media_conversion_gate::probe_layer_audit(
                            "png_bit_depth_parse_failed",
                            path,
                            format!(
                                "PNG structure parse failed during bit-depth measurement: {err}"
                            ),
                        );
                        None
                    }
                }
            }
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "png_bit_depth_read_failed",
                    path,
                    format!("PNG read failed during bit-depth measurement: {err}"),
                );
                None
            }
        },
        DetectedFormat::JPEG => match crate::conversion::jpeg_precision_from_header(path) {
            Ok(Some(v)) => Some(v),
            Ok(None) => match crate::conversion::media_info_without_ffprobe(path) {
                Ok(info) => info.and_then(|info| info.bit_depth),
                Err(err) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "jpeg_bit_depth_media_info_probe_failed",
                        path,
                        format!("JPEG bit-depth fallback probe failed: {err}"),
                    );
                    None
                }
            },
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "jpeg_precision_probe_failed",
                    path,
                    format!("JPEG precision probe failed: {err}"),
                );
                None
            }
        },
        DetectedFormat::GIF | DetectedFormat::WebP | DetectedFormat::QOI => Some(8),
        DetectedFormat::AVIF
        | DetectedFormat::HEIC
        | DetectedFormat::HEIF
        | DetectedFormat::JXL
        | DetectedFormat::TIFF
        | DetectedFormat::BMP
        | DetectedFormat::ICO
        | DetectedFormat::TGA
        | DetectedFormat::EXR
        | DetectedFormat::PSD
        | DetectedFormat::DDS => match crate::conversion::media_info_without_ffprobe(path) {
            Ok(info) => info.and_then(|info| info.bit_depth),
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "image_bit_depth_media_info_probe_failed",
                    path,
                    format!("bit-depth fallback probe failed: {err}"),
                );
                None
            }
        },
        _ => None,
    }
}

/// Returns true if the ISOBMFF file (AVIF/HEIC/HEIF) is an image sequence
/// (animated). Checks `major_brand` and `compatible_brands` for known sequence
/// brand codes.
///
/// # Errors
/// Returns an error if the file cannot be read or ftyp box is malformed.
pub fn is_isobmff_animated_sequence(path: &Path) -> Result<bool> {
    // Sequence brands: avis=AVIF sequence, msf1=multi-sample ftyp (used by animated
    // HEIC/AVIF)
    use crate::constants::ISOBMFF_ANIMATED_BRANDS;

    let file = File::open(path)?;
    let mut header = Vec::new();
    std::io::Read::read_to_end(&mut file.take(4096), &mut header)?;
    let Some(ftyp_payload) = crate::common_utils::isobmff_ftyp_payload(&header) else {
        return Ok(false);
    };

    let major_brand = &ftyp_payload[0..4];
    for seq_brand in ISOBMFF_ANIMATED_BRANDS {
        if major_brand == *seq_brand {
            return Ok(true);
        }
    }

    let (compatible_brands, remainder) = ftyp_payload[8..].as_chunks::<4>();
    if !remainder.is_empty() {
        return Ok(false);
    }
    for cb in compatible_brands {
        for seq_brand in ISOBMFF_ANIMATED_BRANDS {
            if cb == *seq_brand {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Returns true if the JXL file contains animation.
/// JXL stores animation natively in its container; we use ffprobe to check
/// duration > 0.
fn detect_jxl_animation_via_jxlinfo(path: &Path) -> Result<Option<bool>> {
    use crate::ToolBuilder;
    if !JxlinfoBuilder::new().check_available() {
        return Ok(None);
    }

    let output = run_jxlinfo_bounded(path, "JXL animation detection")?;

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_jxlinfo_animation_hint(&combined);

    if !output.status.success() && parsed.is_none() {
        return Err(ImgQualityError::AnalysisError(format!(
            "jxlinfo did not complete successfully for {} during JXL animation detection",
            path.display()
        )));
    }

    Ok(parsed)
}

/// Query `jxlinfo` for precise frame count and FPS of an animated JXL.
/// Returns `(frame_count, fps)` — both may be `None` if jxlinfo is unavailable
/// or cannot parse the output. Never fabricates values.
fn jxlinfo_refine_jxl_animation(path: &Path, oxide_fps: Option<f32>) -> (Option<u32>, Option<f32>) {
    use crate::ToolBuilder;
    if !JxlinfoBuilder::new().check_available() {
        return (None, oxide_fps);
    }

    let output = match run_jxlinfo_bounded(path, "JXL frame-count refinement") {
        Ok(o) => o,
        Err(err) => {
            tracing::debug!(
                target: "jxlinfo_probe",
                path = %path.display(),
                error = %err,
                "jxlinfo invocation failed for frame count"
            );
            return (None, oxide_fps);
        }
    };

    if !output.status.success() {
        tracing::debug!(
            target: "jxlinfo_probe",
            path = %path.display(),
            "jxlinfo exited non-zero during frame count refinement"
        );
        return (None, oxide_fps);
    }

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let (frame_count, jxlinfo_fps) = parse_jxlinfo_full_info(&combined);
    let fps = jxlinfo_fps.or(oxide_fps);
    (frame_count, fps)
}

const JXLINFO_SOFT_TIMEOUT: Duration = Duration::from_mins(2);
const JXLINFO_HARD_TIMEOUT: Duration = Duration::from_mins(10);

fn run_jxlinfo_bounded(path: &Path, context: &str) -> Result<Output> {
    let mut command = JxlinfoBuilder::new().input(path).build();
    crate::process_runner::run_command_with_liveness_timeout(
        &mut command,
        JXLINFO_SOFT_TIMEOUT,
        JXLINFO_HARD_TIMEOUT,
        context,
    )
    .map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "{context} via jxlinfo failed for {}: {err}",
            path.display()
        ))
    })
}

fn parse_jxlinfo_compression_hint(output: &str) -> Option<CompressionType> {
    output.lines().find_map(|line| {
        let line = line.trim().to_ascii_lowercase();
        if line.starts_with("jpeg xl image") && line.contains(", lossy") {
            Some(CompressionType::Lossy)
        } else {
            // jxlinfo deliberately says "(possibly) lossless" for streams it
            // cannot prove lossless. Never upgrade that hedge to a verdict.
            None
        }
    })
}

/// Parse `jxlinfo` stdout/stderr for frame count and FPS.
/// Recognises lines like:
///   `Number of frames: 42`
///   `Animation: 100ms per frame (10.00 fps)`
fn parse_jxlinfo_full_info(output: &str) -> (Option<u32>, Option<f32>) {
    let mut frame_count: Option<u32> = None;
    let mut fps: Option<f32> = None;

    for line in output.lines() {
        let lower = line.to_lowercase();

        // "Number of frames: 42" or "num_frames: 42" or "frames: 42"
        if frame_count.is_none() {
            for prefix in &["number of frames:", "num_frames:", "frames:"] {
                if let Some((_, rest)) = lower.split_once(prefix) {
                    if let Some(token) = rest.split_whitespace().next() {
                        frame_count =
                            crate::numeric_cast::parse_strict::<u32>(token, "jxlinfo_num_frames");
                    }
                    break;
                }
            }
        }

        // "Animation: 100ms per frame (10.00 fps)"
        if fps.is_none()
            && let Some(pos) = lower.find(" fps)")
            && let Some(lparen) = lower[..pos].rfind('(')
        {
            let token = lower[lparen + 1..pos].trim();
            fps = crate::numeric_cast::parse_strict::<f64>(token, "jxlinfo_fps")
                .map(crate::numeric_cast::f64_to_f32_lossy);
        }

        if frame_count.is_some() && fps.is_some() {
            break;
        }
    }

    (frame_count, fps)
}

fn parse_jxlinfo_animation_hint(output: &str) -> Option<bool> {
    let normalized = output.to_lowercase();

    for line in normalized.lines() {
        if let Some((_, value)) = line.split_once("have_animation:") {
            let token = value.split_whitespace().next()?;
            return match token {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            };
        }

        if let Some((_, value)) = line.split_once("animation length:") {
            let token = value.split_whitespace().next()?;
            let seconds =
                crate::numeric_cast::parse_strict::<f64>(token, "jxlinfo_animation_length")?;
            return Some(seconds > 0.0);
        }
    }

    if normalized
        .lines()
        .any(|line| line.starts_with("jpeg xl image"))
    {
        return Some(false);
    }

    if normalized.contains("decoder error") || normalized.contains("error reading file") {
        return Some(false);
    }

    None
}

/// Uses `jxlinfo` metadata when available, then falls back to `djxl ->
/// ffprobe`.
fn is_jxl_animated_via_ffprobe(path: &Path) -> Result<bool> {
    // FFmpeg's jpegxl_anim decoder is incomplete and cannot properly detect JXL
    // animation. We need to convert to APNG first, then check frame count.

    use crate::ToolBuilder;
    if let Some(is_animated) = detect_jxl_animation_via_jxlinfo(path)? {
        return Ok(is_animated);
    }

    if !DjxlBuilder::check_available() {
        crate::media_conversion_gate::probe_detection_recovery_audit(
            "jxl_animation_tools_missing",
            format!(
                "djxl/jxlinfo unavailable for {}; treating animation as unverified static (no \
                 fabricated frame count)",
                path.display()
            ),
        );
        return Ok(false);
    }

    // Create temporary APNG file
    let temp_apng = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "image_detection_jxl_apng",
        None,
        Some(".apng"),
    )
    .map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "Failed to allocate temporary APNG for JXL animation detection ({}): {}",
            path.display(),
            e
        ))
    })?;
    let temp_apng_path = temp_apng.path();

    // Convert JXL to APNG using djxl
    let djxl_output = DjxlBuilder::new()
        .input(path)
        .output(temp_apng_path)
        .build()
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "djxl failed while preparing JXL animation detection for {}: {}",
                path.display(),
                e
            ))
        })?;

    if !djxl_output.status.success() || !temp_apng_path.exists() {
        return Err(ImgQualityError::AnalysisError(format!(
            "djxl did not produce a probeable APNG for {} during JXL animation detection",
            path.display()
        )));
    }

    // Check frame count using ffprobe with -count_frames
    let output = FfprobeBuilder::new()
        .loglevel(crate::constants::FFMPEG_VAL_ERROR)
        .select_streams_custom("v:0")
        .count_frames()
        .show_entries("stream=nb_read_frames")
        .print_format(crate::constants::FFMPEG_VAL_JSON)
        .arg("--")
        .input(temp_apng_path)
        .build()
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "ffprobe failed while counting JXL animation frames for {}: {}",
                path.display(),
                e
            ))
        })?;
    if !output.status.success() {
        return Err(ImgQualityError::AnalysisError(format!(
            "ffprobe did not complete successfully while counting JXL animation frames for {}",
            path.display()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = serde_json::from_str::<serde_json::Value>(&stdout).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "Failed to parse ffprobe frame-count JSON for {} during JXL animation detection: {}",
            path.display(),
            e
        ))
    })?;
    let stream = json
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "ffprobe returned no streams while counting JXL animation frames for {}",
                path.display()
            ))
        })?;
    let nb_frames_str = stream
        .get("nb_read_frames")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "ffprobe omitted nb_read_frames while counting JXL animation frames for {}",
                path.display()
            ))
        })?;
    let nb_frames = crate::numeric_cast::parse_strict::<u32>(nb_frames_str, "jxl_nb_read_frames")
        .ok_or_else(|| {
        ImgQualityError::AnalysisError(format!(
            "Failed to parse nb_read_frames='{}' during JXL animation detection for {}",
            nb_frames_str,
            path.display()
        ))
    })?;

    Ok(nb_frames > 1)
}

/// Detect if an image is lossy or lossless based on its format and internal
/// structure.
///
/// # Errors
/// Returns an error if file access fails or format-specific analysis fails.
pub fn detect_compression(format: &DetectedFormat, path: &Path) -> Result<CompressionType> {
    match format {
        DetectedFormat::PNG => detect_png_compression(path),

        DetectedFormat::BMP
        | DetectedFormat::GIF
        | DetectedFormat::QOI
        | DetectedFormat::FLIF
        | DetectedFormat::PNM
        | DetectedFormat::TGA
        | DetectedFormat::PSD
        | DetectedFormat::DDS => Ok(CompressionType::Lossless),

        DetectedFormat::TIFF => detect_tiff_compression(path),

        DetectedFormat::WebP => {
            crate::common_utils::validate_file_size_limit(
                path,
                crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
            )
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
            let data = std::fs::read(path)?;

            if crate::image_formats::webp::is_animated_from_bytes(&data) {
                return detect_webp_animation_compression(&data);
            }

            let is_lossless = crate::image_formats::webp::is_lossless_from_bytes(&data)?;
            Ok(if is_lossless {
                CompressionType::Lossless
            } else {
                CompressionType::Lossy
            })
        }

        DetectedFormat::HEIC | DetectedFormat::HEIF => detect_heic_compression(path),

        DetectedFormat::AVIF => detect_avif_compression(path),

        DetectedFormat::JXL => detect_jxl_compression(path),

        DetectedFormat::ICO => detect_ico_compression(path),
        DetectedFormat::EXR => detect_exr_compression(path),
        DetectedFormat::JP2 => detect_jp2_compression(path),

        // Baseline/progressive JPEG is lossy by definition of the route that
        // reaches this probe; reversible-JPEG handling lives in the dedicated
        // JPEG analysis path, not here.
        DetectedFormat::JPEG => Ok(CompressionType::Lossy),

        // Video containers and unknown bytes are not still images; answering
        // "lossy" would fabricate a compression verdict that no caller of this
        // API is allowed to consume.
        DetectedFormat::MP4
        | DetectedFormat::MOV
        | DetectedFormat::MKV
        | DetectedFormat::WEBM
        | DetectedFormat::Unknown(_) => Err(ImgQualityError::AnalysisError(format!(
            "detect_compression requires a still-image format, got {format:?}: {}",
            path.display()
        ))),
    }
}

fn detect_png_compression(path: &Path) -> Result<CompressionType> {
    if !super::png_validation::png_heuristic_enabled() && super::png_validation::is_true_png(path)?
    {
        return Ok(CompressionType::Lossless);
    }

    let analysis = analyze_png_quantization(path)?;

    if std::env::var(crate::constants::ENV_VERBOSE).is_ok()
        || std::env::var(crate::constants::ENV_DEBUG).is_ok()
    {
        crate::progress_mode::emit_stderr(&format!(
            "   📊 PNG Analysis: {} (confidence: {:.1}%)\n      {}",
            if analysis.is_quantized {
                "Quantized/Lossy"
            } else {
                "Lossless"
            },
            crate::media_conversion_gate::ui_confidence_scale100_one_decimal_or_na(
                analysis.confidence,
                "png_analysis_confidence",
            ),
            analysis.explanation
        ));
    }

    Ok(if analysis.is_quantized {
        CompressionType::Lossy
    } else {
        CompressionType::Lossless
    })
}

/// Analyze PNG file for quantization artifacts and determine if it's lossy.
///
/// # Errors
/// Returns an error if the file is not a valid PNG or cannot be read.
/// Specifically, `ImgQualityError::IoError` if file operations fail, or
/// `ImgQualityError::AnalysisError` if the PNG structure is invalid or
/// corrupted.
pub fn analyze_png_quantization(path: &Path) -> Result<PngQuantizationAnalysis> {
    let file = std::fs::File::open(path).map_err(ImgQualityError::IoError)?;
    let mut reader = std::io::BufReader::new(file);
    analyze_png_quantization_from_reader(&mut reader, Some(path))
}

/// Analyze PNG quantization from raw bytes.
///
/// # Errors
/// Returns an error if the data is not a valid PNG.
pub fn analyze_png_quantization_from_bytes(data: &[u8]) -> Result<PngQuantizationAnalysis> {
    let mut cursor = std::io::Cursor::new(data);
    analyze_png_quantization_from_reader(&mut cursor, None)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PngImageScale {
    Small,
    Medium,
    Large,
}

struct PngPixelProfile {
    pixel_count: u64,
    scale: PngImageScale,
}

impl PngPixelProfile {
    fn new(width: u32, height: u32) -> Self {
        let pixel_count = u64::from(width) * u64::from(height);
        let scale = if pixel_count > crate::constants::IMAGE_SIZE_THRESHOLD_LARGE {
            PngImageScale::Large
        } else if pixel_count > crate::constants::IMAGE_SIZE_THRESHOLD_MEDIUM {
            PngImageScale::Medium
        } else {
            PngImageScale::Small
        };
        Self { pixel_count, scale }
    }

    const fn is_large(&self) -> bool {
        matches!(self.scale, PngImageScale::Large)
    }

    const fn is_medium_or_larger(&self) -> bool {
        !matches!(self.scale, PngImageScale::Small)
    }
}

struct PngScoreSummary {
    final_score: f64,
}

struct PngQuantizationSession {
    png_info: PngStructureInfo,
    path: Option<std::path::PathBuf>,
    factors: PngQuantizationFactors,
    detected_tool: Option<String>,
    explanations: Vec<String>,
}

impl PngQuantizationSession {
    fn new(png_info: PngStructureInfo, path: Option<&Path>) -> Self {
        Self {
            png_info,
            path: path.map(std::path::Path::to_path_buf),
            factors: PngQuantizationFactors::default(),
            detected_tool: None,
            explanations: Vec::new(),
        }
    }

    fn analyze<R: Read + Seek>(mut self, reader: &mut R) -> Result<PngQuantizationAnalysis> {
        self.apply_indexed_structure_signals()?;
        self.apply_tool_signature_signal();
        self.apply_indexed_pixel_signals()?;
        self.apply_compression_signal(reader);

        if let Some(result) = self.finish_tool_signature() {
            return Ok(result);
        }
        if let Some(result) = self.finish_lossless_16bit() {
            return Ok(result);
        }
        if let Some(result) = self.finish_truecolor_analysis()? {
            return Ok(result);
        }

        Ok(self.finish_scored_analysis())
    }

    fn pixel_profile(&self) -> PngPixelProfile {
        PngPixelProfile::new(self.png_info.width, self.png_info.height)
    }

    fn apply_indexed_structure_signals(&mut self) -> Result<()> {
        if self.png_info.color_type == 3 {
            let profile = self.pixel_profile();
            if self.png_info.has_trns {
                self.factors.indexed_with_alpha = crate::constants::PNG_ALPHA_INDEXED_FACTOR_HIGH;
                self.explanations
                    .push("Indexed PNG with alpha (tRNS) - definite quantization".to_string());
            } else if profile.is_large() {
                self.factors.indexed_with_alpha = crate::constants::PNG_ALPHA_INDEXED_FACTOR_MEDIUM;
                self.explanations.push(format!(
                    "Large indexed PNG ({}x{}) - likely quantized",
                    self.png_info.width, self.png_info.height
                ));
            } else {
                self.factors.indexed_with_alpha = if profile.is_medium_or_larger() {
                    crate::constants::PNG_ALPHA_INDEXED_FACTOR_LOW
                } else {
                    crate::constants::PNG_ALPHA_INDEXED_FACTOR_MIN
                };
            }
        }

        let Some(palette_size) = self.png_info.palette_size else {
            return Ok(());
        };
        let profile = self.pixel_profile();
        let colors_per_megapixel = Self::colors_per_megapixel(palette_size, profile.pixel_count)?;
        let palette_density = Self::palette_density(palette_size, profile.pixel_count)?;

        if palette_size > 240 {
            self.factors.large_palette = crate::constants::PNG_PALETTE_FACTOR_NEAR_MAX;
            self.explanations.push(format!(
                "Near-max palette ({palette_size} colors) - definitely quantized"
            ));
        } else if palette_size > 200 {
            self.factors.large_palette = crate::constants::PNG_PALETTE_FACTOR_LARGE;
            self.explanations.push(format!(
                "Large palette ({palette_size} colors) - likely quantized"
            ));
        } else if profile.is_large() && palette_size > 64 {
            self.factors.large_palette = crate::constants::PNG_PALETTE_FACTOR_MEDIUM;
            self.explanations.push(format!(
                "Large image ({}x{}) with limited palette ({} colors) - quantization indicator",
                self.png_info.width, self.png_info.height, palette_size
            ));
        } else if profile.is_large() && palette_size > 32 {
            self.factors.large_palette = crate::constants::PNG_PALETTE_FACTOR_SMALL;
            self.explanations.push(format!(
                "Large image with small palette ({palette_size} colors)"
            ));
        } else if profile.is_medium_or_larger() && palette_size > 128 {
            self.factors.large_palette = crate::constants::PNG_PALETTE_FACTOR_MIN;
        } else if palette_size <= 16 && palette_density > crate::constants::PNG_PALETTE_DENSITY_HIGH
        {
            self.factors.large_palette = 0.0_f64;
        } else if palette_size <= 32
            && palette_density > crate::constants::PNG_PALETTE_DENSITY_MEDIUM
        {
            self.factors.large_palette = crate::constants::PNG_PALETTE_SCORE_LOW;
        } else {
            self.factors.large_palette =
                if palette_density < crate::constants::PNG_PALETTE_DENSITY_HIGH {
                    crate::constants::PNG_PALETTE_SCORE_HIGH
                } else {
                    crate::constants::PNG_PALETTE_SCORE_MEDIUM
                };
        }

        if profile.is_large()
            && colors_per_megapixel < crate::constants::PNG_COLORS_PER_MP_THRESHOLD
        {
            self.factors.large_palette = self
                .factors
                .large_palette
                .max(crate::constants::PNG_PALETTE_SCORE_MEDIUM);
            if !self.explanations.iter().any(|e| e.contains("colors/MP")) {
                self.explanations.push(format!(
                    "Low color density ({colors_per_megapixel:.1} colors/MP)"
                ));
            }
        }

        Ok(())
    }

    fn colors_per_megapixel(palette_size: usize, pixel_count: u64) -> Result<f64> {
        let num = Rational::from(
            crate::numeric_cast::usize_to_u32_strict(
                palette_size,
                "palette_size for ratio component",
            )
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "PNG Analysis: 'palette_size' conversion failed".to_string(),
                )
            })?,
        );
        let den = Rational::from(
            crate::numeric_cast::u64_to_u32_strict(pixel_count, "pixel_count for ratio component")
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError(
                        "PNG Analysis: 'pixel_count' conversion failed".to_string(),
                    )
                })?,
        ) / Rational::from(1_000_000_i32);
        if den == 0 {
            Ok(1000.0_f64)
        } else {
            Ok((num / den).to_f64().min(1000.0_f64))
        }
    }

    fn palette_density(palette_size: usize, pixel_count: u64) -> Result<f64> {
        let num = Rational::from(
            crate::numeric_cast::usize_to_u32_strict(
                palette_size,
                "palette_size for ratio component",
            )
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "PNG Analysis: 'palette_size' conversion failed".to_string(),
                )
            })?,
        );
        let den_f = f64::from(
            crate::numeric_cast::u64_to_u32_strict(pixel_count, "pixel_count for ratio component")
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError(
                        "PNG Analysis: 'pixel_count' conversion failed".to_string(),
                    )
                })?,
        )
        .sqrt();

        let den = {
            #[cfg(feature = "high-precision")]
            {
                crate::numeric_cast::f64_to_rational_strict(den_f, "palette_density_denominator")
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "PNG Analysis: 'palette_density_denominator' conversion failed"
                                .to_string(),
                        )
                    })?
            }
            #[cfg(not(feature = "high-precision"))]
            {
                Rational::from_f64(den_f).ok_or_else(|| {
                    ImgQualityError::AnalysisError(
                        "PNG Analysis: 'den_f' is not a finite number".to_string(),
                    )
                })?
            }
        };
        Ok((num / den).to_f64())
    }

    fn apply_tool_signature_signal(&mut self) {
        if let Some(tool) = self.png_info.detected_tool.clone() {
            self.factors.tool_signature = 1.0_f64;
            self.detected_tool = Some(tool.clone());
            self.explanations
                .push(format!("Tool signature detected: {tool}"));
        }
    }

    fn apply_indexed_pixel_signals(&mut self) -> Result<()> {
        if self.png_info.color_type != 3 {
            return Ok(());
        }
        let Some(img) = self.open_analysis_image() else {
            return Ok(());
        };
        let profile = self.pixel_profile();

        let dithering_score = detect_dithering_pattern(&img)?;
        self.factors.dithering_detected = dithering_score;
        if dithering_score > crate::constants::PNG_DITHERING_THRESHOLD {
            self.explanations.push(format!(
                "Dithering pattern detected (score: {dithering_score:.2})"
            ));
        }

        self.apply_indexed_palette_usage_signals(&img, &profile)?;
        self.apply_indexed_banding_and_frequency_signals(&img)?;
        self.apply_indexed_entropy_signal(&img, &profile);

        Ok(())
    }

    fn open_analysis_image(&self) -> Option<image::DynamicImage> {
        let path = self.path.as_deref()?;
        match open_image_with_limits_without_png_heuristic(path) {
            Ok(img) => Some(img),
            Err(err) => {
                tracing::debug!(
                    target: "mfb.detection",
                    path = %path.display(),
                    %err,
                    "PNG pixel analysis: could not open image; skipping pixel-level signals"
                );
                None
            }
        }
    }

    fn apply_indexed_palette_usage_signals(
        &mut self,
        img: &image::DynamicImage,
        profile: &PngPixelProfile,
    ) -> Result<()> {
        let (unique_colors, _expected_colors) =
            analyze_color_distribution(img, self.png_info.palette_size)?;

        if let Some(palette_size) = self.png_info.palette_size {
            let usage_ratio = (Rational::from(crate::numeric_cast::usize_to_u64(unique_colors))
                / Rational::from(crate::numeric_cast::usize_to_u64(palette_size))
                    .max(Rational::from(1)))
            .to_f64();

            if profile.is_large() {
                if usage_ratio > crate::constants::PNG_USAGE_RATIO_HIGH {
                    self.factors.color_count_anomaly = crate::constants::PNG_ANOMALY_SCORE_CRITICAL;
                    self.explanations.push(format!(
                        "Large image using {:.0}% of {} color palette",
                        usage_ratio * crate::constants::SCALE_100,
                        palette_size
                    ));
                } else if usage_ratio > crate::constants::PNG_USAGE_RATIO_RELAXED {
                    self.factors.color_count_anomaly = crate::constants::PNG_ANOMALY_SCORE_MEDIUM;
                } else {
                    self.factors.color_count_anomaly = crate::constants::PNG_ANOMALY_SCORE_LOW;
                }
            } else if usage_ratio > crate::constants::PNG_USAGE_RATIO_CRITICAL && palette_size > 200
            {
                self.factors.color_count_anomaly = crate::constants::PNG_ANOMALY_SCORE_HIGH;
                self.explanations.push(format!(
                    "High palette utilization ({:.0}%)",
                    usage_ratio * crate::constants::SCALE_100
                ));
            } else if usage_ratio > crate::constants::PNG_USAGE_RATIO_MEDIUM && palette_size > 128 {
                self.factors.color_count_anomaly = crate::constants::PNG_ANOMALY_SCORE_LOW;
            }
        }

        let sampled_uniques = sample_unique_color_count(img, 10_000)?;
        if sampled_uniques > 0 && profile.is_large() {
            if sampled_uniques <= 256 {
                self.factors.color_count_anomaly = self
                    .factors
                    .color_count_anomaly
                    .max(crate::constants::PNG_ANOMALY_SCORE_CRITICAL);
                self.explanations.push(format!(
                    "Sampled palette-like distribution (≈{sampled_uniques} bins) — strong \
                     quantization indicator"
                ));
            } else if sampled_uniques <= 512 {
                self.factors.color_count_anomaly = self
                    .factors
                    .color_count_anomaly
                    .max(crate::constants::PNG_ANOMALY_SCORE_MEDIUM);
                self.explanations.push(format!(
                    "Sampled palette-like distribution (≈{sampled_uniques} bins) — possible \
                     quantization"
                ));
            }
        }

        Ok(())
    }

    fn apply_indexed_banding_and_frequency_signals(
        &mut self,
        img: &image::DynamicImage,
    ) -> Result<()> {
        let banding_score = detect_gradient_banding(img);
        self.factors.gradient_banding = banding_score;
        if banding_score > crate::constants::PNG_BANDING_THRESHOLD {
            self.explanations.push(format!(
                "Gradient banding detected (score: {banding_score:.2})"
            ));
        }

        let freq_score = detect_color_frequency_distribution(img)?;
        self.factors.color_frequency_distribution = freq_score;
        if freq_score > crate::constants::PNG_FREQ_THRESHOLD {
            self.explanations.push(format!(
                "Color frequency concentrated (score: {freq_score:.2}) — quantization indicator"
            ));
        }

        Ok(())
    }

    fn apply_indexed_entropy_signal(
        &mut self,
        img: &image::DynamicImage,
        profile: &PngPixelProfile,
    ) {
        let (entropy, max_entropy, entropy_ratio) =
            if let Some(palette_size) = self.png_info.palette_size {
                calculate_palette_index_entropy(img, palette_size)
            } else {
                let entropy = calculate_rgb_entropy(img);
                let max_entropy = crate::constants::PNG_MAX_INDEXED_COLORS.log2();
                let ratio = if max_entropy > 0.0_f64 {
                    entropy / max_entropy
                } else {
                    0.0_f64
                };
                (entropy, max_entropy, ratio)
            };

        let Some(palette_size) = self.png_info.palette_size else {
            return;
        };
        let palette_size_f = crate::numeric_cast::usize_to_f64(palette_size);

        if palette_size_f >= crate::constants::PNG_PALETTE_SIZE_ANOMALY_THRESHOLD
            && entropy_ratio < crate::constants::PNG_ENTROPY_RATIO_THRESHOLD_HIGH
            && profile.pixel_count > 10_000
        {
            self.factors.entropy_anomaly =
                (crate::constants::PNG_ENTROPY_RATIO_THRESHOLD_HIGH - entropy_ratio).mul_add(
                    crate::constants::ENTROPY_ANOMALY_MUL_ADD_FACTOR,
                    crate::constants::ENTROPY_ANOMALY_MUL_ADD_OFFSET,
                );
            self.factors.entropy_anomaly = self
                .factors
                .entropy_anomaly
                .clamp(0.0, crate::constants::ENTROPY_ANOMALY_UPPER_CLAMP);
            if self.factors.entropy_anomaly > crate::constants::PNG_ENTROPY_ANOMALY_THRESHOLD_LOW {
                self.explanations.push(format!(
                    "Low palette entropy ratio ({entropy_ratio:.2}, max {:.2}) — quantization \
                     indicator",
                    1.0_f64
                ));
            }
        } else if palette_size_f >= crate::constants::PNG_ENTROPY_PALETTE_SIZE_LARGE
            && entropy < crate::constants::PNG_ENTROPY_LOW_LIMIT
            && profile.pixel_count > crate::constants::PNG_ENTROPY_PIXEL_COUNT_LARGE
        {
            self.factors.entropy_anomaly = (crate::constants::PNG_ENTROPY_LOW_LIMIT - entropy)
                .mul_add(
                    crate::constants::ENTROPY_ANOMALY_MUL_ADD_FACTOR,
                    crate::constants::ENTROPY_ANOMALY_MUL_ADD_OFFSET,
                );
            self.factors.entropy_anomaly = self
                .factors
                .entropy_anomaly
                .clamp(0.0, crate::constants::PNG_ENTROPY_ANOMALY_MAX);
            if self.factors.entropy_anomaly > crate::constants::PNG_ENTROPY_ANOMALY_THRESHOLD {
                self.explanations.push(format!(
                    "Low entropy ({entropy:.2} vs max {max_entropy:.2}) — quantization indicator"
                ));
            }
        } else if palette_size_f >= crate::constants::PNG_ENTROPY_PALETTE_SIZE_MEDIUM
            && entropy_ratio < crate::constants::PNG_ENTROPY_RATIO_MEDIUM_CONFIDENCE
            && profile.pixel_count > crate::constants::PNG_ENTROPY_PIXEL_COUNT_MEDIUM
        {
            self.factors.entropy_anomaly = crate::constants::PNG_ANOMALY_SCORE_MIN;
        }
    }

    fn apply_compression_signal<R: Read + Seek>(&mut self, reader: &mut R) {
        let expected_size = estimate_uncompressed_size(&self.png_info);
        let actual_size = match reader.seek(SeekFrom::End(0)) {
            Ok(size) => size,
            Err(error) => {
                crate::progress_mode::emit_stderr(&format!(
                    "☢️ [ANOMALY] Seek failed: {error}. Refusing to forge actual_size."
                ));
                0
            }
        };
        let compression_ratio = if expected_size > 0 && actual_size > 0 {
            let actual = crate::numeric_cast::u64_to_u32_strict(actual_size, "actual_size");
            let expected = crate::numeric_cast::u64_to_u32_strict(expected_size, "expected_size");
            match (actual, expected) {
                (Some(actual), Some(expected)) if expected > 0 => {
                    f64::from(actual) / f64::from(expected)
                }
                _ => {
                    crate::progress_mode::emit_stderr(
                        "☢️ [ANOMALY] Compression ratio calculation failed due to overflow. \
                         Refusing to forge anomaly.",
                    );
                    1.0_f64
                }
            }
        } else {
            1.0_f64
        };

        if self.png_info.color_type == 3
            && compression_ratio < crate::constants::PNG_SIZE_EFFICIENCY_THRESHOLD
            && u64::from(self.png_info.width) * u64::from(self.png_info.height)
                > u64::from(crate::constants::PNG_EFFICIENCY_PIXEL_COUNT_THRESHOLD)
        {
            self.factors.size_efficiency_anomaly = crate::constants::PNG_SIZE_EFFICIENCY_ANOMALY;
            self.explanations.push(format!(
                "Unusually efficient compression ({:.1}%)",
                compression_ratio * crate::constants::SCALE_100
            ));
        }
    }

    fn finish_lossless_16bit(&self) -> Option<PngQuantizationAnalysis> {
        (self.png_info.bit_depth == 16).then(|| PngQuantizationAnalysis {
            is_quantized: false,
            quality_estimate: None,
            confidence: None,
            factor_scores: self.factors.clone(),
            detected_tool: None,
            explanation: "16-bit PNG - always lossless (no scored confidence)".to_string(),
        })
    }

    fn finish_truecolor_analysis(&mut self) -> Result<Option<PngQuantizationAnalysis>> {
        if !(self.png_info.color_type == 2 || self.png_info.color_type == 6)
            || self.detected_tool.is_some()
        {
            return Ok(None);
        }

        let Some(img) = self.open_analysis_image() else {
            return Ok(Some(PngQuantizationAnalysis {
                is_quantized: false,
                quality_estimate: None,
                confidence: Some(
                    crate::constants::IMAGE_DETECTION_CONFIDENCE_TRUECOLOR_INDICATORS_NONE,
                ),
                factor_scores: self.factors.clone(),
                detected_tool: None,
                explanation: "Truecolor PNG without quantization indicators".to_string(),
            }));
        };

        let profile = self.pixel_profile();
        let freq_signal = detect_color_frequency_distribution(&img)?;
        let rgb_entropy = calculate_rgb_entropy(&img);
        let entropy_ratio = rgb_entropy / crate::constants::PNG_MAX_RGB_ENTROPY;
        let entropy_signal = if entropy_ratio < crate::constants::PNG_ENTROPY_RATIO_HIGH_CONFIDENCE
            && profile.pixel_count > crate::constants::IMAGE_DETECTION_SMALL_PIXEL_THRESHOLD
        {
            crate::constants::PNG_ENTROPY_RATIO_HIGH
        } else if entropy_ratio < crate::constants::PNG_ENTROPY_RATIO_MEDIUM_CONFIDENCE
            && profile.pixel_count > crate::constants::IMAGE_DETECTION_SMALL_PIXEL_THRESHOLD
        {
            crate::constants::PNG_ENTROPY_RATIO_MEDIUM
        } else {
            0.0_f64
        };
        let banding_signal = detect_gradient_banding(&img);

        if let Some(result) = self.apply_truecolor_sampled_palette_signal(&img, &profile)? {
            return Ok(Some(result));
        }

        let strong_signals = [freq_signal, entropy_signal, banding_signal]
            .iter()
            .filter(|&&signal| signal >= crate::constants::IMAGE_DETECTION_STRONG_SIGNAL_THRESHOLD)
            .count();

        if std::env::var(crate::constants::ENV_DEBUG).is_ok() {
            crate::log_debug!(
                "      🎨 Truecolor analysis: freq={freq_signal:.2}, entropy={entropy_signal:.2} \
                 (raw={rgb_entropy:.2}), band={banding_signal:.2}, strong={strong_signals}"
            );
        }

        Ok(Some(self.build_truecolor_statistical_result(
            freq_signal,
            entropy_signal,
            banding_signal,
        )))
    }

    fn apply_truecolor_sampled_palette_signal(
        &mut self,
        img: &image::DynamicImage,
        profile: &PngPixelProfile,
    ) -> Result<Option<PngQuantizationAnalysis>> {
        let sampled_uniques =
            sample_unique_color_count(img, crate::constants::IMAGE_DETECTION_SAMPLING_SIZE)?;
        if sampled_uniques == 0
            || profile.pixel_count <= crate::constants::IMAGE_DETECTION_LARGE_PIXEL_THRESHOLD
        {
            return Ok(None);
        }
        if sampled_uniques <= crate::constants::PNG_PALETTE_SIZE_LIMIT {
            return Ok(Some(PngQuantizationAnalysis {
                is_quantized: true,
                quality_estimate: None,
                confidence: Some(crate::constants::IMAGE_DETECTION_CONFIDENCE_QUANTIZED),
                factor_scores: self.factors.clone(),
                detected_tool: None,
                explanation: format!(
                    "Sampled palette-like distribution (≈{sampled_uniques} bins) — likely \
                     pngquant-style quantization"
                ),
            }));
        }
        if sampled_uniques <= crate::constants::PNG_PALETTE_EXTENDED_LIMIT {
            self.factors.color_count_anomaly = self
                .factors
                .color_count_anomaly
                .max(crate::constants::IMAGE_DETECTION_COLOR_COUNT_ANOMALY_MAX);
            self.explanations.push(format!(
                "Sampled palette-like distribution (≈{sampled_uniques} bins) — possible \
                 quantization"
            ));
        }
        Ok(None)
    }

    fn build_truecolor_statistical_result(
        &self,
        freq_signal: f64,
        entropy_signal: f64,
        banding_signal: f64,
    ) -> PngQuantizationAnalysis {
        let freq_r = Self::rationalized_signal(
            freq_signal,
            "Frequency signal NaN/Inf in truecolor; defaulting to 0",
        );
        let ent_r = Self::rationalized_signal(
            entropy_signal,
            "Entropy signal NaN/Inf in truecolor; defaulting to 0",
        );
        let band_r = Self::rationalized_signal(
            banding_signal,
            "Banding signal NaN/Inf in truecolor; defaulting to 0",
        );
        let tc_score_f = ((freq_r + ent_r + band_r) / Rational::from(3)).to_f64();

        PngQuantizationAnalysis {
            is_quantized: true,
            quality_estimate: None,
            confidence: Some(tc_score_f.mul_add(
                crate::constants::IMAGE_DETECTION_TRUECOLOR_CONF_SLOPE,
                crate::constants::IMAGE_DETECTION_CONFIDENCE_TRUECOLOR_QUANT,
            )),
            factor_scores: self.factors.clone(),
            detected_tool: None,
            explanation: format!(
                "Truecolor quantization detected (freq={freq_signal:.2}, \
                 entropy={entropy_signal:.2}, band={banding_signal:.2})"
            ),
        }
    }

    fn rationalized_signal(signal: f64, anomaly_message: &str) -> Rational {
        if let Some(rational) =
            crate::media_conversion_gate::probe_rational_from_f64_optional(signal, anomaly_message)
        {
            return rational;
        }
        crate::Rational::from(0_u8)
    }

    fn finish_tool_signature(&self) -> Option<PngQuantizationAnalysis> {
        self.detected_tool
            .clone()
            .map(|detected_tool| PngQuantizationAnalysis {
                is_quantized: true,
                quality_estimate: None,
                confidence: Some(crate::constants::IMAGE_DETECTION_CONFIDENCE_TOOL_SIGNATURE),
                factor_scores: self.factors.clone(),
                detected_tool: Some(detected_tool),
                explanation: self.explanations.join("; "),
            })
    }

    fn finish_scored_analysis(self) -> PngQuantizationAnalysis {
        let score_summary = self.build_score_summary();
        let (is_quantized, confidence) = Self::classify_final_score(score_summary.final_score);
        let explanation = if self.explanations.is_empty() {
            if is_quantized {
                format!(
                    "Quantization detected (score: {:.2})",
                    score_summary.final_score
                )
            } else {
                format!(
                    "No quantization indicators (score: {:.2})",
                    score_summary.final_score
                )
            }
        } else {
            self.explanations.join("; ")
        };

        PngQuantizationAnalysis {
            is_quantized,
            quality_estimate: None,
            confidence: Some(confidence.min(1.0)),
            factor_scores: self.factors,
            detected_tool: self.detected_tool,
            explanation,
        }
    }

    fn build_score_summary(&self) -> PngScoreSummary {
        let weights = PngQuantizationWeights {
            structural: crate::constants::DETECTION_WEIGHT_STRUCTURAL,
            metadata: crate::constants::DETECTION_WEIGHT_METADATA,
            statistical: crate::constants::DETECTION_WEIGHT_STATISTICAL,
            heuristic: crate::constants::DETECTION_WEIGHT_HEURISTIC,
        };
        let structural_score =
            f64::midpoint(self.factors.indexed_with_alpha, self.factors.large_palette);
        let metadata_score = self.factors.tool_signature;
        let statistical_score = (self.factors.dithering_detected
            + self.factors.color_count_anomaly
            + self.factors.gradient_banding
            + self.factors.color_frequency_distribution)
            / crate::constants::DETECTION_STATISTICAL_DIVISOR;
        let heuristic_score = f64::midpoint(
            self.factors.size_efficiency_anomaly,
            self.factors.entropy_anomaly,
        );
        let final_score = heuristic_score.mul_add(
            weights.heuristic,
            statistical_score.mul_add(
                weights.statistical,
                structural_score.mul_add(weights.structural, metadata_score * weights.metadata),
            ),
        );

        if std::env::var(crate::constants::ENV_DEBUG).is_ok() {
            crate::log_debug!(
                "         Structural: {structural_score:.2} (indexed_alpha={indexed_alpha:.2}, \
                 large_palette={large_palette:.2}) × {weight:.2} = {total:.3}",
                indexed_alpha = self.factors.indexed_with_alpha,
                large_palette = self.factors.large_palette,
                weight = weights.structural,
                total = structural_score * weights.structural
            );
            crate::log_debug!(
                "         Metadata: {metadata_score:.2} × {weight:.2} = {total:.3}",
                weight = weights.metadata,
                total = metadata_score * weights.metadata
            );
            crate::log_debug!(
                "         Statistical: {statistical_score:.2} (dither={dither:.2}, \
                 color={color:.2}, band={band:.2}, freq={freq:.2}) × {weight:.2} = {total:.3}",
                dither = self.factors.dithering_detected,
                color = self.factors.color_count_anomaly,
                band = self.factors.gradient_banding,
                freq = self.factors.color_frequency_distribution,
                weight = weights.statistical,
                total = statistical_score * weights.statistical
            );
            crate::log_debug!(
                "         Heuristic: {heuristic_score:.2} × {weight:.2} = {total:.3}",
                weight = weights.heuristic,
                total = heuristic_score * weights.heuristic
            );
            crate::log_debug!(
                "         FINAL SCORE: {final_score:.3} (threshold: {:.2} for lossy, gray zone: \
                 [{:.2}, {:.2}] → lossless)",
                crate::constants::PNG_QUANT_THRESHOLD_HIGH,
                crate::constants::PNG_QUANT_THRESHOLD_LOW,
                crate::constants::PNG_QUANT_THRESHOLD_HIGH
            );
        }

        PngScoreSummary { final_score }
    }

    fn classify_final_score(final_score: f64) -> (bool, f64) {
        let lossy_threshold = crate::constants::PNG_QUANT_THRESHOLD_HIGH;
        if final_score >= crate::constants::IMAGE_DETECTION_FINAL_SCORE_HIGH {
            (
                true,
                (final_score - crate::constants::IMAGE_DETECTION_FINAL_SCORE_HIGH).mul_add(
                    crate::constants::IMAGE_DETECTION_CONFIDENCE_SCALING_HIGH,
                    crate::constants::IMAGE_DETECTION_CONFIDENCE_BASE_HIGH,
                ),
            )
        } else if final_score >= lossy_threshold {
            (
                true,
                (final_score - lossy_threshold)
                    .mul_add(1.0, crate::constants::PNG_SCORER_HIGH_CONF_BIAS),
            )
        } else if final_score >= crate::constants::PNG_QUANT_THRESHOLD_LOW {
            (
                false,
                (lossy_threshold - final_score)
                    .mul_add(1.0, crate::constants::PNG_SCORER_NEUTRAL_BIAS),
            )
        } else if final_score >= crate::constants::IMAGE_DETECTION_FINAL_SCORE_MEDIUM {
            (
                false,
                (lossy_threshold - final_score)
                    .mul_add(1.0, crate::constants::IMAGE_DETECTION_CONFIDENCE_BASE_LOW),
            )
        } else {
            (
                false,
                (crate::constants::IMAGE_DETECTION_FINAL_SCORE_MEDIUM - final_score).mul_add(
                    crate::constants::IMAGE_DETECTION_CONFIDENCE_SCALING_MEDIUM,
                    crate::constants::IMAGE_DETECTION_CONFIDENCE_OFFSET_MEDIUM,
                ),
            )
        }
    }
}

/// Analyze PNG quantization from a generic reader.
///
/// # Errors
/// Returns an error if the PNG structure is invalid or decoding fails.
pub fn analyze_png_quantization_from_reader<R: Read + Seek>(
    mut reader: R,
    path: Option<&Path>,
) -> Result<PngQuantizationAnalysis> {
    let png_info = parse_png_structure(&mut reader)?;
    PngQuantizationSession::new(png_info, path).analyze(&mut reader)
}

pub struct PngStructureInfo {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    pub palette_size: Option<usize>,
    pub has_trns: bool,
    pub has_text_chunks: bool,
    pub detected_tool: Option<String>,
}

struct PngQuantizationWeights {
    structural: f64,
    metadata: f64,
    statistical: f64,
    heuristic: f64,
}

fn decompress_png_text_bounded(data: &[u8], remaining_budget: &mut usize) -> Result<Vec<u8>> {
    let allowed = *remaining_budget;
    let read_limit = u64::try_from((*remaining_budget).saturating_add(1)).map_err(|error| {
        ImgQualityError::AnalysisError(format!(
            "PNG text decompression limit conversion failed: {error}"
        ))
    })?;
    let mut decompressed = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(read_limit)
        .read_to_end(&mut decompressed)
        .map_err(|error| {
            ImgQualityError::AnalysisError(format!(
                "PNG compressed text payload is invalid: {error}"
            ))
        })?;
    if decompressed.len() > *remaining_budget {
        return Err(ImgQualityError::AnalysisError(format!(
            "PNG decompressed text exceeds remaining {allowed} byte safety budget"
        )));
    }
    *remaining_budget -= decompressed.len();
    Ok(decompressed)
}

/// # Errors
/// Returns an error if the file cannot be read or if the PNG structure is
/// corrupted. Specifically, `ImgQualityError::IoError` for file operations and
/// `ImgQualityError::AnalysisError` for parsing issues.
pub fn parse_png_structure<R: Read + Seek>(mut reader: R) -> Result<PngStructureInfo> {
    const PNG_DIMENSION_MAX: u32 = 0x7FFF_FFFF;

    fn skip_bytes<R: Seek>(
        reader: &mut R,
        bytes: u64,
        stream_end: u64,
        context: &str,
    ) -> Result<()> {
        let current = reader.stream_position().map_err(|error| {
            ImgQualityError::AnalysisError(format!(
                "Failed to locate PNG stream while parsing {context}: {error}"
            ))
        })?;
        let next = current
            .checked_add(bytes)
            .filter(|next| *next <= stream_end)
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "PNG {context} extends beyond the end of the file"
                ))
            })?;
        reader.seek(SeekFrom::Start(next)).map_err(|error| {
            ImgQualityError::AnalysisError(format!("Failed to seek past PNG {context}: {error}"))
        })?;
        Ok(())
    }

    let initial_position = reader.stream_position().map_err(|error| {
        ImgQualityError::AnalysisError(format!("Failed to locate PNG stream start: {error}"))
    })?;
    let stream_end = reader.seek(SeekFrom::End(0)).map_err(|error| {
        ImgQualityError::AnalysisError(format!("Failed to locate PNG stream end: {error}"))
    })?;
    reader
        .seek(SeekFrom::Start(initial_position))
        .map_err(|error| {
            ImgQualityError::AnalysisError(format!("Failed to rewind PNG stream: {error}"))
        })?;

    let mut header = [0u8; 8];

    reader
        .read_exact(&mut header)
        .map_err(|e| ImgQualityError::AnalysisError(format!("PNG too small: {e}")))?;
    if header != [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Err(ImgQualityError::AnalysisError(
            "Invalid PNG signature".to_string(),
        ));
    }

    // Read IHDR
    let mut ihdr_header = [0u8; 8];
    reader
        .read_exact(&mut ihdr_header)
        .map_err(|e| ImgQualityError::AnalysisError(format!("Missing IHDR: {e}")))?;
    if u32::from_be_bytes([
        ihdr_header[0],
        ihdr_header[1],
        ihdr_header[2],
        ihdr_header[3],
    ]) != 13
        || &ihdr_header[4..8] != b"IHDR"
    {
        return Err(ImgQualityError::AnalysisError(
            "PNG must begin with a 13-byte IHDR chunk".to_string(),
        ));
    }
    let mut ihdr_data = [0u8; 13];
    reader
        .read_exact(&mut ihdr_data)
        .map_err(|e| ImgQualityError::AnalysisError(format!("IHDR data truncated: {e}")))?;

    let width = u32::from_be_bytes([ihdr_data[0], ihdr_data[1], ihdr_data[2], ihdr_data[3]]);
    let height = u32::from_be_bytes([ihdr_data[4], ihdr_data[5], ihdr_data[6], ihdr_data[7]]);
    let bit_depth = ihdr_data[8];
    let color_type = ihdr_data[9];
    let compression_method = ihdr_data[10];
    let filter_method = ihdr_data[11];
    let interlace_method = ihdr_data[12];
    if width == 0 || height == 0 || width > PNG_DIMENSION_MAX || height > PNG_DIMENSION_MAX {
        return Err(ImgQualityError::AnalysisError(
            "PNG IHDR dimensions are outside the valid range".to_string(),
        ));
    }
    let valid_depth = match color_type {
        0 => [1, 2, 4, 8, 16].contains(&bit_depth),
        2 | 4 | 6 => [8, 16].contains(&bit_depth),
        3 => [1, 2, 4, 8].contains(&bit_depth),
        _ => false,
    };
    if !valid_depth || compression_method != 0 || filter_method != 0 || interlace_method > 1 {
        return Err(ImgQualityError::AnalysisError(
            "PNG IHDR contains an unsupported format field".to_string(),
        ));
    }
    skip_bytes(&mut reader, 4, stream_end, "IHDR CRC")?;

    let mut palette_size: Option<usize> = None;
    let mut has_trns = false;
    let mut has_text_chunks = false;
    let mut detected_tool: Option<String> = None;
    let mut decompressed_text_budget = crate::constants::PNG_TEXT_CHUNK_SIZE_LIMIT;
    let mut saw_idat = false;

    let signatures: &[(&str, &str)] = &[
        ("pngquant", "pngquant"),
        ("pngnq", "pngnq"),
        ("TinyPNG", "TinyPNG"),
        ("tinypng", "TinyPNG"),
        ("ImageOptim", "ImageOptim"),
        ("imageoptim", "ImageOptim"),
        ("posterize", "posterize"),
        ("quantize", "quantize tool"),
        ("Quantized", "quantization"),
        ("color reduction", "color reduction"),
        ("palette optimization", "palette optimization"),
        ("Squoosh", "Squoosh"),
        ("squoosh", "Squoosh"),
        ("sharp", "sharp"),
        ("libvips", "sharp/libvips"),
        ("pngcrush", "pngcrush"),
        ("PNGOUT", "PNGOUT"),
        ("pngout", "PNGOUT"),
        ("Fireworks", "Adobe Fireworks"),
        ("Adobe Fireworks", "Adobe Fireworks"),
        ("Sketch", "Sketch"),
        ("com.bohemiancoding", "Sketch"),
    ];

    let mut buf = [0u8; 8];
    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(ImgQualityError::AnalysisError(
                    "PNG ended before the IEND chunk".to_string(),
                ));
            }
            Err(e) => {
                return Err(ImgQualityError::AnalysisError(format!(
                    "Failed to read PNG chunk header: {e}"
                )));
            }
        }
        let chunk_len = u64::from(u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]));
        let chunk_type = &buf[4..8];

        match chunk_type {
            b"PLTE" => {
                let plte_len =
                    crate::numeric_cast::u64_to_usize_strict(chunk_len, "PLTE chunk_len")
                        .ok_or_else(|| {
                            ImgQualityError::AnalysisError(format!(
                                "Invalid PLTE length: {chunk_len}"
                            ))
                        })?;
                if plte_len == 0 || plte_len > 768 || plte_len % 3 != 0 {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "Invalid PLTE length: {plte_len}"
                    )));
                }
                if color_type == 0 || color_type == 4 {
                    return Err(ImgQualityError::AnalysisError(
                        "PLTE is forbidden for greyscale PNG images".to_string(),
                    ));
                }
                if color_type == 3 {
                    palette_size = Some(plte_len / 3);
                }
                skip_bytes(&mut reader, chunk_len + 4, stream_end, "PLTE chunk")?;
            }
            b"IDAT" => {
                saw_idat = true;
                skip_bytes(&mut reader, chunk_len + 4, stream_end, "IDAT chunk")?;
            }
            b"tRNS" => {
                has_trns = true;
                skip_bytes(&mut reader, chunk_len + 4, stream_end, "tRNS chunk")?;
            }
            b"tEXt" | b"iTXt" | b"zTXt" if detected_tool.is_none() => {
                has_text_chunks = true;
                let text_len =
                    crate::numeric_cast::u64_to_usize_strict(chunk_len, "PNG text chunk_len")
                        .ok_or_else(|| {
                            ImgQualityError::AnalysisError(format!(
                                "Invalid text length: {chunk_len}"
                            ))
                        })?;
                if text_len > crate::constants::PNG_TEXT_CHUNK_SIZE_LIMIT {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "PNG text chunk exceeds {} byte safety limit",
                        crate::constants::PNG_TEXT_CHUNK_SIZE_LIMIT
                    )));
                }
                let mut payload = vec![0u8; text_len];
                reader.read_exact(&mut payload).map_err(|e| {
                    ImgQualityError::AnalysisError(format!(
                        "Failed to read PNG text chunk payload: {e}"
                    ))
                })?;
                let null_pos = payload.iter().position(|&b| b == 0).ok_or_else(|| {
                    ImgQualityError::AnalysisError(
                        "PNG text chunk is missing its keyword separator".to_string(),
                    )
                })?;
                if !(1..=79).contains(&null_pos) {
                    return Err(ImgQualityError::AnalysisError(
                        "PNG text keyword length must be between 1 and 79 bytes".to_string(),
                    ));
                }
                let keyword = String::from_utf8_lossy(&payload[..null_pos]);
                for &(pattern, tool_name) in signatures {
                    if keyword.contains(pattern) {
                        detected_tool = Some(tool_name.to_string());
                        break;
                    }
                }

                if detected_tool.is_none() {
                    let (text_payload, is_compressed) = match chunk_type {
                        b"zTXt" => {
                            let method = payload.get(null_pos + 1).copied().ok_or_else(|| {
                                ImgQualityError::AnalysisError(
                                    "PNG zTXt chunk is missing its compression method".to_string(),
                                )
                            })?;
                            if method != 0 {
                                return Err(ImgQualityError::AnalysisError(format!(
                                    "PNG zTXt chunk uses unsupported compression method {method}"
                                )));
                            }
                            let compressed = payload.get(null_pos + 2..).ok_or_else(|| {
                                ImgQualityError::AnalysisError(
                                    "PNG zTXt chunk is missing compressed text".to_string(),
                                )
                            })?;
                            (compressed, true)
                        }
                        b"iTXt" => {
                            let comp_flag =
                                payload.get(null_pos + 1).copied().ok_or_else(|| {
                                    ImgQualityError::AnalysisError(
                                        "PNG iTXt chunk is missing its compression flag"
                                            .to_string(),
                                    )
                                })?;
                            let method = payload.get(null_pos + 2).copied().ok_or_else(|| {
                                ImgQualityError::AnalysisError(
                                    "PNG iTXt chunk is missing its compression method".to_string(),
                                )
                            })?;
                            if comp_flag > 1 || (comp_flag == 1 && method != 0) {
                                return Err(ImgQualityError::AnalysisError(format!(
                                    "PNG iTXt chunk has invalid compression flag/method {comp_flag}/{method}"
                                )));
                            }
                            let mut pos = null_pos + 3;
                            let lang_null = payload
                                .get(pos..)
                                .and_then(|rest| rest.iter().position(|&byte| byte == 0))
                                .ok_or_else(|| {
                                    ImgQualityError::AnalysisError(
                                        "PNG iTXt chunk is missing its language separator"
                                            .to_string(),
                                    )
                                })?;
                            pos += lang_null + 1;
                            let translated_null = payload
                                .get(pos..)
                                .and_then(|rest| rest.iter().position(|&byte| byte == 0))
                                .ok_or_else(|| {
                                    ImgQualityError::AnalysisError(
                                        "PNG iTXt chunk is missing its translated keyword separator"
                                            .to_string(),
                                    )
                                })?;
                            pos += translated_null + 1;
                            let text = payload.get(pos..).ok_or_else(|| {
                                ImgQualityError::AnalysisError(
                                    "PNG iTXt chunk has an invalid text offset".to_string(),
                                )
                            })?;
                            (text, comp_flag == 1)
                        }
                        b"tEXt" => {
                            let text = payload.get(null_pos + 1..).ok_or_else(|| {
                                ImgQualityError::AnalysisError(
                                    "PNG tEXt chunk has an invalid text offset".to_string(),
                                )
                            })?;
                            (text, false)
                        }
                        _ => {
                            return Err(ImgQualityError::AnalysisError(
                                "Unsupported PNG text chunk type".to_string(),
                            ));
                        }
                    };

                    let decompressed;
                    let text = if is_compressed {
                        decompressed = decompress_png_text_bounded(
                            text_payload,
                            &mut decompressed_text_budget,
                        )?;
                        String::from_utf8_lossy(&decompressed)
                    } else {
                        String::from_utf8_lossy(text_payload)
                    };
                    for &(pattern, tool_name) in signatures {
                        if text.contains(pattern) {
                            detected_tool = Some(tool_name.to_string());
                            break;
                        }
                    }
                }
                skip_bytes(&mut reader, 4, stream_end, "text chunk CRC")?;
            }
            b"IEND" => {
                if chunk_len != 0 {
                    return Err(ImgQualityError::AnalysisError(
                        "PNG IEND chunk must be empty".to_string(),
                    ));
                }
                if !saw_idat {
                    return Err(ImgQualityError::AnalysisError(
                        "PNG IEND appeared before any IDAT chunk".to_string(),
                    ));
                }
                skip_bytes(&mut reader, 4, stream_end, "IEND CRC")?;
                break;
            }
            _ => {
                skip_bytes(&mut reader, chunk_len + 4, stream_end, "PNG chunk")?;
            }
        }
    }

    Ok(PngStructureInfo {
        width,
        height,
        bit_depth,
        color_type,
        palette_size,
        has_trns,
        has_text_chunks,
        detected_tool,
    })
}

fn detect_dithering_pattern(img: &DynamicImage) -> anyhow::Result<f64> {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    if width < 8 || height < 8 {
        return Ok(0.0);
    }

    let mut high_freq_count = 0u64;
    let mut total_comparisons = 0u64;

    let step = crate::numeric_cast::f64_to_u32_strict(
        (crate::numeric_cast::u64_to_f64(u64::from(width) * u64::from(height))
            / crate::constants::PNG_DITHER_SAMPLING_FACTOR)
            .max(1.0),
        "step",
    )
    .ok_or_else(|| anyhow::anyhow!("Sampling step overflow during dithering detection"))?;

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if (x + y * width) % step != 0 {
                continue;
            }

            let center = rgba.get_pixel(x, y);
            // 8-neighbor check: cardinal + diagonal catches Floyd-Steinberg diagonal
            // artifacts
            let neighbors = [
                rgba.get_pixel(x - 1, y),
                rgba.get_pixel(x + 1, y),
                rgba.get_pixel(x, y - 1),
                rgba.get_pixel(x, y + 1),
                rgba.get_pixel(x - 1, y - 1),
                rgba.get_pixel(x + 1, y - 1),
                rgba.get_pixel(x - 1, y + 1),
                rgba.get_pixel(x + 1, y + 1),
            ];

            let mut alternations = 0_i32;
            for neighbor in &neighbors {
                let diff = color_difference(*center, **neighbor);
                if diff > crate::constants::DITHER_DIFF_MIN
                    && diff < crate::constants::DITHER_DIFF_MAX
                {
                    alternations += 1_i32;
                }
            }

            if alternations >= crate::constants::DITHER_ALTERNATION_THRESHOLD {
                high_freq_count += 1;
            }
            total_comparisons += 1;
        }
    }

    if total_comparisons == 0 {
        return Ok(0.0);
    }

    let dithering_ratio = {
        let count = crate::numeric_cast::u64_to_u32_strict(high_freq_count, "high_freq_count");
        let total = crate::numeric_cast::u64_to_u32_strict(total_comparisons, "total_comparisons");

        match (count, total) {
            (Some(c), Some(t)) if t > 0 => f64::from(c) / f64::from(t),
            _ => {
                crate::media_conversion_gate::probe_layer_batch_audit(
                    "delivery_db_numeric",
                    "Dithering ratio overflow! Refusing to forge anomaly score.",
                );
                0.0
            }
        }
    };

    let floyd_steinberg_score =
        (dithering_ratio * crate::constants::DITHER_FLOYD_STEINBERG_MULTIPLIER).min(1.0);

    // Bayer/ordered dithering: 2x2 checkerboard — diagonal pairs similar, cross
    // pairs differ
    let mut bayer_count = 0u64;
    let mut bayer_total = 0u64;
    for y in (1..height.saturating_sub(1)).step_by(2) {
        for x in (1..width.saturating_sub(1)).step_by(2) {
            if (x + y * width) % step != 0 {
                continue;
            }
            let c00 = rgba.get_pixel(x, y);
            let c10 = rgba.get_pixel(x + 1, y);
            let c01 = rgba.get_pixel(x, y + 1);
            let c11 = rgba.get_pixel(x + 1, y + 1);
            let diag_diff = color_difference(*c00, *c11) + color_difference(*c10, *c01);
            let cross_diff = color_difference(*c00, *c10) + color_difference(*c00, *c01);
            if cross_diff > crate::constants::DITHER_CROSS_DIFF_THRESHOLD
                && diag_diff < cross_diff * crate::constants::DITHER_DIAG_RATIO
            {
                bayer_count += 1;
            }
            bayer_total += 1;
        }
    }
    let bayer_score = if bayer_total > 0 {
        let count = crate::numeric_cast::u64_to_u32_strict(bayer_count, "bayer_count");
        let total = crate::numeric_cast::u64_to_u32_strict(bayer_total, "bayer_total");

        match (count, total) {
            (Some(c), Some(t)) if t > 0 => {
                ((f64::from(c) / f64::from(t)) * crate::constants::DITHER_DENSE_MULTIPLIER).min(1.0)
            }
            _ => {
                crate::progress_mode::emit_stderr(
                    "☢️ [ANOMALY] Bayer ratio overflow! Refusing to forge anomaly score.",
                );
                0.0
            }
        }
    } else {
        0.0_f64
    };

    Ok(floyd_steinberg_score.max(bayer_score))
}

/// Perceptually weighted color difference (Compuphase approximation).
/// Human vision: green > red > blue sensitivity. Equal-weight Euclidean RGB
/// under-weights green differences and over-weights blue, causing dithering
/// detection to miss green-channel artifacts and false-trigger on blue noise.
fn color_difference(a: Rgba<u8>, b: Rgba<u8>) -> f64 {
    let rmean = f64::midpoint(f64::from(a[0]), f64::from(b[0]));
    let dr = f64::from(a[0]) - f64::from(b[0]);
    let dg = f64::from(a[1]) - f64::from(b[1]);
    let db = f64::from(a[2]) - f64::from(b[2]);
    // Weights shift with mean red: redder pixels → more red weight, bluer → more
    // blue weight
    let wr =
        crate::constants::COLOR_DIFF_WEIGHT_R_BASE + rmean / crate::constants::COLOR_DIFF_DIVISOR;
    let wg = crate::constants::COLOR_DIFF_WEIGHT_G;
    let wb = crate::constants::COLOR_DIFF_WEIGHT_B_BASE
        + (255.0_f64 - rmean) / crate::constants::COLOR_DIFF_DIVISOR;
    (wb * db)
        .mul_add(db, (wr * dr).mul_add(dr, wg * dg * dg))
        .sqrt()
}

/// Sample image pixels (grid-subsample) and count unique quantized colors.
/// Uses a small quantization (5 bits per channel) to approximate palette
/// variety without full quantization work. Returns number of unique colors
/// observed.
fn sample_unique_color_count(img: &DynamicImage, max_samples: usize) -> anyhow::Result<usize> {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    if width == 0 || height == 0 {
        return Ok(0);
    }

    let total = u64::from(width) * u64::from(height);
    let step = crate::numeric_cast::f64_to_u32_strict(
        (crate::numeric_cast::u64_to_f64(total)
            / crate::numeric_cast::usize_to_f64(max_samples).max(1.0))
        .sqrt()
        .ceil(),
        "step",
    )
    .ok_or_else(|| anyhow::anyhow!("Sampling step overflow in unique color count"))?;
    let step = step.max(1);

    let mut set = HashSet::new();

    let mut sampled = 0usize;
    let step_usize = crate::numeric_cast::u32_to_usize_strict(step, "sample_step")
        .ok_or_else(|| anyhow::anyhow!("Sampling step overflow in unique color count"))?;
    for y in (0..height).step_by(step_usize) {
        for x in (0..width).step_by(step_usize) {
            let p = rgba.get_pixel(x, y);
            // 5-bit per channel quantization (approximate palette bins)
            let r5 = p[0] >> 3_i32;
            let g5 = p[1] >> 3_i32;
            let b5 = p[2] >> 3_i32;
            let key = (u32::from(r5) << 16_i32) | (u32::from(g5) << 8_i32) | u32::from(b5);
            set.insert(key);
            sampled += 1;
            if sampled >= max_samples {
                break;
            }
        }
        if sampled >= max_samples {
            break;
        }
    }

    Ok(set.len())
}

/// Block-based random sampling — divides image into grid cells and randomly
/// samples from each, avoiding the systematic bias of stride sampling (which
/// creates periodic blind spots on structured images like game UI screenshots).
/// Quantized images have concentrated color distributions; stride sampling can
/// miss local color clusters.
const fn lcg_next(state: &mut u64) -> u32 {
    // Simple 64-bit LCG; return high bits for better distribution
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005_u64)
        .wrapping_add(1_442_695_040_888_963_407_u64);
    crate::numeric_cast::u64_high32_to_u32(*state)
}

fn analyze_color_distribution(
    img: &DynamicImage,
    _palette_size: Option<usize>,
) -> anyhow::Result<(usize, usize)> {
    let rgba = img.to_rgba8();
    let mut color_set: HashMap<[u8; 4], u32> = HashMap::new();

    let (width, height) = rgba.dimensions();
    let total_pixels_u64 = u64::from(width) * u64::from(height);
    let Ok(total_pixels) = usize::try_from(total_pixels_u64) else {
        return Err(anyhow::anyhow!(
            "Image dimensions overflow usize in color distribution analysis"
        ));
    };

    // Target ~50k samples, distributed across a grid of blocks
    let target_samples: usize = 50_000;
    let grid_size: u32 = 16; // 16x16 = 256 blocks
    let block_w = (width / grid_size).max(1);
    let block_h = (height / grid_size).max(1);
    let grid_size_usize = crate::numeric_cast::u32_to_usize_strict(grid_size, "grid_size")
        .ok_or_else(|| anyhow::anyhow!("grid_size overflow"))?;
    let grid_cells = grid_size_usize
        .checked_mul(grid_size_usize)
        .ok_or_else(|| anyhow::anyhow!("grid_size squared overflow"))?;
    let samples_per_block = (target_samples / grid_cells.max(1)).max(1);

    // Simple LCG for deterministic pseudo-random sampling (no need for rand crate)
    let mut rng_state: u64 = 0x1234_5678_9ABC_DEF0;
    crate::numeric_cast::u64_to_u32_strict(rng_state >> 32, "lcg_state")
        .ok_or_else(|| anyhow::anyhow!("LCG state overflow during random sampling"))?;

    for by in 0..grid_size {
        for bx in 0..grid_size {
            let x0 = (bx * block_w).min(width);
            let y0 = (by * block_h).min(height);
            let x1 = ((bx + 1) * block_w).min(width);
            let y1 = ((by + 1) * block_h).min(height);
            let current_block_width = x1 - x0;
            let current_block_height = y1 - y0;
            let block_pixels = crate::numeric_cast::u64_to_usize_strict(
                u64::from(current_block_width) * u64::from(current_block_height),
                "block_pixels",
            )
            .ok_or_else(|| {
                anyhow::anyhow!("Block pixels overflow usize in color distribution analysis")
            })?;
            if block_pixels == 0 {
                continue;
            }

            // Random sampling within this block
            let n_samples = samples_per_block.min(block_pixels);
            for _ in 0..n_samples {
                let rand_x = x0 + (lcg_next(&mut rng_state) % current_block_width);
                let rand_y = y0 + (lcg_next(&mut rng_state) % current_block_height);
                let pixel = rgba.get_pixel(rand_x, rand_y);
                let key = [pixel[0], pixel[1], pixel[2], pixel[3]];
                *color_set.entry(key).or_insert(0) += 1;
            }
        }
    }

    let unique_colors = color_set.len();

    let expected = if total_pixels > crate::constants::SAMPLED_COLORS_PIXELS_LARGE {
        crate::constants::SAMPLED_COLORS_EXPECTED_LARGE
    } else if total_pixels > crate::constants::SAMPLED_COLORS_PIXELS_MEDIUM {
        crate::constants::SAMPLED_COLORS_EXPECTED_MEDIUM
    } else {
        crate::constants::SAMPLED_COLORS_EXPECTED_SMALL
    };

    Ok((unique_colors, expected))
}

/// Color frequency concentration — quantized images have a few dominant colors
/// covering most pixels. Natural palette art distributes more evenly.
/// Returns score in [0.0, 1.0] where high = likely quantized.
fn detect_color_frequency_distribution(img: &DynamicImage) -> anyhow::Result<f64> {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let width_u = crate::numeric_cast::u32_to_usize_strict(width, "width")
        .ok_or_else(|| anyhow::anyhow!("Width overflow in color frequency analysis"))?;
    let height_u = crate::numeric_cast::u32_to_usize_strict(height, "height")
        .ok_or_else(|| anyhow::anyhow!("Height overflow in color frequency analysis"))?;
    let total_pixels = width_u
        .checked_mul(height_u)
        .ok_or_else(|| anyhow::anyhow!("Total pixels overflow in color frequency analysis"))?;
    if total_pixels < crate::constants::COLOR_DIST_MIN_PIXELS {
        return Ok(0.0);
    }

    // Block-random sampling: divide image into a grid of blocks, sample one pixel
    // per block at a deterministic-but-spread position. Avoids stride bias where
    // step-based sampling always hits the same spatial columns/rows.
    let target_samples: usize = crate::constants::COLOR_DIST_TARGET_SAMPLES.min(total_pixels);
    let block_size = crate::numeric_cast::f64_to_usize_strict(
        (crate::numeric_cast::u64_to_f64(crate::numeric_cast::usize_to_u64(total_pixels))
            / crate::numeric_cast::usize_to_f64(target_samples).max(1.0))
        .max(1.0),
        "block_size",
    )
    .ok_or_else(|| anyhow::anyhow!("Block size overflow in color frequency analysis"))?;
    let block_size = block_size.max(1);
    let blocks_x = width_u.div_ceil(block_size);
    let blocks_y = height_u.div_ceil(block_size);

    let mut color_freq: std::collections::HashMap<[u8; 4], u32> = std::collections::HashMap::new();
    let mut sampled = 0u64;

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            // Pick a pixel near the center of each block (deterministic, no RNG needed)
            // Calculate coordinates using high-precision arithmetic to prevent overflow
            // Calculate coordinates using u64 to prevent overflow (sufficient for any
            // practical image)
            let px = {
                let x = crate::numeric_cast::usize_to_u64(bx)
                    * crate::numeric_cast::usize_to_u64(block_size)
                    + crate::numeric_cast::usize_to_u64(block_size) / 2;
                let max_x = u64::from(width).saturating_sub(1);
                crate::numeric_cast::u64_to_u32_strict(x.min(max_x), "px")
                    .ok_or_else(|| anyhow::anyhow!("px overflow in color frequency analysis"))?
            };
            let py = {
                let y = crate::numeric_cast::usize_to_u64(by)
                    * crate::numeric_cast::usize_to_u64(block_size)
                    + crate::numeric_cast::usize_to_u64(block_size) / 2;
                let max_y = u64::from(height).saturating_sub(1);
                crate::numeric_cast::u64_to_u32_strict(y.min(max_y), "py")
                    .ok_or_else(|| anyhow::anyhow!("py overflow in color frequency analysis"))?
            };

            let pixel = rgba.get_pixel(px, py);
            let key = [pixel[0], pixel[1], pixel[2], pixel[3]];
            *color_freq.entry(key).or_insert(0) += 1;
            sampled += 1;
        }
    }

    if sampled == 0 || color_freq.len() < 2 {
        return Ok(0.0);
    }

    let mut freqs: Vec<u32> = color_freq.values().copied().collect();
    freqs.sort_unstable_by(|a, b| b.cmp(a));

    let target = crate::numeric_cast::f64_to_u64_strict(
        crate::numeric_cast::u64_to_f64(sampled)
            * crate::constants::PNG_COLOR_CONCENTRATION_TARGET_RATIO,
        "entropy_target",
    )
    .ok_or_else(|| anyhow::anyhow!("Entropy target calculation overflowed u64"))?;
    let mut cumulative = 0u64;
    let mut colors_for_85pct = 0usize;
    for &f in &freqs {
        cumulative += u64::from(f);
        colors_for_85pct += 1;
        if cumulative >= target {
            break;
        }
    }

    // Low ratio = few colors dominate = quantized
    let coverage_ratio =
        crate::numeric_cast::u64_to_f64(crate::numeric_cast::usize_to_u64(colors_for_85pct))
            / crate::numeric_cast::u64_to_f64(crate::numeric_cast::usize_to_u64(freqs.len()))
                .max(1.0);

    let score = if coverage_ratio < crate::constants::PNG_COVERAGE_RATIO_ULTRA_LOW {
        crate::constants::PNG_COVERAGE_ULTRA_LOW_SCORE
    } else if coverage_ratio < crate::constants::PNG_COVERAGE_TIER1_THRESHOLD {
        crate::constants::PNG_COVERAGE_TIER1_SCORE
    } else if coverage_ratio < crate::constants::PNG_COVERAGE_TIER2_THRESHOLD {
        crate::constants::PNG_COVERAGE_TIER2_SCORE
    } else if coverage_ratio < crate::constants::PNG_COVERAGE_TIER3_THRESHOLD {
        crate::constants::PNG_COVERAGE_TIER3_SCORE
    } else {
        0.0
    };
    Ok(score)
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn detect_gradient_banding(img: &DynamicImage) -> f64 {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    if width < crate::constants::BANDING_MIN_DIM || height < crate::constants::BANDING_MIN_DIM {
        return 0.0;
    }

    // Per-channel detection weighted by human visual sensitivity (G > R > B).
    // Grayscale projection loses hue info — red vs blue map to similar luma,
    // causing missed banding in single-channel gradients.
    let channel_weights = [
        crate::constants::RGB_LUMINANCE_WEIGHT_R,
        crate::constants::RGB_LUMINANCE_WEIGHT_G,
        crate::constants::RGB_LUMINANCE_WEIGHT_B,
    ]; // R, G, B
    let mut total_score = 0.0f64;

    for (ch, &weight) in channel_weights.iter().enumerate() {
        let mut banding_score = 0.0f64;
        let mut gradient_regions = 0u32;

        for y in (0..height).step_by(crate::constants::BANDING_SCAN_STEP) {
            let mut prev_val = i16::from(rgba.get_pixel(0, y)[ch]);
            let mut gradient_length = 0u32;
            let mut step_count = 0u32;
            let mut last_step_x = 0u32;

            for x in 1..width {
                let val = i16::from(rgba.get_pixel(x, y)[ch]);
                let diff = (val - prev_val).abs();

                if diff > 0 && diff < crate::constants::PNG_BANDING_DIFF_THRESHOLD {
                    gradient_length += 1;
                    // Require step width > 3px to reduce false positives on natural gradients
                    if diff > crate::constants::BANDING_DIFF_MIN
                        && x - last_step_x > crate::constants::PNG_BANDING_STEP_WIDTH_THRESHOLD
                    {
                        step_count += 1;
                        last_step_x = x;
                    }
                } else if gradient_length > crate::constants::PNG_BANDING_GRADIENT_LENGTH_THRESHOLD
                {
                    if step_count > 0 {
                        let step_ratio = f64::from(step_count) / f64::from(gradient_length);
                        if step_ratio > crate::constants::PNG_BANDING_RATIO_LOW
                            && step_ratio < crate::constants::PNG_BANDING_RATIO_HIGH
                        {
                            banding_score += step_ratio;
                            gradient_regions += 1;
                        }
                    }
                    gradient_length = 0;
                    step_count = 0;
                    last_step_x = x;
                }
                prev_val = val;
            }
        }

        let ch_score = if gradient_regions > 0 {
            (banding_score / f64::from(gradient_regions)).min(1.0)
        } else {
            0.0_f64
        };
        total_score = f64::mul_add(ch_score, weight, total_score);
    }

    // Diagonal scan on luma for efficiency — catches diagonal gradients
    let gray = img.to_luma8();
    let mut diag_banding = 0.0f64;
    let mut diag_regions = 0u32;
    let diag_step: usize = crate::constants::BANDING_DIAG_SCAN_STEP;

    for start_offset in (0..width.max(height)).step_by(diag_step) {
        // Top-left to bottom-right diagonals from top edge
        if start_offset < width {
            let mut x = start_offset;
            let mut y = 0u32;
            let mut prev_val = i16::from(gray.get_pixel(x, y)[0]);
            let mut grad_len = 0u32;
            let mut steps = 0u32;

            while {
                x += 1;
                y += 1;
                x < width && y < height
            } {
                let val = i16::from(gray.get_pixel(x, y)[0]);
                let diff = (val - prev_val).abs();
                if diff > 0 && diff < crate::constants::PNG_BANDING_DIFF_THRESHOLD {
                    grad_len += 1;
                    if diff > crate::constants::BANDING_DIFF_MIN {
                        steps += 1;
                    }
                } else if grad_len > crate::constants::PNG_BANDING_GRADIENT_LENGTH_THRESHOLD
                    && steps > 0
                {
                    let r = f64::from(steps) / f64::from(grad_len);
                    if r > crate::constants::PNG_BANDING_RATIO_LOW
                        && r < crate::constants::PNG_BANDING_RATIO_HIGH
                    {
                        diag_banding += r;
                        diag_regions += 1;
                    }
                    grad_len = 0;
                    steps = 0;
                } else {
                    grad_len = 0;
                    steps = 0;
                }
                prev_val = val;
            }
        }

        // Top-right to bottom-left diagonals from top edge
        if start_offset < width && start_offset > 0 {
            let mut x = start_offset;
            let mut y = 0u32;
            let mut prev_val = i16::from(gray.get_pixel(x, y)[0]);
            let mut grad_len = 0u32;
            let mut steps = 0u32;

            while x > 0 && y + 1 < height {
                x -= 1;
                y += 1;
                let val = i16::from(gray.get_pixel(x, y)[0]);
                let diff = (val - prev_val).abs();
                if diff > 0 && diff < crate::constants::PNG_BANDING_DIFF_THRESHOLD {
                    grad_len += 1;
                    if diff > 3 {
                        steps += 1;
                    }
                } else if grad_len > crate::constants::PNG_BANDING_GRADIENT_LENGTH_THRESHOLD
                    && steps > 0
                {
                    let r = f64::from(steps) / f64::from(grad_len);
                    if r > crate::constants::PNG_BANDING_RATIO_LOW
                        && r < crate::constants::PNG_BANDING_RATIO_HIGH
                    {
                        diag_banding += r;
                        diag_regions += 1;
                    }
                    grad_len = 0;
                    steps = 0;
                } else {
                    grad_len = 0;
                    steps = 0;
                }
                prev_val = val;
            }
        }
    }

    let diag_score = if diag_regions > 0 {
        (diag_banding / f64::from(diag_regions)).min(1.0)
    } else {
        0.0_f64
    };

    // Combine: per-channel horizontal + diagonal luma
    total_score
        .mul_add(
            crate::constants::PNG_BANDING_WEIGHT_HORIZONTAL,
            diag_score * crate::constants::PNG_BANDING_WEIGHT_DIAGONAL,
        )
        .min(1.0)
}

fn estimate_uncompressed_size(info: &PngStructureInfo) -> u64 {
    let bits_per_sample: u64 = match info.color_type {
        0 | 3 => 1, // grayscale (0) or indexed (3): 1 channel/index
        2 => 3,     // RGB: 3 channels
        4 => 2,     // grayscale + alpha: 2 channels
        _ => 4,     // RGBA (6) or unknown: 4 channels
    };

    // bit_depth applies per sample; for sub-byte depths (1, 2, 4) pixels are packed
    let total_bits = u64::from(info.width)
        * u64::from(info.height)
        * bits_per_sample
        * u64::from(info.bit_depth);
    // Round up to bytes
    total_bits.div_ceil(8)
}

fn try_measure_entropy_via_ffmpeg(path: &Path) -> Result<f64> {
    use crate::builder_base::ToolBuilder;

    // Extract a 64x64 thumbnail to measure approximate entropy
    let output = FfmpegBuilder::new()
        .input(path)
        .frames_v(1)
        .arg("-vf")
        .arg("scale=64:64")
        .format("rawvideo")
        .pix_fmt(crate::ffmpeg_builder::PixFmt::Rgb24)
        .output_pipe()
        .build()
        .output()?;

    if !output.status.success() || output.stdout.len() < 64 * 64 * 3 {
        return Err(ImgQualityError::ImageReadError(
            "FFmpeg entropy recovery failed".to_string(),
        ));
    }

    if let Some(img_buf) = image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(64, 64, output.stdout)
    {
        let img = image::DynamicImage::ImageRgb8(img_buf);
        return calculate_entropy(&img).map_err(|e| ImgQualityError::AnalysisError(e.to_string()));
    }

    Err(ImgQualityError::ImageReadError(
        "Failed to wrap recovered buffer".to_string(),
    ))
}

/// # Errors
/// Returns an error if the image data is corrupted and pixels cannot be
/// accessed. # Panics
/// Panics if the image data is corrupted and pixels cannot be accessed.
pub fn calculate_entropy(img: &DynamicImage) -> anyhow::Result<f64> {
    let gray = img.to_luma8();
    let mut histogram = [0u64; 256];

    for pixel in gray.pixels() {
        if let Some(h) =
            histogram.get_mut(usize::from(pixel.0.first().copied().ok_or_else(|| {
                anyhow::anyhow!("Histogram bin corruption: empty pixel data")
            })?))
        {
            *h += 1;
        }
    }

    let total =
        crate::numeric_cast::u64_to_f64(crate::numeric_cast::usize_to_u64(gray.pixels().len()))
            .max(1.0);
    let mut entropy = 0.0_f64;

    for &count in &histogram {
        if count > 0 {
            let p = crate::numeric_cast::u64_to_f64(count) / total;
            entropy = p.mul_add(-p.log2(), entropy);
        }
    }

    Ok(entropy)
}

/// Per-channel RGB entropy — avoids the grayscale projection problem where
/// perceptually distinct colors (e.g. red vs blue) map to similar luma values,
/// inflating entropy and masking quantization artifacts.
/// Returns the mean of R, G, B channel entropies.
/// Palette-index frequency entropy for indexed PNG.
///
/// Counts how many pixels use each palette index (`0..palette_size`), computes
/// Shannon entropy H = -Σ freq\[i\]*log2(freq\[i\]), and returns (H, `max_H`,
/// ratio). Quantized images have uneven palette usage (few dominant entries) →
/// low ratio. Natural palette art uses entries more uniformly → ratio close to
/// 1.0.
fn calculate_palette_index_entropy(img: &DynamicImage, palette_size: usize) -> (f64, f64, f64) {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let total = crate::numeric_cast::u64_to_f64(u64::from(width) * u64::from(height));
    if crate::numeric_cast::is_effectively_zero(
        total,
        crate::numeric_cast::FloatContext::Accumulation,
    ) || palette_size == 0
    {
        return (0.0, 0.0, 0.0);
    }

    // Map each pixel to its nearest palette index by building a color→index lookup.
    // Since we don't have direct access to the raw index buffer through the `image`
    // crate (it decodes to RGBA), we approximate by quantizing to unique RGBA
    // values and counting.
    let mut color_freq: HashMap<[u8; 4], u64> = HashMap::new();
    for pixel in rgba.pixels() {
        let key = pixel.0;
        *color_freq.entry(key).or_insert(0) += 1;
    }

    // Compute entropy over the frequency distribution of distinct colors
    let mut entropy = 0.0_f64;
    for &count in color_freq.values() {
        if count > 0 {
            let p = crate::numeric_cast::u64_to_f64(count) / total;
            entropy = p.mul_add(-p.log2(), entropy);
        }
    }

    let max_entropy =
        crate::numeric_cast::u64_to_f64(crate::numeric_cast::usize_to_u64(palette_size))
            .max(1.0)
            .log2();
    let ratio = if max_entropy > 0.0_f64 {
        entropy / max_entropy
    } else {
        0.0_f64
    };

    (entropy, max_entropy, ratio)
}

fn calculate_rgb_entropy(img: &DynamicImage) -> f64 {
    fn channel_entropy(hist: &[u64; 256], total: f64) -> f64 {
        let mut h = 0.0_f64;
        for &count in hist {
            if count > 0 {
                let p = crate::numeric_cast::u64_to_f64(count) / total;
                h = p.mul_add(-p.log2(), h);
            }
        }
        h
    }
    let rgba = img.to_rgba8();
    let mut hist_r = [0u64; 256];
    let mut hist_g = [0u64; 256];
    let mut hist_b = [0u64; 256];

    for pixel in rgba.pixels() {
        let [r, g, b, _] = pixel.0;
        if let Some(h) = hist_r.get_mut(usize::from(r)) {
            *h += 1;
        }
        if let Some(h) = hist_g.get_mut(usize::from(g)) {
            *h += 1;
        }
        if let Some(h) = hist_b.get_mut(usize::from(b)) {
            *h += 1;
        }
    }

    let total =
        crate::numeric_cast::u64_to_f64(crate::numeric_cast::usize_to_u64(rgba.pixels().len()))
            .max(1.0);

    let er = channel_entropy(&hist_r, total);
    let eg = channel_entropy(&hist_g, total);
    let eb = channel_entropy(&hist_b, total);

    (er + eg + eb) / crate::constants::CHANNELS_COUNT_F64
}

/// Perform comprehensive image detection — format, compression, animation, and
/// quality.
///
/// # Errors
/// Returns an error if the file cannot be read, the format is unrecognized, or
/// analysis fails.
// Rationale: This function handles complex, sequential initialization or business logic where
// further fragmentation would hinder readability and maintainability.
/// Detects the media type and characteristics of an image file.
///
/// # Panics
///
/// Panics if the frame count logic encounters an internal consistency error or
/// if certain numeric conversions fail unexpectedly during metadata extraction.
pub fn detect_image(path: &Path) -> Result<DetectionResult> {
    let file_size = std::fs::metadata(path)?.len();

    let format = detect_format_from_bytes(path)?;

    let (is_animated, frame_count, fps) = detect_animation(path, &format)?;

    let compression = detect_compression(&format, path)?;

    let (img_data, read_error) = match open_image_with_limits(path) {
        Ok(img) => (Some(img), None),
        Err(e) => {
            crate::media_conversion_gate::probe_layer_audit(
                "probe_image_detection",
                path,
                format!(
                    "IMAGE DECODE AUDIT: Primary decode failed for '{}' | Forensic: Error '{}'; \
                     attempting secondary recovery via direct bitstream analysis \
                     (identify/ffprobe fallback)",
                    path.display(),
                    e
                ),
            );
            (None, Some(e))
        }
    };

    let (width, height, has_alpha, recovered_bit_depth, entropy) = if let Some(img) = img_data {
        let (w, h) = img.dimensions();
        let alpha = img.color().has_alpha();
        let ent = calculate_entropy(&img)?;
        (w, h, alpha, None, Some(ent))
    } else {
        // Honest recovery: Extract REAL data from bitstream using identify.
        let media_info = crate::conversion::media_info_without_ffprobe(path)
            .map_err(|err| {
                tracing::error!(
                    file = %path.display(),
                    "Secondary recovery failed: bitstream fallback probe failed: {err}"
                );
                ImgQualityError::AnalysisError(format!(
                    "Secondary recovery bitstream probe failed for {}: {err}",
                    path.display()
                ))
            })?
            .ok_or_else(|| {
                tracing::error!(
                    file = %path.display(),
                    "Secondary recovery failed: could not determine REAL media properties via bitstream fallback"
                );
                crate::media_conversion_gate::probe_image_decode_failure_or_unknown(
                    read_error,
                    "detect_image:secondary_recovery",
                )
            })?;
        let (Some(channel_type), Some(depth)) = (media_info.channel_type, media_info.bit_depth)
        else {
            return Err(ImgQualityError::AnalysisError(format!(
                "Secondary recovery produced only partial metadata for {}: dimensions were \
                 available but channel_type/bit_depth were not measured",
                path.display()
            )));
        };
        let w = media_info.width;
        let h = media_info.height;

        // channel_type string comes directly from ImageMagick (e.g. 'srgba', 'graya').
        // If it contains 'a', Alpha is physically present in the bitstream.
        let alpha = channel_type.contains('a');

        // Secondary recovery for entropy: measure from a small FFmpeg-decoded buffer
        // to avoid 'entropy is unmeasurable' failures for HEIC/AVIF/JXL.
        let recovered_entropy = match try_measure_entropy_via_ffmpeg(path) {
            Ok(entropy) => Some(entropy),
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "secondary_entropy_probe_failed",
                    path,
                    format!("secondary entropy measurement unavailable: {err}"),
                );
                None
            }
        };

        (w, h, alpha, Some(depth), recovered_entropy)
    };

    let mut precision = PrecisionMetadata {
        is_lossless_deterministic: matches!(format, DetectedFormat::PNG),
        ..PrecisionMetadata::default()
    };

    match format {
        DetectedFormat::PNG => {
            let data = std::fs::read(path)?;
            let mut cursor = std::io::Cursor::new(&data);
            let info = parse_png_structure(&mut cursor)?;
            precision.bit_depth = Some(info.bit_depth);
            precision.palette_size = info.palette_size;
            precision.color_type = Some(info.color_type);

            if compression == CompressionType::Lossy {
                cursor.set_position(0);
                let quant = PngQuantizationSession::new(info, Some(path)).analyze(&mut cursor)?;
                precision.quality_estimate = Some(estimate_png_quantized_quality(
                    precision.palette_size,
                    entropy,
                    &quant.factor_scores,
                    quant.confidence,
                )?);
            }
        }
        DetectedFormat::GIF => {
            precision = parse_gif_precision_metadata(path)?;
        }
        DetectedFormat::TIFF => {
            let comp = detect_tiff_compression(path)?;
            precision.is_lossless_deterministic = comp == CompressionType::Lossless;
            // TIFF bit depth is usually in Tag 258, but Image crate handles
            // basic ones. For now, we flag deterministic lossless.
        }
        DetectedFormat::WebP => {
            precision.bit_depth = Some(8);
            let data = std::fs::read(path)?;
            if crate::image_formats::webp::is_animated_from_bytes(&data) {
                let comp = detect_webp_animation_compression(&data)?;
                precision.is_lossless_deterministic = comp == CompressionType::Lossless;
            } else {
                precision.is_lossless_deterministic =
                    crate::image_formats::webp::is_lossless_from_bytes(&data)?;
            }
        }
        DetectedFormat::JPEG => {
            precision.bit_depth = match crate::conversion::jpeg_precision_from_header(path) {
                Ok(Some(v)) => Some(v),
                Ok(None) => measured_bit_depth_for_format(path, &format),
                Err(err) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "jpeg_precision_metadata_probe_failed",
                        path,
                        format!("JPEG precision metadata probe failed: {err}"),
                    );
                    None
                }
            };
            precision.is_lossless_deterministic = false;
            precision.quality_estimate = match estimate_jpeg_quality(path) {
                Ok(q) => Some(q),
                Err(e) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "checkpoint_progress",
                        path,
                        format!(
                            "JPEG QUALITY AUDIT: Failed to estimate quality for '{}' | Forensic: \
                             Error '{}'; refusing to forge data; information invalidated to \
                             prevent downstream precision loss",
                            path.display(),
                            e
                        ),
                    );
                    None
                }
            };
        }
        DetectedFormat::HEIC | DetectedFormat::HEIF => {
            let comp = detect_heic_compression(path)?;
            precision.is_lossless_deterministic = comp == CompressionType::Lossless;
        }
        DetectedFormat::AVIF => {
            let comp = detect_avif_compression(path)?;
            precision.is_lossless_deterministic = comp == CompressionType::Lossless;
        }
        _ => {}
    }

    if precision.bit_depth.is_none() {
        precision.bit_depth = measured_bit_depth_for_format(path, &format).or(recovered_bit_depth);
    }

    let mut estimated_quality = if format == DetectedFormat::JPEG
        || (format == DetectedFormat::WebP && compression == CompressionType::Lossy)
        || (format == DetectedFormat::PNG && compression == CompressionType::Lossy)
    {
        precision.quality_estimate
    } else {
        None
    };

    if crate::algorithm_runtime::image_quality_heuristic_enabled()
        && estimated_quality.is_none()
        && compression == CompressionType::Lossy
    {
        let quality_frame_count = if is_animated {
            if let Some(count) = frame_count {
                count
            } else {
                crate::media_conversion_gate::probe_layer_audit(
                    "delivery_db_metadata",
                    path,
                    format!(
                        "ANIMATION AUDIT: Lossy animated image is missing frame_count at '{}' | \
                         Forensic: Mandatory metadata missing; refusing to forge data to prevent \
                         upstream calculation forgery",
                        path.display()
                    ),
                );
                return Err(ImgQualityError::AnalysisError(format!(
                    "Cannot estimate quality for lossy animated {}: missing frame count",
                    format.as_str()
                )));
            }
        } else {
            1
        };
        let val = estimate_lossy_quality_fallback(
            path,
            &format,
            width,
            height,
            file_size,
            quality_frame_count,
            entropy,
        );
        match val {
            Ok(q) => estimated_quality = Some(q),
            Err(e) => {
                if crate::algorithm_runtime::image_quality_heuristic_enabled() {
                    return Err(e);
                }
                tracing::debug!(
                    "Heuristic quality estimation failed (skipped under fallback-disabled): {e}"
                );
            }
        }
    }

    let duration = if is_animated {
        if let (Some(fc), Some(f)) = (frame_count, fps) {
            Some(crate::numeric_cast::f64_to_f32_lossy(f64::from(fc)) / f)
        } else {
            crate::media_conversion_gate::probe_layer_audit(
                "delivery_db_numeric",
                path,
                format!(
                    "ANIMATION AUDIT: Animated image missing frame_count/fps at '{}' | Forensic: \
                     Duration calculation impossible without both; skipping to prevent numeric \
                     forgery",
                    path.display()
                ),
            );
            None
        }
    } else {
        None
    };

    let mut result = DetectionResult {
        file_path: path.display().to_string(),
        format,
        image_type: if is_animated {
            ImageType::Animated
        } else {
            ImageType::Static
        },
        compression,
        width,
        height,
        bit_depth: precision.bit_depth,
        has_alpha,
        file_size,
        frame_count,
        fps,
        duration,
        estimated_quality,
        entropy,
        precision,
    };

    if result.compression == CompressionType::Lossy {
        if is_animated {
            let analysis = crate::image_analyzer::ImageAnalysis {
                file_path: result.file_path.clone(),
                format: result.format.as_str().to_string(),
                width: result.width,
                height: result.height,
                file_size: result.file_size,
                color_depth: result.bit_depth,
                has_alpha: result.has_alpha,
                is_animated: true,
                is_lossless: false,
                features: crate::image_analyzer::ImageFeatures {
                    entropy: result.entropy,
                    compression_ratio: None,
                },
                precision: result.precision.clone(),
                ..Default::default()
            };
            if crate::algorithm_runtime::quality_db_lookup_enabled("image_detection_animated")
                && let Some(quality_prediction) =
                    crate::image_quality_db::lookup_image_quality_with_path(&analysis, Some(path))
            {
                result.estimated_quality =
                    crate::image_quality_db::fuse_quality_regression_prediction_if_enabled(
                        "image_detection_animated",
                        result.estimated_quality,
                        quality_prediction,
                    );
            }
        } else if let Some(ent) = result.entropy {
            let analysis = crate::image_analyzer::ImageAnalysis {
                file_path: result.file_path.clone(),
                format: result.format.as_str().to_string(),
                width: result.width,
                height: result.height,
                file_size: result.file_size,
                color_depth: result.bit_depth,
                has_alpha: result.has_alpha,
                is_animated: false,
                is_lossless: false,
                features: crate::image_analyzer::ImageFeatures {
                    entropy: Some(ent),
                    compression_ratio: result.bit_depth.map(|bit_depth| {
                        (f64::from(result.width) * f64::from(result.height) * f64::from(bit_depth)
                            / 8.0)
                            / crate::numeric_cast::u64_to_f64(result.file_size).max(1.0)
                    }),
                },
                precision: result.precision.clone(),
                ..Default::default()
            };

            if crate::algorithm_runtime::quality_db_lookup_enabled("image_detection_static")
                && let Some(quality_prediction) =
                    crate::image_quality_db::lookup_image_quality_with_path(&analysis, Some(path))
            {
                result.estimated_quality =
                    crate::image_quality_db::fuse_quality_regression_prediction_if_enabled(
                        "image_detection_static",
                        result.estimated_quality,
                        quality_prediction,
                    );
            }
        }
    }

    verify_transparency_claim(path, &mut result);
    reconcile_animated_frame_count(path, frame_count, is_animated, &mut result)?;

    Ok(result)
}

fn verify_transparency_claim(path: &Path, result: &mut DetectionResult) {
    // Transparency penetration requires duration for stratified sampling.
    // Static images (duration=None) skip this check entirely - their alpha is
    // always real.
    let Some(duration) = result.duration else {
        return;
    };
    if result.has_alpha
        && let crate::media_penetration::PenetrationResult::Verified(is_real) =
            crate::media_penetration::detect_real_transparency(path, Some(f64::from(duration)))
        && !is_real
    {
        crate::media_conversion_gate::ui_penetration_warning_stderr(
            &crate::media_conversion_gate::path_file_name_for_log(path),
            "Image transparency penetration: FAKE alpha (unused)",
        );
        result.has_alpha = false;
    }
}

fn frame_count_claim_for_penetration(path: &Path, frame_count: Option<u32>) -> Option<u64> {
    if frame_count.is_none() {
        crate::media_conversion_gate::probe_layer_audit(
            "delivery_db_metadata",
            path,
            format!(
                "ANIMATION AUDIT: Missing frame_count for '{}' before penetration check | \
                 Forensic: Value is None; preserving unknown claim and forcing exhaustive \
                 bitstream verification",
                path.display()
            ),
        );
    }
    frame_count.map(u64::from)
}

fn display_file_name_for_log(path: &Path) -> String {
    crate::media_conversion_gate::probe_path_file_name_for_log(path)
}

fn reconcile_animated_frame_count(
    path: &Path,
    frame_count: Option<u32>,
    is_animated: bool,
    result: &mut DetectionResult,
) -> Result<()> {
    if !is_animated {
        return Ok(());
    }

    let claimed_count = frame_count_claim_for_penetration(path, frame_count);
    if claimed_count.is_some_and(|claimed| {
        claimed > crate::constants::FRAME_COUNT_TRUST_LOWER_LIMIT
            && claimed <= crate::constants::FRAME_COUNT_TRUST_UPPER_LIMIT
    }) {
        return Ok(());
    }

    if let crate::media_penetration::PenetrationResult::Verified(real_count) =
        crate::media_penetration::detect_real_frame_count(path, claimed_count)
    {
        let real_u32 = u32::try_from(real_count).map_err(|_| {
            ImgQualityError::AnalysisError(format!(
                "Penetration failure: Real frame count {} for {} exceeds u32::MAX",
                real_count,
                path.display()
            ))
        })?;
        if Some(real_u32) != frame_count {
            crate::media_conversion_gate::ui_penetration_warning_stderr(
                &display_file_name_for_log(path),
                format!(
                    "Image frame count mismatch: metadata={}, actual={}, correcting",
                    crate::media_conversion_gate::ui_optional_u32_display_or_unknown(frame_count),
                    real_u32
                ),
            );
            result.frame_count = Some(real_u32);
        }
    }

    Ok(())
}

fn estimate_png_quantized_quality(
    palette_size: Option<usize>,
    entropy: Option<f64>,
    factor_scores: &PngQuantizationFactors,
    confidence: Option<f64>,
) -> Result<u8> {
    let entropy_norm = entropy
        .filter(|e| e.is_finite() && *e > 0.0)
        .map(|e| (e / 8.0).clamp(0.0, 1.0));

    let quality_signal = match palette_size {
        None => {
            let factor_avg = (factor_scores.dithering_detected
                + factor_scores.color_count_anomaly
                + factor_scores.gradient_banding
                + factor_scores.color_frequency_distribution)
                / 4.0;
            let inverse_factor = 1.0 - factor_avg.clamp(0.0, 1.0);
            match entropy_norm {
                Some(entropy_norm) => entropy_norm.mul_add(
                    crate::constants::PNG_QUALITY_TRUECOLOR_ENTROPY_WEIGHT,
                    inverse_factor * crate::constants::PNG_QUALITY_TRUECOLOR_FACTOR_WEIGHT,
                ),
                None => inverse_factor,
            }
        }
        Some(ps) => {
            let ps_clamped = crate::numeric_cast::usize_to_f64(ps.max(2)).min(256.0);
            let palette_signal = ps_clamped.log2() / crate::constants::PNG_QUALITY_PALETTE_LOG_BASE;
            match entropy_norm {
                Some(entropy_norm) => entropy_norm.mul_add(
                    crate::constants::PNG_QUALITY_ENTROPY_WEIGHT,
                    palette_signal * crate::constants::PNG_QUALITY_PALETTE_WEIGHT,
                ),
                None => palette_signal,
            }
        }
    };

    let adjusted = match confidence {
        Some(c) => {
            let conf_penalty = (1.0 - c) * 0.1;
            (quality_signal + conf_penalty).clamp(0.0, 1.0)
        }
        None => quality_signal.clamp(0.0, 1.0),
    };

    let range = crate::constants::PNG_QUALITY_EST_MAX - crate::constants::PNG_QUALITY_EST_MIN;
    let q = f64::mul_add(adjusted, range, crate::constants::PNG_QUALITY_EST_MIN);
    let result = crate::numeric_cast::f64_to_u8_strict(q.round(), "png_estimated_quality")
        .ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "Cannot estimate PNG quantized quality: rounded quality {q} is not representable \
                 as u8"
            ))
        })?;
    let min_quality =
        crate::numeric_cast::f64_to_u8_strict(crate::constants::PNG_QUALITY_EST_MIN, "png_min")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "PNG quality minimum {} is not representable as u8",
                    crate::constants::PNG_QUALITY_EST_MIN
                ))
            })?;
    let max_quality =
        crate::numeric_cast::f64_to_u8_strict(crate::constants::PNG_QUALITY_EST_MAX, "png_max")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "PNG quality maximum {} is not representable as u8",
                    crate::constants::PNG_QUALITY_EST_MAX
                ))
            })?;
    Ok(result.clamp(min_quality, max_quality))
}

fn estimate_lossy_quality_fallback(
    path: &Path,
    format: &DetectedFormat,
    width: u32,
    height: u32,
    file_size: u64,
    frame_count: u32,
    entropy: Option<f64>,
) -> Result<u8> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || file_size == 0 {
        if crate::algorithm_runtime::image_quality_heuristic_enabled() {
            crate::progress_mode::emit_stderr(&format!(
                "   \x1b[1;31m🚨 [CRITICAL FALLBACK]\x1b[0m \x1b[31mQuality detection failed and \
                     heuristic fallback is impossible.\x1b[0m\n\x1b[31m      File: \
                     {}\x1b[0m\n\x1b[31m      Refusing to invent a hardcoded quality value.\x1b[0m",
                path.display()
            ));
        }
        return Err(ImgQualityError::AnalysisError(format!(
            "Cannot estimate quality for lossy {}: invalid dimensions ({width}x{height}) or empty \
             file",
            format.as_str()
        )));
    }

    // Entropy is required: without it, the BPP-only heuristic collapses to a
    // format-efficiency-only formula that saturates to Q=100 on modern codecs
    // (AVIF/HEIC at 3.0x efficiency). Refuse rather than forge a verdict.
    let Some(entropy) = entropy.filter(|e| e.is_finite() && *e > 0.0) else {
        if crate::algorithm_runtime::image_quality_heuristic_enabled() {
            crate::progress_mode::emit_stderr(&format!(
                "   \x1b[1;31m🚨 [CRITICAL FALLBACK]\x1b[0m \x1b[31mQuality detection failed; entropy \
                 is unmeasurable so heuristic refuses to invent a value.\x1b[0m\n\x1b[31m      File: \
                 {}\x1b[0m",
                path.display()
            ));
        }
        return Err(ImgQualityError::AnalysisError(format!(
            "Cannot estimate quality for lossy {}: entropy unavailable (decode failed)",
            format.as_str()
        )));
    };

    // Heuristic v2: Multi-factor quality estimation
    let raw_bpp = crate::numeric_cast::u64_to_f64(file_size) * crate::constants::BITS_PER_BYTE
        / crate::numeric_cast::u64_to_f64(pixels.max(1))
        / f64::from(frame_count.max(1));

    // Format efficiency multiplier (relative to JPEG)
    // AVIF/HEIC ~ 3.0x, WebP ~ 1.5x
    let efficiency_factor = match format {
        DetectedFormat::AVIF | DetectedFormat::HEIC | DetectedFormat::HEIF => {
            crate::constants::AVIF_EFFICIENCY_FACTOR
        }
        DetectedFormat::WebP => crate::constants::WEBP_EFFICIENCY_FACTOR,
        _ => crate::constants::JPEG_EFFICIENCY_FACTOR,
    };

    // Entropy compensation:
    // High entropy (>7.5) means complex texture, needs more BPP for same quality
    // Low entropy (<4.0) means flat colors, quality is higher even with low BPP
    let entropy_adj = (crate::constants::ENTROPY_QUALITY_BASE / entropy.max(1.0))
        .sqrt()
        .clamp(
            crate::constants::ENTROPY_ADJ_MIN,
            crate::constants::ENTROPY_ADJ_MAX,
        );

    let effective_bpp = raw_bpp * efficiency_factor * entropy_adj;
    // Calibrated formula for multi-format heuristic:
    // 12 * log2(effective_bpp * 1.5) + 60
    // Results: 0.2 bpp -> ~39, 1.0 bpp -> ~67, 5.0 bpp -> ~95, 10.0 bpp -> 100
    let clamped_quality = crate::constants::QUALITY_EST_BPP_LOG_SCALE
        .mul_add(
            (effective_bpp * crate::constants::JXL_DISTANCE_EST_BPP_FACTOR)
                .max(crate::constants::LOG2_SAFETY_FLOOR)
                .log2(),
            crate::constants::JXL_DISTANCE_EST_OFFSET,
        )
        .clamp(
            crate::constants::QUALITY_EST_MIN,
            crate::constants::QUALITY_EST_MAX,
        );
    let Some(bpp_quality) = crate::numeric_cast::f64_to_u8_strict(clamped_quality, "bpp_quality")
    else {
        return Err(ImgQualityError::AnalysisError(format!(
            "Cannot estimate quality for lossy {}: clamped heuristic value {clamped_quality} is \
             not representable as u8",
            format.as_str()
        )));
    };

    crate::progress_mode::emit_stderr(&format!(
        "   \x1b[1;33m⚠️  [QUALITY FALLBACK]\x1b[0m \x1b[33mExact detection unavailable for {} \
         codec.\x1b[0m\n\x1b[33m      File: {}\x1b[0m\n\x1b[33m      Heuristic: BPP={:.3}, \
         Eff={:.1}x, Entropy={:.2} -> \x1b[1;32mEstimated Q: {}\x1b[0m",
        format.as_str(),
        path.display(),
        raw_bpp,
        efficiency_factor,
        entropy,
        bpp_quality
    ));

    Ok(bpp_quality)
}

fn estimate_jpeg_quality(path: &Path) -> Result<u8> {
    use crate::image_jpeg_analysis::analyze_jpeg_quality;
    let data = std::fs::read(path)?;
    let analysis = analyze_jpeg_quality(&data).map_err(ImgQualityError::AnalysisError)?;
    Ok(analysis.estimated_quality)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ApngTimingStats {
    pub frame_count: u32,
    pub duration_secs: f64,
    pub fps: f64,
}

/// Aggregate APNG timing from `fcTL` frame delays and `acTL` frame count.
#[must_use]
pub(crate) fn apng_timing_stats_from_bytes(data: &[u8]) -> Option<ApngTimingStats> {
    let info = match crate::image::png_validation::parse_apng_animation(data) {
        Ok(info) => info?,
        Err(error) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "apng_timing_parse_failed",
                format!("APNG timing parse failed: {error}"),
            );
            return None;
        }
    };
    let frame_count = info.frame_count;
    let duration_secs = info.duration_secs;
    if frame_count <= 1 {
        return None;
    }

    if !duration_secs.is_finite() || duration_secs <= f64::EPSILON {
        return None;
    }

    let fps = f64::from(frame_count) / duration_secs;
    if !fps.is_finite() || fps <= 0.0_f64 {
        return None;
    }

    Some(ApngTimingStats {
        frame_count,
        duration_secs,
        fps,
    })
}

/// Minimal animated PNG with two `fcTL` frames (10 ms + 20 ms) for unit tests
/// only.
#[cfg(test)]
pub(crate) fn synthetic_two_frame_apng_for_test() -> Vec<u8> {
    fn png_chunk(chunk_type: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::with_capacity(12 + payload.len());
        chunk.extend_from_slice(
            &u32::try_from(payload.len())
                .expect("test apng chunk payload fits u32")
                .to_be_bytes(),
        );
        chunk.extend_from_slice(chunk_type);
        chunk.extend_from_slice(payload);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(chunk_type);
        hasher.update(payload);
        chunk.extend_from_slice(&hasher.finalize().to_be_bytes());
        chunk
    }

    fn fctl_chunk(sequence: u32, delay_num: u16, delay_den: u16) -> Vec<u8> {
        let mut payload = vec![0u8; 26];
        payload[0..4].copy_from_slice(&sequence.to_be_bytes());
        payload[7] = 1;
        payload[11] = 1;
        payload[20] = crate::numeric_cast::u16_high8_to_u8(delay_num);
        payload[21] = crate::numeric_cast::u16_low8_to_u8(delay_num);
        payload[22] = crate::numeric_cast::u16_high8_to_u8(delay_den);
        payload[23] = crate::numeric_cast::u16_low8_to_u8(delay_den);
        png_chunk(b"fcTL", &payload)
    }

    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let ihdr = [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0];
    data.extend(png_chunk(b"IHDR", &ihdr));
    data.extend(png_chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]));
    data.extend(fctl_chunk(0, 1, 100));
    data.extend(png_chunk(
        b"IDAT",
        &[0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01],
    ));
    data.extend(fctl_chunk(1, 2, 100));
    let mut second_frame = 2u32.to_be_bytes().to_vec();
    second_frame.extend_from_slice(&[0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01]);
    data.extend(png_chunk(b"fdAT", &second_frame));
    data.extend(png_chunk(b"IEND", &[]));
    data
}

// ============================================================================
// Enhanced Format-Specific Lossless Detection
// ============================================================================

/// Detect WebP animated compression by traversing all ANMF (animation frame)
/// chunks.
fn detect_webp_animation_compression(data: &[u8]) -> Result<CompressionType> {
    if crate::image_formats::webp::detect_webp_animation_is_lossless(data)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect TIFF compression type — traverses ALL IFDs. Supports both standard
/// TIFF and `BigTIFF`.
fn detect_tiff_compression(path: &Path) -> Result<CompressionType> {
    let is_lossless = crate::image_formats::tiff_family::is_lossless_tiff_family(path)?;

    if is_lossless {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect AVIF compression from positive codec evidence.
fn detect_avif_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    crate::image_formats::avif::classify_compression(&data, path)
}

/// Detect HEIC/HEIF compression from positive codec evidence.
fn detect_heic_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    crate::image_heic_analysis::classify_heic_compression(&data, path)
}

/// Detect ICO compression by inspecting embedded image entries.
///
/// ICO directory: header\[6\] + entries[16 each]. Each entry has an offset to
/// image data. If image data starts with PNG magic → recursively check PNG
/// quantization. Any quantized PNG entry → Lossy. Otherwise → Lossless.
fn detect_ico_compression(path: &Path) -> Result<CompressionType> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(ImgQualityError::IoError)?;
    let file_len = file.metadata().map_err(ImgQualityError::IoError)?.len();
    if file_len > crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT {
        return Err(ImgQualityError::AnalysisError(format!(
            "ICO: file is too large ({file_len} bytes > {} max allowed)",
            crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
        )));
    }

    // ICO header: reserved(2) + type(2) + count(2) = 6 bytes
    let mut header = [0u8; 6];
    file.read_exact(&mut header).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "ICO: failed to read 6-byte header from '{}': {err}",
            path.display()
        ))
    })?;

    let image_count = usize::from(u16::from_le_bytes([header[4], header[5]]));
    if header[0..2] != [0, 0] || header[2..4] != [1, 0] || image_count == 0 {
        return Err(ImgQualityError::AnalysisError(format!(
            "ICO: invalid ICONDIR header in '{}' (reserved={}, type={}, count={image_count})",
            path.display(),
            u16::from_le_bytes([header[0], header[1]]),
            u16::from_le_bytes([header[2], header[3]]),
        )));
    }
    let directory_end = 6_u64
        .checked_add(
            u64::try_from(image_count).map_err(|error| {
                ImgQualityError::AnalysisError(format!(
                    "ICO: image count conversion failed in '{}': {error}",
                    path.display()
                ))
            })? * 16,
        )
        .filter(|end| *end <= file_len)
        .ok_or_else(|| {
            ImgQualityError::AnalysisError(format!(
                "ICO: directory for {image_count} images exceeds file length {file_len} in '{}'",
                path.display()
            ))
        })?;
    let png_magic: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    // Each directory entry is 16 bytes, starting at offset 6
    for i in 0..image_count {
        let entry_offset = 6 + crate::numeric_cast::usize_to_u64_strict(i, "ICO entry index")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!("ICO index conversion failed for entry {i}"))
            })?
            * 16;
        file.seek(SeekFrom::Start(entry_offset))
            .map_err(ImgQualityError::IoError)?;

        let mut entry = [0u8; 16];
        file.read_exact(&mut entry).map_err(|error| {
            ImgQualityError::AnalysisError(format!(
                "ICO: directory entry {i} is truncated in '{}': {error}",
                path.display()
            ))
        })?;
        if entry[3] != 0 {
            return Err(ImgQualityError::AnalysisError(format!(
                "ICO: directory entry {i} has non-zero reserved byte in '{}'",
                path.display()
            )));
        }

        // Bytes 8-11: size of image data, bytes 12-15: offset of image data
        let img_size = u64::from(u32::from_le_bytes([
            entry[8], entry[9], entry[10], entry[11],
        ]));
        let img_offset = u64::from(u32::from_le_bytes([
            entry[12], entry[13], entry[14], entry[15],
        ]));
        let img_end = img_offset
            .checked_add(img_size)
            .filter(|end| img_size >= 8 && img_offset >= directory_end && *end <= file_len)
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "ICO: entry {i} image range offset={img_offset}, size={img_size} is outside \
                     data region {directory_end}..{file_len} in '{}'",
                    path.display()
                ))
            })?;

        file.seek(SeekFrom::Start(img_offset))
            .map_err(ImgQualityError::IoError)?;
        let mut magic_peek = [0u8; 8];
        file.read_exact(&mut magic_peek).map_err(|error| {
            ImgQualityError::AnalysisError(format!(
                "ICO: failed to read entry {i} image header ending at {img_end} in '{}': {error}",
                path.display()
            ))
        })?;
        if magic_peek == png_magic {
            file.seek(SeekFrom::Start(img_offset))
                .map_err(ImgQualityError::IoError)?;
            let png_len = crate::numeric_cast::u64_to_usize_strict(img_size, "ico_img_size")
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError(format!(
                        "ICO: entry {i} image size {img_size} overflows usize in '{}'",
                        path.display()
                    ))
                })?;
            let mut png_data = vec![0; png_len];
            file.read_exact(&mut png_data).map_err(|error| {
                ImgQualityError::AnalysisError(format!(
                    "ICO: entry {i} PNG payload changed or truncated during read in '{}': {error}",
                    path.display()
                ))
            })?;
            if analyze_png_quantization_from_bytes(&png_data)?.is_quantized {
                return Ok(CompressionType::Lossy);
            }
        }
    }

    Ok(CompressionType::Lossless)
}

/// Detect `OpenEXR` compression type by parsing the header attributes.
///
/// EXR header: magic (76 2F 31 01) + version (4 bytes) + attributes until empty
/// name. Each attribute: null-terminated name + null-terminated type + size
/// (u32 LE) + value. The "compression" attribute value byte:
///   0=NONE, 1=RLE, 2=ZIPS, 3=ZIP, 4=PIZ → lossless
///   5=PXR24, 6=B44, 7=B44A, 8=DWAA, 9=DWAB → lossy
///
/// EXR 2.0 multi-part: version bit 9 = 1. Each part has independent header with
/// its own compression. Parts separated by empty name; all parts end with two
/// consecutive empty names. Any lossy part → Lossy overall.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn detect_exr_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    // Magic (4) + version (4) = 8 bytes minimum before attributes
    if data.len() < 12 || !data.starts_with(&[0x76, 0x2F, 0x31, 0x01]) {
        return Err(ImgQualityError::AnalysisError(format!(
            "EXR: invalid magic bytes or file too short in '{}'; cannot determine compression",
            path.display()
        )));
    }

    // Check version field for multi-part flag (bit 9)
    let version = data
        .get_u32_le_strict(4, "EXR version")
        .ok_or_else(|| anyhow::anyhow!("Truncated EXR version field"))?;
    let is_multipart = (version & (1 << 9_i32)) != 0;

    let mut pos = 8; // skip magic + version
    let mut found_any_compression = false;
    let mut part_count = 0_i32;

    // Scan all parts (single-part = 1 iteration, multi-part = multiple)
    loop {
        part_count += 1_i32;

        // Scan attributes in this part: each is name\0 + type\0 + size(u32 LE) + value
        // Empty name terminates the part header
        while pos < data.len() {
            // Read attribute name (null-terminated)
            let name_start = pos;
            while pos < data.len() && data.get(pos) != Some(&0) {
                pos += 1;
            }
            if pos >= data.len() {
                break;
            }
            let name = data.get(name_start..pos).ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "EXR attribute name slice missing (out of bounds)".to_string(),
                )
            })?;
            pos += 1; // skip null terminator

            // Empty name = end of this part's header
            if name.is_empty() {
                break;
            }

            // Read type name (null-terminated)
            while pos < data.len() && data.get(pos) != Some(&0) {
                pos += 1;
            }
            if pos >= data.len() {
                break;
            }
            pos += 1; // skip null terminator

            // Read value size (u32 LE)
            let value_size = crate::numeric_cast::u32_to_usize_strict(
                data.get_u32_le_strict(pos, "EXR attribute value size")
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "Truncated EXR attribute value size".to_string(),
                        )
                    })?,
                "EXR value_size",
            )
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "EXR attribute value size overflows usize".to_string(),
                )
            })?;
            pos += 4;

            if name == b"compression" && value_size >= 1 {
                let compression =
                    data.get_byte_strict(pos, "EXR compression")
                        .ok_or_else(|| {
                            ImgQualityError::AnalysisError("Missing compression value".to_string())
                        })?;
                found_any_compression = true;

                if std::env::var(crate::constants::ENV_VERBOSE).is_ok() {
                    crate::log_detail!(&format!(
                        "   📊 EXR part#{} compression: {} ({})",
                        part_count,
                        compression,
                        match compression {
                            0 => "NONE",
                            1 => "RLE",
                            2 => "ZIPS",
                            3 => "ZIP",
                            4 => "PIZ",
                            5 => "PXR24",
                            6 => "B44",
                            7 => "B44A",
                            8 => "DWAA",
                            9 => "DWAB",
                            _ => "Unknown",
                        }
                    ));
                }

                // EXR compression values:
                //  0=NONE, 1=RLE, 2=ZIPS, 3=ZIP, 4=PIZ → lossless
                //  5=PXR24, 6=B44, 7=B44A, 8=DWAA, 9=DWAB → lossy
                // Any lossy part → entire file is lossy
                if (5..=9).contains(&compression) {
                    return Ok(CompressionType::Lossy);
                }
                if compression > 9 {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "EXR: unknown compression value {compression} in '{}'; cannot determine losslessness",
                        path.display()
                    )));
                }
            }

            // Skip value
            pos += value_size;
        }

        // If not multi-part, we're done after first part
        if !is_multipart {
            break;
        }

        // Multi-part: check for second consecutive empty name (end of all parts)
        if data.get(pos) == Some(&0) {
            // Two consecutive empty names → end of multi-part file
            break;
        }

        // Continue to next part
        if pos >= data.len() {
            break;
        }
    }

    if !found_any_compression {
        return Err(ImgQualityError::AnalysisError(format!(
            "EXR: no 'compression' attribute found in '{}'; cannot determine losslessness",
            path.display()
        )));
    }
    Ok(CompressionType::Lossless)
}

/// Detect positive JPEG 2000 lossy evidence by parsing COD and COC markers.
///
/// COD (Coding style Default, FF 52) contains default `SPcod` parameters; the
/// last byte is the wavelet transform type:
///   - 0 = 9/7 irreversible (lossy)
///   - 1 = 5/3 reversible (necessary but insufficient evidence for lossless)
///
/// COC (Component-specific coding style, FF 53) can override COD for specific
/// components. For multi-component images (e.g. DICOM-JP2), if COD=9/7 but COC
/// overrides to 5/3 for a component, we need to check all components. Any lossy
/// component → Lossy overall.
///
/// For JP2 container: find the codestream inside "jp2c" box, then scan for
/// COD/COC. For raw codestream (FF 4F FF 51): scan directly.
fn detect_jp2_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if data.len() < 4 {
        return Err(ImgQualityError::AnalysisError(format!(
            "JP2: file is too short ({len} bytes) to contain a codestream — {}",
            path.display(),
            len = data.len()
        )));
    }

    let cs = if data.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        data.as_slice()
    } else {
        crate::common_utils::find_box_data_recursive(&data, *b"jp2c").ok_or_else(|| {
            ImgQualityError::AnalysisError(
                "Could not find JPEG 2000 codestream (jp2c box)".to_string(),
            )
        })?
    };

    // Resolve main-header COD/COC defaults plus first-tile overrides exactly.
    // Inspecting only the main COD is unsafe: a tile COD can replace it for
    // every component. Conversely, one effective 9/7 component in a real tile
    // is sufficient positive proof that the codestream is lossy.
    let wavelets = first_jp2_tile_wavelets(cs)?;
    for (component, wavelet) in wavelets.iter().copied().enumerate() {
        if std::env::var(crate::constants::ENV_VERBOSE).is_ok() {
            crate::log_detail!(&format!(
                "   📊 JP2 first-tile component {} wavelet: {} ({})",
                component,
                wavelet,
                if wavelet == 1 {
                    "5/3 reversible — losslessness unproven"
                } else {
                    "9/7 irreversible — lossy"
                }
            ));
        }
        // Any lossy component → entire file is lossy
        if wavelet == 0 {
            return Ok(CompressionType::Lossy);
        }
    }

    // A reversible wavelet alone does not prove a lossless codestream: QCD/QCC
    // quantization and component transforms must also be reversible. Until the
    // complete main header is proven, retain the source rather than fabricating
    // a Lossless verdict.
    Ok(CompressionType::Unknown)
}

/// Resolve the effective wavelet for every component of the first real tile.
/// Main-header parameters are copied to each tile; tile COD replaces all
/// component defaults, then tile COC replaces one component.
fn first_jp2_tile_wavelets(cs: &[u8]) -> Result<Vec<u8>> {
    if !cs.starts_with(&[0xFF, 0x4F]) {
        return Err(ImgQualityError::AnalysisError(
            "JP2: codestream does not start with SOC".to_string(),
        ));
    }

    let mut main_wavelets: Option<Vec<Option<u8>>> = None;
    let mut tile_wavelets: Option<Vec<Option<u8>>> = None;
    let mut pos = 0;
    while pos + 2 <= cs.len() {
        if cs.get(pos) != Some(&0xFF) {
            return Err(ImgQualityError::AnalysisError(format!(
                "JP2: expected marker at codestream offset {pos}"
            )));
        }
        let Some(marker) = cs.get_byte_strict(pos + 1, "JP2 marker") else {
            return Err(ImgQualityError::AnalysisError(
                "JP2: truncated marker".to_string(),
            ));
        };

        if marker == 0x4F {
            if pos != 0 {
                return Err(ImgQualityError::AnalysisError(
                    "JP2: duplicate SOC marker".to_string(),
                ));
            }
            pos += 2;
            continue;
        }
        if marker == 0xFF {
            pos += 1;
            continue;
        }
        if marker == 0x93 {
            let wavelets = tile_wavelets.ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "JP2: SOD encountered before a first-tile SOT marker".to_string(),
                )
            })?;
            return wavelets
                .into_iter()
                .enumerate()
                .map(|(component, wavelet)| {
                    wavelet.ok_or_else(|| {
                        ImgQualityError::AnalysisError(format!(
                            "JP2: no effective COD/COC wavelet for component {component}"
                        ))
                    })
                })
                .collect();
        }

        if pos + 4 > cs.len() {
            return Err(ImgQualityError::AnalysisError(format!(
                "JP2: truncated marker segment 0xff{marker:02x}"
            )));
        }
        let seg_len = usize::from(
            cs.get_u16_be_strict(pos + 2, "JP2 segment length")
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError("JP2: missing segment length".to_string())
                })?,
        );
        if seg_len < 2 {
            return Err(ImgQualityError::AnalysisError(format!(
                "JP2: invalid segment length {seg_len} for marker 0xff{marker:02x}"
            )));
        }
        let next = pos
            .checked_add(2)
            .and_then(|v| v.checked_add(seg_len))
            .ok_or_else(|| {
                ImgQualityError::AnalysisError("JP2: marker boundary overflow".to_string())
            })?;
        if next > cs.len() {
            return Err(ImgQualityError::AnalysisError(format!(
                "JP2: marker 0xff{marker:02x} exceeds codestream boundary"
            )));
        }
        let segment = crate::media_conversion_gate::probe_jpeg_buffer_slice(
            cs,
            pos..next,
            "JP2 marker segment",
        );
        if segment.len() != next - pos {
            return Err(ImgQualityError::AnalysisError(format!(
                "JP2: marker 0xff{marker:02x} slice is incomplete"
            )));
        }

        match marker {
            0x51 => {
                if main_wavelets.is_some() || tile_wavelets.is_some() || seg_len < 41 {
                    return Err(ImgQualityError::AnalysisError(
                        "JP2: invalid or duplicate SIZ marker".to_string(),
                    ));
                }
                let components = usize::from(
                    segment
                        .get_u16_be_strict(38, "JP2 SIZ component count")
                        .ok_or_else(|| {
                            ImgQualityError::AnalysisError("JP2: truncated SIZ marker".to_string())
                        })?,
                );
                let expected = 38usize
                    .checked_add(components.checked_mul(3).ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "JP2: SIZ component count overflow".to_string(),
                        )
                    })?)
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError("JP2: SIZ length overflow".to_string())
                    })?;
                if components == 0 || seg_len != expected {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "JP2: SIZ length {seg_len} does not match {components} components"
                    )));
                }
                main_wavelets = Some(vec![None; components]);
            }
            0x52 => {
                if seg_len < 12 {
                    return Err(ImgQualityError::AnalysisError(
                        "JP2: COD marker is too short".to_string(),
                    ));
                }
                let wavelet = segment
                    .get_byte_strict(13, "JP2 COD wavelet")
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError("JP2: truncated COD marker".to_string())
                    })?;
                if wavelet > 1 {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "JP2: invalid COD wavelet {wavelet}"
                    )));
                }
                let target = tile_wavelets
                    .as_mut()
                    .or(main_wavelets.as_mut())
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "JP2: COD encountered before SIZ".to_string(),
                        )
                    })?;
                target.fill(Some(wavelet));
            }
            0x53 => {
                let target = tile_wavelets
                    .as_mut()
                    .or(main_wavelets.as_mut())
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "JP2: COC encountered before SIZ".to_string(),
                        )
                    })?;
                let component_bytes = if target.len() <= 256 { 1 } else { 2 };
                let minimum = 8 + component_bytes;
                if seg_len < minimum {
                    return Err(ImgQualityError::AnalysisError(
                        "JP2: COC marker is too short".to_string(),
                    ));
                }
                let component = if component_bytes == 1 {
                    usize::from(segment.get_byte_strict(4, "JP2 COC component").ok_or_else(
                        || ImgQualityError::AnalysisError("JP2: truncated COC marker".to_string()),
                    )?)
                } else {
                    usize::from(
                        segment
                            .get_u16_be_strict(4, "JP2 COC component")
                            .ok_or_else(|| {
                                ImgQualityError::AnalysisError(
                                    "JP2: truncated COC marker".to_string(),
                                )
                            })?,
                    )
                };
                let wavelet = segment
                    .get_byte_strict(9 + component_bytes, "JP2 COC wavelet")
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError("JP2: truncated COC marker".to_string())
                    })?;
                if component >= target.len() || wavelet > 1 {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "JP2: invalid COC component/wavelet ({component}, {wavelet})"
                    )));
                }
                target[component] = Some(wavelet);
            }
            0x90 => {
                if tile_wavelets.is_some() {
                    return Err(ImgQualityError::AnalysisError(
                        "JP2: second SOT encountered before first SOD".to_string(),
                    ));
                }
                tile_wavelets = Some(main_wavelets.clone().ok_or_else(|| {
                    ImgQualityError::AnalysisError("JP2: SOT encountered before SIZ".to_string())
                })?);
            }
            _ => {}
        }
        pos = next;
    }

    Err(ImgQualityError::AnalysisError(
        "JP2: first tile header ended without SOD".to_string(),
    ))
}

/// Detect JXL (JPEG XL) lossless encoding — multi-dimension analysis.
fn detect_jxl_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    let internal = crate::image_formats::jxl::classify_compression(&data, path)?;
    if internal != CompressionType::Unknown || !JxlinfoBuilder::new().check_available() {
        return Ok(internal);
    }

    let output = run_jxlinfo_bounded(path, "JXL Modular compression classification")?;
    if !output.status.success() {
        tracing::debug!(
            target: "jxlinfo_probe",
            path = %path.display(),
            status = %output.status,
            "jxlinfo could not refine JXL compression; retaining Unknown"
        );
        return Ok(CompressionType::Unknown);
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(parse_jxlinfo_compression_hint(&combined).unwrap_or(CompressionType::Unknown))
}

#[cfg(test)]
mod tests {
    include!("../../tests/internal/image_detection.rs");
}
