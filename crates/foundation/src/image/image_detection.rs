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
//! - **AVIF**: Multi-dimension (av1C chroma 4:2:0/4:2:2→lossy; 4:4:4 + colr
//!   Identity MC u16[8..9]/pixi/high bit depth→lossless; 4:4:4 ambiguous→Err).
//!   Err when av1C missing or 4:4:4 without definitive indicators.
//! - **HEIC**: Multi-dimension (hvcC chromaFormatIdc 4:2:0/4:2:2→lossy;
//!   Main/Main10/MSP→lossy; RExt/SCC + 4:4:4→lossless; `RExt` without
//!   4:4:4→Err). Err when hvcC missing.
//! - **JXL**: Container jbrd box→lossless (naked codestream skips jbrd scan);
//!   codestream `xyb_encoded→lossy/modular`; Err only when no jbrd and header
//!   unparseable.
//! - **JPEG**: Always lossy; JXL transcoding does not require quality judgment.
//! - **EXR**: Parses compression attribute (NONE/RLE/ZIPS/ZIP/PIZ→lossless;
//!   PXR24/B44/B44A/DWAA/DWAB→lossy).
//! - **QOI, FLIF, PNM**: Treated as lossless. **JP2**: COD marker wavelet
//!   transform (9/7 irreversible→lossy, 5/3 reversible→lossless); fallback
//!   lossy if COD not found.
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
//! | AVIF   | High        | Multi (av1C)  | Err if no av1C or ambiguous 4:4:4. colr MC u16 fix. |
//! | HEIC   | High        | Multi (hvcC)  | chromaFormatIdc + profile. Err if no hvcC or `RExt` w/o 4:4:4. |
//! | JXL    | High        | Multi (jbrd/xyb)| Container-only jbrd. Err if unparseable. |
//! | GIF    | Assumed     | N/A           | Treated as lossless. |
//! | EXR    | High        | Yes (attr)    | Parses compression attr. No attr → lossless. |
//! | QOI/FLIF/PNM | Assumed | N/A        | Treated as lossless. |
//! | JP2    | High        | Yes (COD wavelet)| Fallback lossy if COD not found. |
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
use crate::unified_error::{ImgQualityError, Result};
use crate::{DjxlBuilder, FfmpegBuilder, FfprobeBuilder};
use image::{DynamicImage, GenericImageView, ImageReader, Rgba};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

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

    // PNG Heuristic Detection: Enable 4-layer analysis for PNG files.
    // Keep this out of the core decoder to avoid recursive analysis when PNG
    // heuristics need decoded pixels themselves.
    if format == Some(image::ImageFormat::Png) {
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
    let _file = File::open(path)?;
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

    let mut reader = ImageReader::open(path)?;
    let _ = reader.set_limits(limits);
    let (img, _metadata) = reader.decode().map_err(ImgQualityError::from)?;

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
        matches!(self, Self::HEIC | Self::HEIF | Self::AVIF | Self::JXL)
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

/// Detect if an image is animated (GIF, APNG, WebP, etc.).
///
/// # Errors
/// Returns an error if the file cannot be read or parsed.
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
            let frame_count = crate::image_formats::gif::count_frames_from_bytes(&data)?;
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
            let (is_animated, frame_count) = parse_apng_frames(&data);
            let fps = if is_animated {
                apng_timing_stats_from_bytes(&data)
                    .filter(|stats| stats.fps.is_finite() && stats.fps > 0.0_f64)
                    .map(|stats| crate::numeric_cast::f64_to_f32_lossy(stats.fps))
            } else {
                None
            };
            return Ok((is_animated, Some(frame_count), fps));
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

        // If metadata probe fails to find frame count (common for AVIF/JXL sequences),
        // we explicitly count the packets. This demuxes the file and is 100% accurate.
        if matches!(
            format,
            DetectedFormat::AVIF
                | DetectedFormat::JXL
                | DetectedFormat::HEIC
                | DetectedFormat::HEIF
        ) && let Some(explicit_count) = crate::ffprobe::get_frame_count(path)
        {
            if explicit_count > 1 {
                let final_count =
                    crate::numeric_cast::u64_to_u32_strict(explicit_count, "explicit_count");
                return Ok((true, final_count, fps));
            }
            if explicit_count == 1
                && matches!(format, DetectedFormat::AVIF | DetectedFormat::JXL)
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
            let (is_animated, count) = parse_apng_frames(&data);
            Ok(!is_animated && count <= 1)
        }
        DetectedFormat::AVIF | DetectedFormat::HEIC | DetectedFormat::HEIF => {
            isobmff_confirmed_static_only(path)
        }
        DetectedFormat::JXL => {
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
        _ => Ok(false),
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

    let mut file = File::open(path)?;

    let mut header = [0u8; 32];
    std::io::Read::read_exact(&mut file, &mut header)?;

    if header.get(4..8) != Some(b"ftyp") {
        return Ok(false);
    }

    let major_brand = &header[8..12];
    for seq_brand in ISOBMFF_ANIMATED_BRANDS {
        if major_brand == *seq_brand {
            return Ok(true);
        }
    }

    // Scan compatible_brands (each 4 bytes, starting at offset 16)
    let ftyp_box_size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let ftyp_box_size = crate::numeric_cast::u32_to_usize_strict(ftyp_box_size, "ftyp_box_size")
        .ok_or_else(|| ImgQualityError::AnalysisError("ftyp box size overflow".to_string()))?;

    if !(16..=4096).contains(&ftyp_box_size) {
        return Ok(false);
    }
    let compat_size = ftyp_box_size - 16;
    if compat_size == 0 {
        return Ok(false);
    }

    let mut compat_data = vec![0u8; compat_size];
    std::io::Read::read_exact(&mut file, &mut compat_data)?;

    for cb in compat_data.chunks_exact(4) {
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

    let output = JxlinfoBuilder::new()
        .input(path)
        .build()
        .output()
        .map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "JXL animation detection via jxlinfo failed for {}: {}",
                path.display(),
                e
            ))
        })?;

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

            let is_lossless = crate::image_formats::webp::is_lossless_from_bytes(&data);
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

        _ => Ok(CompressionType::Lossy),
    }
}

fn detect_png_compression(path: &Path) -> Result<CompressionType> {
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
            && self.png_info.width * self.png_info.height
                > crate::constants::PNG_EFFICIENCY_PIXEL_COUNT_THRESHOLD
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
            confidence: Some(
                crate::constants::IMAGE_DETECTION_CONFIDENCE_TRUECOLOR_QUANT
                    + tc_score_f * crate::constants::IMAGE_DETECTION_TRUECOLOR_CONF_SLOPE,
            ),
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
///
/// # Panics
/// Panics if the PNG decompression fails unexpectedly on a valid zTXt chunk.
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

/// # Errors
/// Returns an error if the file cannot be read or if the PNG structure is
/// corrupted. Specifically, `ImgQualityError::IoError` for file operations and
/// `ImgQualityError::AnalysisError` for parsing issues.
///
/// # Panics
/// Panics if the PNG structure is fundamentally corrupted beyond repair.
pub fn parse_png_structure<R: Read + Seek>(mut reader: R) -> Result<PngStructureInfo> {
    fn skip_bytes<R: Seek>(reader: &mut R, bytes: u64, context: &str) -> Result<()> {
        let offset = i64::try_from(bytes).map_err(|e| {
            ImgQualityError::AnalysisError(format!(
                "PNG chunk too large to seek while parsing {context}: {e}"
            ))
        })?;
        reader.seek(SeekFrom::Current(offset)).map_err(|e| {
            ImgQualityError::AnalysisError(format!("Failed to seek past {context}: {e}"))
        })?;
        Ok(())
    }

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
    let mut ihdr_data = [0u8; 13];
    reader
        .read_exact(&mut ihdr_data)
        .map_err(|e| ImgQualityError::AnalysisError(format!("IHDR data truncated: {e}")))?;

    let width = u32::from_be_bytes([ihdr_data[0], ihdr_data[1], ihdr_data[2], ihdr_data[3]]);
    let height = u32::from_be_bytes([ihdr_data[4], ihdr_data[5], ihdr_data[6], ihdr_data[7]]);
    let bit_depth = ihdr_data[8];
    let color_type = ihdr_data[9];
    skip_bytes(&mut reader, 4, "IHDR CRC")?;

    let mut palette_size: Option<usize> = None;
    let mut has_trns = false;
    let mut has_text_chunks = false;
    let mut detected_tool: Option<String> = None;

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
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(ImgQualityError::AnalysisError(format!(
                    "Failed to read PNG chunk header: {e}"
                )));
            }
        }
        let chunk_len = u64::from(u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]));
        let chunk_type = &buf[4..8];

        match chunk_type {
            b"PLTE" if color_type == 3 => {
                let plte_len =
                    crate::numeric_cast::u64_to_usize_strict(chunk_len, "PLTE chunk_len")
                        .ok_or_else(|| {
                            ImgQualityError::AnalysisError(format!(
                                "Invalid PLTE length: {chunk_len}"
                            ))
                        })?;
                palette_size = Some(plte_len / 3);
                skip_bytes(&mut reader, chunk_len + 4, "PLTE chunk")?;
            }
            b"tRNS" => {
                has_trns = true;
                skip_bytes(&mut reader, chunk_len + 4, "tRNS chunk")?;
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
                    return Err(ImgQualityError::AnalysisError(
                        "PNG text chunk exceeds 10MB safety limit".to_string(),
                    ));
                }
                let mut payload = vec![0u8; text_len];
                reader.read_exact(&mut payload).map_err(|e| {
                    ImgQualityError::AnalysisError(format!(
                        "Failed to read PNG text chunk payload: {e}"
                    ))
                })?;
                if let Some(null_pos) = payload.iter().position(|&b| b == 0) {
                    let keyword = String::from_utf8_lossy(&payload[..null_pos]);
                    for &(pattern, tool_name) in signatures {
                        if keyword.contains(pattern) {
                            detected_tool = Some(tool_name.to_string());
                            break;
                        }
                    }

                    if detected_tool.is_none() {
                        let mut text_payload = None;
                        let mut is_compressed = false;

                        match chunk_type {
                            b"zTXt" if null_pos + 2 < payload.len() => {
                                // zTXt: keyword\0 + method(1) + compressed_text
                                text_payload = Some(&payload[null_pos + 2..]);
                                is_compressed = true;
                            }
                            b"iTXt" if null_pos + 5 < payload.len() => {
                                // iTXt: keyword\0 + flag(1) + method(1) + lang\0 + trans\0 + text
                                let comp_flag = payload[null_pos + 1];
                                let mut pos = null_pos + 3;
                                if let Some(lang_null) = payload[pos..].iter().position(|&b| b == 0)
                                {
                                    pos += lang_null + 1;
                                    if let Some(trans_null) =
                                        payload[pos..].iter().position(|&b| b == 0)
                                    {
                                        pos += trans_null + 1;
                                        if pos < payload.len() {
                                            text_payload = Some(&payload[pos..]);
                                            is_compressed = comp_flag == 1;
                                        }
                                    }
                                }
                            }
                            b"tEXt" => {
                                text_payload = Some(&payload[null_pos + 1..]);
                                is_compressed = false;
                            }
                            _ => {}
                        }

                        if let Some(data) = text_payload {
                            if is_compressed {
                                let mut decompressed = Vec::new();
                                // Security: 50MB decompression limit to prevent Zip Bomb / OOM
                                if flate2::read::ZlibDecoder::new(data)
                                    .take(52_428_800)
                                    .read_to_end(&mut decompressed)
                                    .is_ok()
                                {
                                    let text = String::from_utf8_lossy(&decompressed);
                                    for &(pattern, tool_name) in signatures {
                                        if text.contains(pattern) {
                                            detected_tool = Some(tool_name.to_string());
                                            break;
                                        }
                                    }
                                }
                            } else {
                                let text = String::from_utf8_lossy(data);
                                for &(pattern, tool_name) in signatures {
                                    if text.contains(pattern) {
                                        detected_tool = Some(tool_name.to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                skip_bytes(&mut reader, 4, "text chunk CRC")?;
            }
            b"IEND" => break,
            _ => {
                skip_bytes(&mut reader, chunk_len + 4, "PNG chunk")?;
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
        total_score += ch_score * weight;
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
                    crate::image_formats::webp::is_lossless_from_bytes(&data);
                if !precision.is_lossless_deterministic {
                    precision.quality_estimate = match estimate_webp_quality(path) {
                        Ok(q) => Some(q),
                        Err(e) => {
                            crate::media_conversion_gate::probe_layer_audit(
                                "checkpoint_progress",
                                path,
                                format!(
                                    "WEBP QUALITY AUDIT: Failed to estimate quality for '{}' | \
                                     Forensic: Error '{}'; refusing to forge data; information \
                                     invalidated to prevent downstream precision loss",
                                    path.display(),
                                    e
                                ),
                            );
                            None
                        }
                    };
                }
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

    if estimated_quality.is_none() && compression == CompressionType::Lossy {
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
        estimated_quality = Some(estimate_lossy_quality_fallback(
            path,
            &format,
            width,
            height,
            file_size,
            quality_frame_count,
            entropy,
        )?);
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
    let q = crate::constants::PNG_QUALITY_EST_MIN + adjusted * range;
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
        crate::progress_mode::emit_stderr(&format!(
            "   \x1b[1;31m🚨 [CRITICAL FALLBACK]\x1b[0m \x1b[31mQuality detection failed and \
                 heuristic fallback is impossible.\x1b[0m\n\x1b[31m      File: \
                 {}\x1b[0m\n\x1b[31m      Refusing to invent a hardcoded quality value.\x1b[0m",
            path.display()
        ));
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
        crate::progress_mode::emit_stderr(&format!(
            "   \x1b[1;31m🚨 [CRITICAL FALLBACK]\x1b[0m \x1b[31mQuality detection failed; entropy \
             is unmeasurable so heuristic refuses to invent a value.\x1b[0m\n\x1b[31m      File: \
             {}\x1b[0m",
            path.display()
        ));
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

/// Estimate WebP VP8 quality by parsing the bitstream quantization index.
fn estimate_webp_quality(path: &Path) -> Result<u8> {
    crate::image_formats::webp::estimate_quality(path)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ApngTimingStats {
    pub frame_count: u32,
    pub duration_secs: f64,
    pub fps: f64,
}

fn apng_frame_delay_secs(delay_num: u16, delay_den: u16) -> f64 {
    let den = if delay_den == 0 { 100_u16 } else { delay_den };
    f64::from(delay_num) / f64::from(den)
}

/// Aggregate APNG timing from `fcTL` frame delays and `acTL` frame count.
#[must_use]
pub(crate) fn apng_timing_stats_from_bytes(data: &[u8]) -> Option<ApngTimingStats> {
    let (is_animated, frame_count) = parse_apng_frames(data);
    if !is_animated || frame_count <= 1 {
        return None;
    }

    let mut duration_secs = 0.0_f64;
    let mut pos = 8usize;
    while pos + 12 <= data.len() {
        let Some(length_bytes) = data.get(pos..pos + 4) else {
            break;
        };
        let length = u32::from_be_bytes([
            length_bytes[0],
            length_bytes[1],
            length_bytes[2],
            length_bytes[3],
        ]);
        pos += 4;

        let Some(chunk_type) = data.get(pos..pos + 4) else {
            break;
        };
        pos += 4;

        if chunk_type == b"fcTL" {
            let Some(chunk_data_size) =
                crate::numeric_cast::u32_to_usize_strict(length, "png_chunk_size")
            else {
                break;
            };
            if chunk_data_size >= 26 && pos + 24 <= data.len() {
                let delay_num = u16::from_be_bytes([data[pos + 20], data[pos + 21]]);
                let delay_den = u16::from_be_bytes([data[pos + 22], data[pos + 23]]);
                let delay = apng_frame_delay_secs(delay_num, delay_den);
                if delay.is_finite() && delay >= 0.0_f64 {
                    duration_secs += delay;
                }
            }
        }

        let Some(chunk_data_size) =
            crate::numeric_cast::u32_to_usize_strict(length, "png_chunk_size")
        else {
            break;
        };
        if pos > data.len().saturating_sub(chunk_data_size.saturating_add(4)) {
            break;
        }
        pos += chunk_data_size.saturating_add(4);
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
        chunk.extend_from_slice(&[0, 0, 0, 0]);
        chunk
    }

    fn fctl_chunk(delay_num: u16, delay_den: u16) -> Vec<u8> {
        let mut payload = vec![0u8; 26];
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
    data.extend(fctl_chunk(1, 100));
    data.extend(fctl_chunk(2, 100));
    data.extend(png_chunk(b"IEND", &[]));
    data
}

/// Parse APNG (Animated PNG) frame count from PNG data
/// Returns (`is_animated`, `frame_count`)
pub(crate) fn parse_apng_frames(data: &[u8]) -> (bool, u32) {
    // Look for acTL (Animation Control) chunk
    let mut pos = 8; // Skip PNG signature
    while pos + 12 <= data.len() {
        // Read chunk length (big-endian)
        let Some(length_bytes) = data.get(pos..pos + 4) else {
            break;
        };
        let length = u32::from_be_bytes([
            length_bytes[0],
            length_bytes[1],
            length_bytes[2],
            length_bytes[3],
        ]);
        pos += 4;

        // Read chunk type
        let Some(chunk_type) = data.get(pos..pos + 4) else {
            break;
        };
        pos += 4;

        // Check if this is acTL chunk
        if chunk_type == b"acTL" {
            if pos + 4 <= data.len() {
                // Read num_frames (first 4 bytes of acTL data)
                let Some(num_frames_bytes) = data.get(pos..pos + 4) else {
                    break;
                };
                let num_frames = u32::from_be_bytes([
                    num_frames_bytes[0],
                    num_frames_bytes[1],
                    num_frames_bytes[2],
                    num_frames_bytes[3],
                ]);
                return (num_frames > 1, num_frames.max(1));
            }
            crate::media_conversion_gate::probe_layer_batch_audit(
                "delivery_db_numeric",
                "PNG DECODE AUDIT: acTL chunk found but num_frames data is missing/truncated! | \
                 Forensic: Malformed APNG bitstream; refusing to forge frame count to prevent \
                 downstream numeric corruption",
            );
            return (true, 0); // Honest report: it's animated, but count is unknown
        }

        // Skip chunk data and CRC
        let Some(chunk_data_size) =
            crate::numeric_cast::u32_to_usize_strict(length, "png_chunk_size")
        else {
            break;
        };
        pos += chunk_data_size + 4;
    }

    (false, 1)
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
    if crate::image_formats::tiff::is_lossless(path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect AVIF lossless encoding — multi-dimension analysis.
fn detect_avif_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if crate::image_formats::avif::is_lossless_from_bytes(&data, path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect HEIC/HEIF lossless encoding — multi-dimension analysis.
fn detect_heic_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if crate::image_heic_analysis::detect_heic_is_lossless(&data, path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

/// Detect ICO compression by inspecting embedded image entries.
///
/// ICO directory: header\[6\] + entries[16 each]. Each entry has an offset to
/// image data. If image data starts with PNG magic → recursively check PNG
/// quantization. Any quantized PNG entry → Lossy. Otherwise → Lossless.
fn detect_ico_compression(path: &Path) -> Result<CompressionType> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(ImgQualityError::IoError)?;

    // ICO header: reserved(2) + type(2) + count(2) = 6 bytes
    let mut header = [0u8; 6];
    if file.read(&mut header).is_err() {
        return Ok(CompressionType::Lossless);
    }

    let image_count = usize::from(u16::from_le_bytes([header[4], header[5]]));
    let png_magic: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    // Each directory entry is 16 bytes, starting at offset 6
    for i in 0..image_count {
        let entry_offset = 6 + crate::numeric_cast::usize_to_u64_strict(i, "ICO entry index")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!("ICO index conversion failed for entry {i}"))
            })?
            * 16;
        if file.seek(SeekFrom::Start(entry_offset)).is_err() {
            crate::media_conversion_gate::probe_layer_audit(
                "delivery_db_metadata",
                path,
                format!(
                    "ICO DECODE AUDIT: Failed to seek to entry {} at offset {} for '{}' | \
                     Forensic: IO failure during directory traversal; breaking loop to prevent \
                     corrupt metadata emission",
                    i,
                    entry_offset,
                    path.display()
                ),
            );
            break;
        }

        let mut entry = [0u8; 16];
        if file.read(&mut entry).is_err() {
            crate::media_conversion_gate::probe_layer_audit(
                "probe_image_detection",
                path,
                format!(
                    "ICO DECODE AUDIT: Truncated entry {} at offset {} for '{}' | Forensic: \
                     Unexpected EOF during directory parse; breaking loop to prevent \
                     out-of-bounds access",
                    i,
                    entry_offset,
                    path.display()
                ),
            );
            break;
        }

        // Bytes 8-11: size of image data, bytes 12-15: offset of image data
        let img_size = u64::from(u32::from_le_bytes([
            entry[8], entry[9], entry[10], entry[11],
        ]));
        let img_offset = u64::from(u32::from_le_bytes([
            entry[12], entry[13], entry[14], entry[15],
        ]));

        // Peak into image data for PNG magic
        match file.seek(SeekFrom::Start(img_offset)) {
            Ok(_) => {
                let mut magic_peek = [0u8; 8];
                match file.read_exact(&mut magic_peek) {
                    Ok(()) if magic_peek == png_magic => {
                        // Seek back to start of image data for full analysis
                        file.seek(SeekFrom::Start(img_offset))?;
                        let mut img_reader = (&file).take(img_size);
                        // Since analyze_png_quantization_from_reader needs Seek, and take() doesn't
                        // provide it easily, we read the PNG part into
                        // memory. BUT: PNGs inside ICO are usually small (max 512KB for 256x256).
                        // This is infinitely safer than loading the whole 64MB ICO.
                        let Some(png_capacity) =
                            crate::numeric_cast::u64_to_usize_strict(img_size, "ico_img_size")
                        else {
                            crate::media_conversion_gate::probe_layer_audit(
                                "delivery_db_numeric",
                                path,
                                format!(
                                    "ICO DECODE AUDIT: Image size {} in '{}' overflows usize | \
                                     Forensic: Magnitude exceeds platform pointer width; skipping \
                                     entry to prevent OOM panic",
                                    img_size,
                                    path.display()
                                ),
                            );
                            continue;
                        };
                        let mut png_data = Vec::with_capacity(png_capacity);
                        img_reader.read_to_end(&mut png_data)?;
                        let analysis = analyze_png_quantization_from_bytes(&png_data)?;
                        if analysis.is_quantized {
                            return Ok(CompressionType::Lossy);
                        }
                    }
                    Ok(()) => {}
                    Err(err) => {
                        crate::media_conversion_gate::probe_layer_audit(
                            "ico_embedded_png_magic_read_failed",
                            path,
                            format!("failed to read embedded ICO image magic: {err}"),
                        );
                    }
                }
            }
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "ico_embedded_png_seek_failed",
                    path,
                    format!("failed to seek to embedded ICO image offset {img_offset}: {err}"),
                );
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
        // Fallback to lossless for corrupted/invalid EXR files (safe default)
        return Ok(CompressionType::Lossless);
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

                // Any lossy part → entire file is lossy
                if compression >= 5 {
                    return Ok(CompressionType::Lossy);
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

    let _ = found_any_compression; // silence found_any_compression if unused
    Ok(CompressionType::Lossless)
}

/// Detect JPEG 2000 lossless vs lossy by parsing COD and COC markers.
///
/// COD (Coding style Default, FF 52) contains default `SPcod` parameters; the
/// last byte is the wavelet transform type:
///   - 0 = 9/7 irreversible (lossy)
///   - 1 = 5/3 reversible (lossless)
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
        return Ok(CompressionType::Lossy);
    }

    // Determine where the codestream starts
    let cs_start = if data.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        // Raw codestream
        0
    } else {
        // JP2 container — find jp2c box
        find_jp2c_offset(&data).ok_or_else(|| {
            ImgQualityError::AnalysisError(
                "Could not find JPEG 2000 codestream (jp2c box)".to_string(),
            )
        })?
    };

    // Scan for COD and COC markers in the codestream header area
    // COD/COC must appear before the first tile-part, so limit scan to first 4KB of
    // codestream
    let scan_end = (cs_start + 4096).min(data.len());
    let cs = data.get(cs_start..scan_end).ok_or_else(|| {
        ImgQualityError::AnalysisError("Required byte slice missing (out of bounds)".to_string())
    })?;

    let (cod_wavelet, coc_wavelets) = find_jp2_wavelets(cs);

    // Check COD default wavelet
    if let Some(wavelet) = cod_wavelet {
        if std::env::var(crate::constants::ENV_VERBOSE).is_ok() {
            crate::log_detail!(&format!(
                "   📊 JP2 COD wavelet: {} ({})",
                wavelet,
                if wavelet == 1 {
                    "5/3 reversible — lossless"
                } else {
                    "9/7 irreversible — lossy"
                }
            ));
        }
        // If COD is lossy and no COC overrides, it's lossy
        if wavelet == 0 && coc_wavelets.is_empty() {
            return Ok(CompressionType::Lossy);
        }
    }

    // Check COC component-specific wavelets
    for (component, wavelet) in &coc_wavelets {
        if std::env::var(crate::constants::ENV_VERBOSE).is_ok() {
            crate::log_detail!(&format!(
                "   📊 JP2 COC component {} wavelet: {} ({})",
                component,
                wavelet,
                if *wavelet == 1 {
                    "5/3 reversible — lossless"
                } else {
                    "9/7 irreversible — lossy"
                }
            ));
        }
        // Any lossy component → entire file is lossy
        if *wavelet == 0 {
            return Ok(CompressionType::Lossy);
        }
    }

    // All components are lossless (or only COD found and it's lossless)
    if cod_wavelet == Some(1) || !coc_wavelets.is_empty() {
        return Ok(CompressionType::Lossless);
    }

    // Couldn't find COD — default to lossy (safer assumption for JP2)
    Ok(CompressionType::Lossy)
}

/// Find the offset of the jp2c (contiguous codestream) box payload in a JP2
/// container.
fn find_jp2c_offset(data: &[u8]) -> Option<usize> {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = crate::numeric_cast::u32_to_usize_strict(
            data.get_u32_be_strict(pos, "JP2 box size")?,
            "jp2_box_size",
        )?;
        let box_type = crate::media_conversion_gate::probe_jpeg_buffer_slice(
            data,
            (pos + 4)..(pos + 8),
            "jp2 box type",
        );

        if box_type == b"jp2c" {
            return Some(pos + 8);
        }

        if size == 0 {
            break;
        } else if size == 1 {
            if pos + 16 > data.len() {
                break;
            }
            let ext = crate::numeric_cast::u64_to_usize_strict(
                data.get_u64_be_strict(pos + 8, "JP2 extended box size")?,
                "jp2_ext_box_size",
            )?;
            pos += ext;
        } else if size < 8 {
            break;
        } else {
            pos += size;
        }
    }
    None
}

/// Scan JPEG 2000 codestream for COD and COC markers, extract wavelet transform
/// types. Returns (COD wavelet, Vec<(`component_index`, COC wavelet)>).
/// COD: Some(0) for 9/7 irreversible (lossy), Some(1) for 5/3 reversible
/// (lossless). COC: component-specific overrides.
fn find_jp2_wavelets(cs: &[u8]) -> (Option<u8>, Vec<(u16, u8)>) {
    let mut cod_wavelet: Option<u8> = None;
    let mut coc_wavelets: Vec<(u16, u8)> = Vec::new();

    // Walk markers: each marker is FF xx, followed by 2-byte length (except
    // SOC=FF4F, SOD=FF93)
    let mut pos = 0;
    while pos + 2 <= cs.len() {
        if cs.get(pos) != Some(&0xFF) {
            pos += 1;
            continue;
        }
        let Some(marker) = cs.get_byte_strict(pos + 1, "JP2 marker") else {
            break;
        };

        // SOC (FF 4F) — no length field
        if marker == 0x4F {
            pos += 2;
            continue;
        }
        // SOD (FF 93) — start of data, stop scanning
        if marker == 0x93 {
            break;
        }

        // COD marker (FF 52)
        if marker == 0x52 && pos + 4 <= cs.len() {
            let Some(seg_len_u16) = cs.get_u16_be_strict(pos + 2, "JP2 COD segment length") else {
                break;
            };
            let Some(seg_len) =
                crate::numeric_cast::u16_to_usize_strict(seg_len_u16, "jp2_seg_len")
            else {
                break;
            };
            // COD segment: Scod(1) + SGcod(4) + SPcod(variable)
            // SPcod starts at offset 5 within segment data
            // SPcod layout: NL(1) + cb_width(1) + cb_height(1) + cb_style(1) + transform(1)
            // So transform byte is at segment_data[5 + 4] = segment_data[9]
            // segment_data starts at pos+4, so transform is at pos+4+9 = pos+13
            let transform_offset = pos + 4 + 9;
            if transform_offset < cs.len() && seg_len >= 10 {
                let Some(wavelet) = cs.get_byte_strict(transform_offset, "JP2 COD wavelet") else {
                    break;
                };
                if wavelet <= 1 {
                    cod_wavelet = Some(wavelet);
                }
            }
        }

        // COC marker (FF 53) — component-specific coding style
        if marker == 0x53 && pos + 4 <= cs.len() {
            let Some(seg_len_u16) = cs.get_u16_be_strict(pos + 2, "JP2 COC segment length") else {
                break;
            };
            let Some(seg_len) =
                crate::numeric_cast::u16_to_usize_strict(seg_len_u16, "jp2_coc_seg_len")
            else {
                break;
            };
            // COC segment: Ccoc(1 or 2 bytes) + Scoc(1) + SPcoc(variable)
            // For images with < 257 components, Ccoc is 1 byte; otherwise 2 bytes
            // We'll assume 1 byte for simplicity (most common case)
            // SPcoc layout is same as SPcod: NL(1) + cb_width(1) + cb_height(1) +
            // cb_style(1) + transform(1)
            let component_offset = pos + 4;
            let spcoc_offset = component_offset + 1; // Ccoc (1 byte) + Scoc (1 byte) = 2 bytes before SPcoc
            let transform_offset = spcoc_offset + 1 + 4; // SPcoc[4] = transform

            if component_offset < cs.len() && transform_offset < cs.len() && seg_len >= 7 {
                let Some(comp_idx) =
                    cs.get_byte_strict(component_offset, "JP2 COC component index")
                else {
                    break;
                };
                let component = u16::from(comp_idx);
                let Some(wavelet) = cs.get_byte_strict(transform_offset, "JP2 COC wavelet") else {
                    break;
                };
                if wavelet <= 1 {
                    coc_wavelets.push((component, wavelet));
                }
            }
        }

        // Skip marker segment
        if pos + 4 > cs.len() {
            break;
        }
        let Some(seg_len_u16) = cs.get_u16_be_strict(pos + 2, "JP2 segment length") else {
            break;
        };
        let Some(seg_len) = crate::numeric_cast::u16_to_usize_strict(seg_len_u16, "jp2_seg_len")
        else {
            break;
        };
        pos += 2 + seg_len;
    }

    (cod_wavelet, coc_wavelets)
}

/// Detect JXL (JPEG XL) lossless encoding — multi-dimension analysis.
fn detect_jxl_compression(path: &Path) -> Result<CompressionType> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::IMAGE_ANALYSIS_FILE_SIZE_LIMIT,
    )
    .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

    let data = std::fs::read(path)?;
    if crate::image_formats::jxl::is_lossless_from_bytes(&data, path)? {
        Ok(CompressionType::Lossless)
    } else {
        Ok(CompressionType::Lossy)
    }
}

#[cfg(test)]
mod tests {
    include!("../tests/image_detection.rs");
}
