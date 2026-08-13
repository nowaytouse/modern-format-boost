// Image Analysis Module
use crate::builder_base::ToolBuilder;
use crate::constants;
use crate::ffprobe_json::ColorInfo;
use crate::image_detection::{
    CompressionType, DetectedFormat, DetectionResult, PrecisionMetadata, detect_image,
};
use crate::image_heic_analysis::{HeicAnalysis, analyze_heic_file_v4, is_heic_file};
use crate::image_jpeg_analysis::{JpegQualityAnalysis, analyze_jpeg_file};
use crate::infra::static_logs::messages;
use crate::infra::static_logs::messages::LABEL_IMAGE;
use crate::media_index_types::MediaIndexRow;
use crate::media_precision::{ImagePrecisionProfile, MediaPrecision};
use crate::probe_video;
use crate::types::{ProcessHistory, Visual};
use crate::unified_error::{ImgQualityError, Result};
use image::{DynamicImage, GenericImageView, ImageFormat};
#[cfg(feature = "high-precision")]
use rug::Rational;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

/// Probe-layer audit (routes to `media_conversion_gate::probe_layer_audit`).
macro_rules! probe_audit {
    ($branch:expr, $path:expr, $($arg:tt)*) => {
        $crate::media_conversion_gate::probe_layer_audit(
            $branch,
            $path,
            format!($($arg)*),
        )
    };
}

/// Analyzer warning routed through `log_detail` with plain-aware prefix (U7).
macro_rules! warn_detail {
    ($($arg:tt)*) => {
        $crate::log_detail!(&format!(
            "{} {}",
            $crate::media_conversion_gate::ui_icon_pick(
                $crate::modern_ui::symbols::WARNING,
                $crate::modern_ui::symbols::plain::WARNING,
            ),
            format!($($arg)*)
        ))
    };
}

/// Analyzer info note routed through `log_detail` with plain-aware prefix (U7).
macro_rules! info_detail {
    ($($arg:tt)*) => {
        $crate::log_detail!(&format!(
            "{} {}",
            $crate::media_conversion_gate::ui_icon_pick(
                $crate::modern_ui::symbols::INFO,
                $crate::modern_ui::symbols::plain::INFO,
            ),
            format!($($arg)*)
        ))
    };
}

/// Minimum duration (seconds) for converting animated images to HEVC video.
/// Shorter animations are skipped (no conversion to video).
pub const ANIMATED_MIN_DURATION_FOR_VIDEO_SECS: f32 =
    crate::constants::ANIMATED_MIN_DURATION_FOR_VIDEO_SECS;

/// Opens an image reader with magic-byte detection to handle non-standard
/// extensions.
///
/// # Errors
/// Returns `ImageError` if the file cannot be opened or format cannot be
/// determined from the file contents.
fn open_image_reader_with_magic_bytes(
    path: &Path,
) -> std::io::Result<image::ImageReaderOptions<std::io::BufReader<std::fs::File>>> {
    image::ImageReaderOptions::open(path)?.with_guessed_format()
}

fn image_dimensions_with_magic_bytes(path: &Path) -> std::result::Result<(u32, u32), String> {
    let reader = image::ImageReaderOptions::open(path).map_err(|err| err.to_string())?;
    let reader = reader
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    reader.into_dimensions().map_err(|err| err.to_string())
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeRecommendation {
    pub current_format: String,
    pub recommended_format: String,
    pub reason: String,
    pub expected_size_reduction: f64,
    pub quality_preservation: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageFeatures {
    pub entropy: Option<f64>,
    pub compression_ratio: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionColorRole {
    TrueHdrMetadata,
    PrecisionOrWideGamutHint,
}

#[derive(Debug, Clone, Default)]
pub struct ConversionColorContext {
    role: Option<ConversionColorRole>,
    pub color_info: Option<ColorInfo>,
}

impl ConversionColorContext {
    #[must_use]
    pub fn classify(color_info: ColorInfo) -> Self {
        let assessment = color_info.assessment();
        if !assessment.should_carry_conversion_metadata() {
            return Self::default();
        }

        let role = if assessment.has_hdr_signaling() {
            ConversionColorRole::TrueHdrMetadata
        } else {
            ConversionColorRole::PrecisionOrWideGamutHint
        };

        Self {
            role: Some(role),
            color_info: Some(color_info),
        }
    }

    #[must_use]
    pub fn true_hdr(color_info: ColorInfo) -> Self {
        debug_assert!(color_info.assessment().has_hdr_signaling());
        Self {
            role: Some(ConversionColorRole::TrueHdrMetadata),
            color_info: Some(color_info),
        }
    }

    #[must_use]
    pub fn precision_or_wide_gamut_hint(color_info: ColorInfo) -> Self {
        debug_assert!(!color_info.assessment().has_hdr_signaling());
        Self {
            role: Some(ConversionColorRole::PrecisionOrWideGamutHint),
            color_info: Some(color_info),
        }
    }

    #[must_use]
    pub const fn role(&self) -> Option<ConversionColorRole> {
        self.role
    }

    #[must_use]
    pub const fn conversion_color_info(&self) -> Option<&ColorInfo> {
        self.color_info.as_ref()
    }

    #[must_use]
    pub const fn has_true_hdr_metadata(&self) -> bool {
        matches!(self.role, Some(ConversionColorRole::TrueHdrMetadata))
    }

    #[must_use]
    pub const fn has_precision_or_hdr_hints(&self) -> bool {
        self.role.is_some()
    }

    #[must_use]
    pub const fn is_precision_or_wide_gamut_hint(&self) -> bool {
        matches!(
            self.role,
            Some(ConversionColorRole::PrecisionOrWideGamutHint)
        )
    }
}

impl From<Option<ColorInfo>> for ConversionColorContext {
    fn from(color_info: Option<ColorInfo>) -> Self {
        match color_info {
            Some(info) => Self::classify(info),
            None => Self::default(),
        }
    }
}

impl From<ColorInfo> for ConversionColorContext {
    fn from(color_info: ColorInfo) -> Self {
        Self::classify(color_info)
    }
}

impl Serialize for ConversionColorContext {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.color_info.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ConversionColorContext {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let color_info = Option::<ColorInfo>::deserialize(deserializer)?;
        Ok(Self::from(color_info))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAnalysis {
    pub cache_version: u16,
    pub file_path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,

    pub color_depth: Option<u8>,
    pub color_space: Option<String>,
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

    /// Color metadata carried into downstream conversion paths. This may
    /// represent true HDR signaling, wide-gamut metadata, or conservative
    /// precision-preservation hints.
    #[serde(default, alias = "hdr_info")]
    pub color_context: ConversionColorContext,

    pub precision: PrecisionMetadata,

    /// 🛠️ New Dimension: Processing history for cache invalidation logic
    pub history: ProcessHistory,

    /// 🔬 New Dimension: Visual perception data (Auxiliary analysis)
    pub perception: Visual,

    /// Real physical features (225-dimensional 15x15 luminance sampling)
    pub physics_225: Option<Vec<f32>>,

    /// ⚠️ Optional: Store error message if deep analysis failed but we fell
    /// back to basic info
    pub analysis_error: Option<String>,
}

const IMAGE_ANALYSIS_CACHE_VERSION: u16 = 3;

impl Default for ImageAnalysis {
    fn default() -> Self {
        Self {
            cache_version: IMAGE_ANALYSIS_CACHE_VERSION,
            file_path: String::new(),
            format: "unknown".into(),
            width: 0,
            height: 0,
            file_size: 0,
            color_depth: None,
            color_space: None,
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
            color_context: ConversionColorContext::default(),
            precision: PrecisionMetadata::default(),
            history: ProcessHistory::default(),
            perception: Visual::default(),
            physics_225: None,
            analysis_error: None,
        }
    }
}

/// Analyzes an image file. Format detection order (by path/content): HEIC → JXL
/// → AVIF → image crate (PNG/JPEG/WebP/GIF/TIFF).
///
/// Quality is then derived via `detect_lossless` / `detect_compression` per
/// format; no conversion is done here. Comprehensive image analysis: format,
/// dimensions, quality, and compression.
///
/// # Errors
/// Returns an error if the file cannot be read, format is unsupported, or
/// analysis fails.
pub fn analyze_image(path: &Path) -> Result<ImageAnalysis> {
    analyze_image_with_cache(path, None)
}

/// Analyzes an image file with optional `SQLite` caching.
/// Image analysis with optional cache lookup.
///
/// # Errors
/// Returns an error if analysis fails and no cached result is available.
pub fn analyze_image_with_cache(
    path: &Path,
    cache: Option<&crate::analysis_cache::AnalysisCache>,
) -> Result<ImageAnalysis> {
    // Fast-path for JPEGs: Bypass the SQLite cache entirely because:
    // 1. JPEG analysis (DQT markers only) is faster than SQLite/Hashing overhead.
    // 2. We don't need pixel-level features for JPEG->JXL lossless transcoding.
    let is_jpeg = cache.is_some()
        && crate::image::format_detect::detect_true_format(path)?
            == crate::image::format_detect::FormatKind::Jpeg;

    if is_jpeg {
        match analyze_image_internal(path) {
            Ok(mut analysis) => {
                crate::media_conversion_gate::reconcile_analysis_animation_flag(
                    path,
                    &mut analysis,
                );
                return Ok(analysis);
            }
            Err(err) => {
                probe_audit!(
                    "jpeg_cache_bypass_analysis_failed",
                    path,
                    "JPEG cache-bypass analysis failed: {err}",
                    err = err,
                );
                return Err(err);
            }
        }
    }

    if let Some(cache) = cache {
        match cache.get_analysis(path) {
            Ok(Some(cached)) => {
                debug!(
                    "CACHE HIT: {} - is_lossless={}",
                    path.display(),
                    cached.is_lossless
                );
                if std::env::var(crate::constants::ENV_DEBUG).is_ok() {
                    crate::log_detail!(
                        &crate::infra::static_logs::messages::MSG_ANALYZER_CACHE_HIT
                            .replace("{}", &path.display().to_string())
                    );
                }
                let mut cached = cached;
                crate::media_conversion_gate::reconcile_analysis_animation_flag(path, &mut cached);
                return Ok(cached);
            }
            Ok(None) => {
                debug!("CACHE MISS: {} - not in cache", path.display());
            }
            Err(e) => {
                debug!("CACHE ERROR: {} - {}", path.display(), e);
                crate::media_conversion_gate::analyzer_cache_load_failed_audit(path, e);
            }
        }
    }

    let mut analysis = analyze_image_internal(path)?;
    crate::media_conversion_gate::reconcile_analysis_animation_flag(path, &mut analysis);

    if let Some(cache) = cache
        && let Err(e) = cache.store_analysis(path, &analysis)
    {
        crate::media_conversion_gate::analyzer_cache_store_failed_audit(path, "analyze-store", e);
    }

    debug!(
        file = %path.display(),
        width = analysis.width,
        height = analysis.height,
        file_size = analysis.file_size,
        format = %analysis.format,
        color_depth = analysis.color_depth,
        is_lossless = analysis.is_lossless,
        has_alpha = analysis.has_alpha,
        conversion_color_role = ?analysis.conversion_color_role(),
        has_hdr_metadata = analysis.has_true_hdr_metadata(),
        has_precision_or_hdr_hints = analysis.has_precision_or_hdr_hints(),
        "Analysis complete"
    );

    Ok(analysis)
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn analyze_image_internal(path: &Path) -> Result<ImageAnalysis> {
    let span = tracing::info_span!("image_analysis", file = %path.display());
    let _enter = span.enter();
    if !path.exists() {
        return Err(ImgQualityError::ImageReadError(format!(
            "File not found: {}",
            path.display()
        )));
    }

    let file_size = std::fs::metadata(path)?.len();

    let is_heic = is_heic_file(path)?;
    debug!(is_heic, "Starting image analysis");

    if is_heic {
        debug!("HEIC format detected for {}", path.display());
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if ![
                crate::constants::EXT_HEIC,
                crate::constants::EXT_HEIF,
                "hif",
            ]
            .contains(&ext_str.as_str())
            {
                probe_audit!(
                    "extension_mismatch_heic",
                    path,
                    "file content is HEIC but extension is .{ext_str}",
                    ext_str = ext_str,
                );
            }
        }
        return analyze_heic_image(path, file_size);
    }

    if is_jxl_file(path)? {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if ext_str != crate::constants::EXT_JXL {
                probe_audit!(
                    "extension_mismatch_jxl",
                    path,
                    "file content is JXL but extension is .{ext_str}",
                    ext_str = ext_str,
                );
            }
        }
        return analyze_jxl_image(path, file_size);
    }

    // AVIF: image crate fails on some variants (e.g. tachimanga output); fall back
    // to ffprobe
    let detected_format = crate::image_detection::detect_format_from_bytes(path)?;

    if detected_format == DetectedFormat::JPEG {
        return Ok(analyze_jpeg_fast_path(path, file_size));
    }

    let is_avif = detected_format == crate::image_detection::DetectedFormat::AVIF;

    if is_avif {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if ext_str != crate::constants::EXT_AVIF {
                probe_audit!(
                    "extension_mismatch_avif",
                    path,
                    "file content is AVIF but extension is .{ext_str}",
                    ext_str = ext_str,
                );
            }
        }
        return Ok(analyze_avif_image(path, file_size));
    }

    let mut reader = open_image_reader_with_magic_bytes(path)
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to open file: {e}")))?;
    {
        use image::Limits;
        let mut limits = Limits::default();
        limits.max_alloc = Some(crate::constants::IMAGE_DECODE_MAX_ALLOC_BYTES);
        reader.limits(limits);
    }

    let format = reader.format().ok_or_else(|| {
        ImgQualityError::image_not_supported(format!(
            "Could not detect content format for {}",
            path.display()
        ))
    })?;
    let img = reader
        .decode()
        .map_err(|e| ImgQualityError::ImageReadError(format!("Failed to decode image: {e}")))?;
    let format_str = format_to_string(format);

    let mut extension_mismatch = false;
    let mut real_extension_suggestion = String::new();
    let mut apple_warning = String::new();

    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();

        let (is_valid, suggested) = match format {
            ImageFormat::Jpeg => (
                [crate::constants::EXT_JPG, crate::constants::EXT_JPEG, "jpe"]
                    .contains(&ext_str.as_str()),
                crate::constants::EXT_JPG,
            ),
            ImageFormat::Png => (
                ext_str == crate::constants::EXT_PNG,
                crate::constants::EXT_PNG,
            ),
            ImageFormat::WebP => (
                ext_str == crate::constants::EXT_WEBP,
                crate::constants::EXT_WEBP,
            ),
            ImageFormat::Gif => (
                ext_str == crate::constants::EXT_GIF,
                crate::constants::EXT_GIF,
            ),
            ImageFormat::Tiff => (
                [crate::constants::EXT_TIFF, crate::constants::EXT_TIF].contains(&ext_str.as_str()),
                crate::constants::EXT_TIFF,
            ),
            ImageFormat::Avif => (
                ext_str == crate::constants::EXT_AVIF,
                crate::constants::EXT_AVIF,
            ),
            _ => (true, ""),
        };

        if !is_valid && !suggested.is_empty() {
            extension_mismatch = true;
            real_extension_suggestion = suggested.to_string();

            probe_audit!(
                "extension_mismatch_generic",
                path,
                "file content is {format_str} but extension is .{ext_str}",
                format_str = format_str,
                ext_str = ext_str,
            );

            apple_warning = format!(
                "Apple Compatibility: Extension mismatch (.{ext_str} vs {format_str}) may cause \
                 playback issues",
            );
        }
    }

    let (width, height) = img.dimensions();
    let has_alpha = has_alpha_channel(&img);
    let PreciseColorMetadata {
        color_space,
        color_context,
        precise_bit_depth,
    } = extract_precise_color_metadata(path);

    let is_animated = is_animated_format(path, format).map_err(|e| {
        ImgQualityError::AnalysisError(format!(
            "Analyzer Audit: Failed to determine animation status for {path_display}: {e}",
            path_display = path.display(),
        ))
    })?;

    let is_lossless = match detect_lossless(format, path) {
        Ok(l) => l,
        Err(e) => {
            probe_audit!(
                "detect_lossless_failed",
                path,
                "Format-level lossless detection failed, using pixel fallback: {e}",
                e = e,
            );
            pixel_fallback_lossless(path)?
        }
    };

    let jpeg_analysis = if format == ImageFormat::Jpeg {
        match analyze_jpeg_file(path) {
            Ok(analysis) => Some(analysis),
            Err(e) => {
                probe_audit!(
                    "jpeg_quantization_analysis_failed",
                    path,
                    "JPEG quantization analysis failed: {e}",
                    e = e,
                );
                None
            }
        }
    } else {
        None
    };

    let features = calculate_image_features(&img, file_size)?;

    let jxl_indicator = generate_jxl_indicator(format, is_lossless, jpeg_analysis.as_ref(), path);

    // PSNR/SSIM only from reference-encode measurement (explore pipeline). Never
    // map JPEG Q→PSNR here.
    let (psnr, ssim) = (None, None);

    let mut metadata = extract_metadata(path);

    if extension_mismatch {
        metadata.insert("extension_mismatch".to_string(), "true".to_string());
        metadata.insert("real_extension".to_string(), real_extension_suggestion);
        metadata.insert("apple_compatibility_warning".to_string(), apple_warning);
        metadata.insert(
            "format_warning".to_string(),
            format!("Content is actually {format_str}"),
        );
    }

    let duration_secs = if is_animated {
        get_animation_duration(path)
    } else {
        None
    };

    let (precision, detected_bit_depth) = match detect_image(path) {
        Ok(d) => (d.precision, d.bit_depth),
        Err(e) => {
            probe_audit!(
                "precision_detection_failed",
                path,
                "precision detection failed: {e}; emitting empty metadata",
                e = e,
            );
            (PrecisionMetadata::default(), None)
        }
    };
    let color_depth = detected_bit_depth.or(precise_bit_depth);

    // Extract real physics features (15x15 luminance downsample)
    let physics_225 = Some(crate::real_physics::extract_image_physics_225(&img));
    let perception = extract_visual_perception(&img);

    Ok(ImageAnalysis {
        cache_version: IMAGE_ANALYSIS_CACHE_VERSION,
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
        color_context,
        precision,
        history: crate::common_utils::get_current_history(),
        perception,
        physics_225,
        analysis_error: None,
    })
}

impl ImageAnalysis {
    #[must_use]
    pub const fn conversion_color_role(&self) -> Option<ConversionColorRole> {
        self.color_context.role()
    }

    #[must_use]
    pub fn conversion_color_context(&self) -> Option<&ConversionColorContext> {
        self.color_context
            .has_precision_or_hdr_hints()
            .then_some(&self.color_context)
    }

    #[must_use]
    pub const fn conversion_color_info(&self) -> Option<&ColorInfo> {
        self.color_context.conversion_color_info()
    }

    #[must_use]
    pub const fn has_true_hdr_metadata(&self) -> bool {
        self.color_context.has_true_hdr_metadata()
    }

    #[must_use]
    pub const fn has_precision_or_hdr_hints(&self) -> bool {
        self.color_context.has_precision_or_hdr_hints()
    }

    /// Returns a human-readable quality summary label (e.g. "Q=95 Excellence",
    /// "Lossless").
    #[must_use]
    pub fn quality_summary(&self) -> String {
        if let Some(jpeg) = self.jpeg_analysis.as_ref() {
            return format!("Q={} {}", jpeg.estimated_quality, jpeg.quality_description);
        }
        if let Some(heic) = self.heic_analysis.as_ref() {
            if heic.is_lossless {
                return constants::VAL_LOSSLESS.to_string();
            }
            let profile_label = if heic.hdr.is_hdr {
                constants::VAL_HDR.to_string()
            } else if let Some(bit_depth) = heic.bit_depth.filter(|&d| d > 8) {
                format!("{bit_depth}-bit")
            } else {
                constants::VAL_SD.to_string()
            };
            return format!("{} {}", heic.codec, profile_label);
        }
        if self.is_lossless {
            constants::VAL_LOSSLESS.to_string()
        } else {
            constants::VAL_LOSSY.to_string()
        }
    }
}

/// # Errors
/// Returns an error if HEIC analysis fails or file cannot be read.
// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn analyze_heic_image(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    debug!("analyze_heic_image called for {}", path.display());

    // Phase 1: Deep Analysis Attempt
    let deep_result = analyze_heic_file_v4(path);

    let (
        width,
        height,
        has_alpha,
        color_depth,
        is_lossless,
        codec,
        features,
        heic_analysis_opt,
        analysis_error,
    ) = match deep_result {
        Ok((img, heic_analysis)) => {
            debug!(
                "analyze_heic_file_v4 OK, is_lossless={}",
                heic_analysis.is_lossless
            );

            // 🛡️ RICH AUDIT LOGGING (Restored)
            if heic_analysis.hdr.is_hdr || heic_analysis.hdr.is_dolby_vision {
                let reason = if heic_analysis.hdr.is_dolby_vision {
                    "HEIC with Dolby Vision - skipping to preserve HDR metadata"
                } else {
                    "HEIC with HDR - skipping to preserve HDR metadata"
                };
                return Err(ImgQualityError::SkipFile(reason.to_string()));
            }

            if heic_analysis.aux.has_auxiliary {
                probe_audit!(
                    "heic_auxiliary_image_present",
                    path,
                    "HEIC auxiliary image layer detected",
                );
            }
            if heic_analysis.hdr.has_gainmap {
                crate::log_success!(
                    LABEL_IMAGE,
                    &crate::infra::static_logs::messages::MSG_ANALYZER_HEIC_GAINMAP
                        .replace("{}", &path.display().to_string())
                );
            }
            if heic_analysis.aux.has_vendor_metadata {
                probe_audit!(
                    "heic_vendor_metadata_present",
                    path,
                    "HEIC vendor metadata detected",
                );
            }

            let (w, h) = img.dimensions();
            let feats = calculate_image_features(&img, file_size)?;

            (
                w,
                h,
                heic_analysis.has_alpha,
                heic_analysis.bit_depth,
                heic_analysis.is_lossless,
                heic_analysis.codec.clone(),
                feats,
                Some(heic_analysis),
                None,
            )
        }
        Err(e) => {
            let error_msg = format!("{e}");
            probe_audit!(
                "heic_deep_analysis_failed",
                path,
                "deep HEIC analysis failed: {error_msg}",
                error_msg = error_msg,
            );

            // ROBUST FALLBACK (Deterministic metadata only)
            let is_lossless_fallback = match crate::image_detection::detect_compression(
                &crate::image_detection::DetectedFormat::HEIC,
                path,
            ) {
                Ok(c) => c == crate::image_detection::CompressionType::Lossless,
                Err(de) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "heic_compression_probe_failed",
                        path,
                        format!("compression detection failed: {de}"),
                    );
                    match pixel_fallback_lossless(path) {
                        Ok(lossless) => lossless,
                        Err(err) => {
                            crate::media_conversion_gate::probe_layer_audit(
                                "heic_pixel_fallback_unavailable",
                                path,
                                format!("pixel heuristic unavailable: {err}; treating as lossy"),
                            );
                            false
                        }
                    }
                }
            };
            let fallback_detection = match detect_image(path) {
                Ok(detection) => Some(detection),
                Err(err) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "heic_fallback_detect_image_failed",
                        path,
                        format!("detect_image unavailable during HEIC fallback: {err}"),
                    );
                    None
                }
            };
            let (fallback_width, fallback_height, fallback_has_alpha, fallback_bit_depth) =
                if let Some(canvas) = crate::media_conversion_gate::probe_detection_canvas_optional(
                    path,
                    fallback_detection.as_ref(),
                ) {
                    canvas
                } else {
                    match probe_video(path) {
                        Ok(probe) => (
                            probe.width,
                            probe.height,
                            pix_fmt_has_alpha(&probe.pix_fmt),
                            probe.confirmed_bit_depth(),
                        ),
                        Err(err) => {
                            probe_audit!(
                                "heic_fallback_probe_video_failed",
                                path,
                                "probe_video unavailable during HEIC fallback: {err}",
                                err = err,
                            );
                            return Err(ImgQualityError::AnalysisError(format!(
                                "HEIC fallback: no canvas dimensions after deep analysis failure \
                                 ({error_msg}); probe_video also failed: {err}"
                            )));
                        }
                    }
                };

            (
                fallback_width,
                fallback_height,
                fallback_has_alpha,
                fallback_bit_depth,
                is_lossless_fallback,
                "unknown".to_string(),
                ImageFeatures::default(),
                None,
                Some(error_msg),
            )
        }
    };

    let PreciseColorMetadata {
        color_space,
        color_context,
        precise_bit_depth,
    } = extract_precise_color_metadata(path);
    let (precision, detected_bit_depth) = match detect_image(path) {
        Ok(d) => (d.precision, d.bit_depth),
        Err(e) => {
            probe_audit!(
                "precision_detection_failed",
                path,
                "precision detection failed: {e}; using empty metadata",
                e = e,
            );
            (PrecisionMetadata::default(), None)
        }
    };

    let resolved_color_depth = detected_bit_depth.or(color_depth).or(precise_bit_depth);

    let mut analysis = ImageAnalysis {
        cache_version: IMAGE_ANALYSIS_CACHE_VERSION,
        file_path: path.display().to_string(),
        format: "HEIC".to_string(),
        width,
        height,
        file_size,
        color_depth: resolved_color_depth,
        color_space,
        has_alpha,
        is_animated: false,
        duration_secs: None,
        is_lossless,
        jpeg_analysis: None,
        heic_analysis: heic_analysis_opt,
        features,
        jxl_indicator: JxlIndicator {
            should_convert: false,
            reason: format!("HEIC is already a modern efficient format ({codec})"),
            ..Default::default()
        },
        metadata: extract_metadata(path),
        color_context,
        precision,
        history: crate::common_utils::get_current_history(),
        analysis_error,
        ..Default::default()
    };

    // Phase 2: Physics Extraction
    let (perception, physics_225, recovered_entropy) =
        match extract_universal_physics_and_perception(path) {
            Ok(data) => data,
            Err(e) => {
                probe_audit!(
                    "physics_extraction_failed",
                    path,
                    "universal physics extraction failed: {e}",
                    e = e,
                );
                (Visual::default(), None, None)
            }
        };

    analysis.perception = perception;
    analysis.physics_225 = physics_225;
    if analysis.features.entropy.is_none() {
        analysis.features.entropy = recovered_entropy;
    }
    if analysis.features.compression_ratio.is_none() {
        analysis.features.compression_ratio = estimate_compression_ratio_from_geometry(
            width,
            height,
            has_alpha,
            resolved_color_depth,
            file_size,
        );
    }

    // HEIC animation: same SSOT as img gate (`detect_animation`, not extension).
    match crate::image_detection::detect_animation(
        path,
        &crate::image_detection::DetectedFormat::HEIC,
    ) {
        Ok((animated, _frame_count, _fps)) => {
            analysis.is_animated = animated;
            if animated {
                analysis.duration_secs = get_animation_duration(path);
            }
        }
        Err(e) => {
            analysis.is_animated = false;
            if analysis.analysis_error.is_none() {
                analysis.analysis_error = Some(format!("HEIC animation detection failed: {e}"));
            }
        }
    }

    Ok(analysis)
}

/// Specialized fast path for JPEG files to avoid expensive pixel decoding.
/// JPEG->JXL transcoding only needs quantization tables, not raw pixels.
fn analyze_jpeg_fast_path(path: &Path, file_size: u64) -> ImageAnalysis {
    let jpeg_analysis = match analyze_jpeg_file(path) {
        Ok(analysis) => Some(analysis),
        Err(e) => {
            warn_detail!(
                "JPEG fast-path analysis failed for {}: {}",
                path.display(),
                e
            );
            None
        }
    };

    // Use fast metadata parsing to get dimensions without decoding pixels
    let (width, height) = match image_dimensions_with_magic_bytes(path) {
        Ok(dimensions) => dimensions,
        Err(e) => {
            crate::log_detail!(format!(
                "Analyzer Audit: Failed to extract JPEG dimensions for {path_display}: {e}",
                path_display = path.display(),
            ));
            (0, 0)
        }
    };

    if width == 0 || height == 0 {
        crate::log_detail!(
            crate::infra::static_logs::messages::MSG_JPEG_DIM_FAIL
                .replace("{}", &path.display().to_string())
        );
    }

    let mut metadata = extract_metadata(path);

    if let Some(jpeg) = jpeg_analysis.as_ref()
        && !jpeg.is_complete
    {
        metadata.insert("is_truncated".to_string(), "true".to_string());
        crate::log_detail!(
            crate::infra::static_logs::messages::MSG_JPEG_TRUNCATED
                .replace("{}", &path.display().to_string())
        );
    }
    let jxl_indicator =
        generate_jxl_indicator(ImageFormat::Jpeg, false, jpeg_analysis.as_ref(), path);

    let (psnr, ssim) = (None, None);

    let PreciseColorMetadata {
        color_space,
        color_context,
        precise_bit_depth,
    } = extract_precise_color_metadata(path);
    let detected_bit_depth = match crate::conversion::jpeg_precision_from_header(path) {
        Ok(bit_depth) => bit_depth,
        Err(e) => {
            probe_audit!(
                "jpeg_precision_detection_failed",
                path,
                "JPEG precision detection failed: {e}; using color metadata only",
                e = e,
            );
            None
        }
    };
    let precision = PrecisionMetadata {
        bit_depth: detected_bit_depth.or(precise_bit_depth),
        is_lossless_deterministic: false,
        quality_estimate: jpeg_analysis.as_ref().map(|jpeg| jpeg.estimated_quality),
        ..PrecisionMetadata::default()
    };

    let (perception, physics_225, recovered_entropy) =
        match extract_universal_physics_and_perception(path) {
            Ok(data) => data,
            Err(e) => {
                probe_audit!(
                    "jpeg_physics_extraction_failed",
                    path,
                    "JPEG physics extraction failed: {e}",
                    e = e,
                );
                (Visual::default(), None, None)
            }
        };

    let resolved_color_depth = detected_bit_depth.or(precise_bit_depth);

    ImageAnalysis {
        cache_version: IMAGE_ANALYSIS_CACHE_VERSION,
        file_path: path.display().to_string(),
        format: "JPEG".to_string(),
        width,
        height,
        file_size,
        color_depth: resolved_color_depth,
        color_space,
        has_alpha: false,
        is_animated: false,
        duration_secs: None,
        is_lossless: false,
        jpeg_analysis,
        heic_analysis: None,
        features: ImageFeatures {
            entropy: recovered_entropy,
            compression_ratio: estimate_compression_ratio_from_geometry(
                width,
                height,
                false,
                resolved_color_depth,
                file_size,
            ),
        },
        jxl_indicator,
        psnr,
        ssim,
        metadata,
        color_context,
        precision,
        history: crate::common_utils::get_current_history(),
        perception,
        physics_225,
        analysis_error: None,
    }
}

fn generate_jxl_indicator(
    format: ImageFormat,
    is_lossless: bool,
    jpeg_analysis: Option<&JpegQualityAnalysis>,
    path: &Path,
) -> JxlIndicator {
    let file_path = path.display().to_string();
    let output_path = path.with_extension("jxl").display().to_string();
    let default_effort = crate::constants::JXL_DEFAULT_EFFORT;

    match format {
        ImageFormat::Png | ImageFormat::Gif | ImageFormat::Tiff => JxlIndicator {
            should_convert: true,
            reason: "Lossless image; strongly recommend converting to JXL".to_string(),
            command: format!(
                "cjxl '{file_path}' '{output_path}' -d 0.0 --modular=1 -e {default_effort}"
            ),
            benefit: "30-60% size reduction while preserving full quality".to_string(),
        },
        ImageFormat::Jpeg => {
            use crate::image_jpeg_analysis::is_ultra_hdr_jpeg_file;
            if let Some(jpeg) = jpeg_analysis {
                let quality_info = format!("original quality Q={}", jpeg.estimated_quality);

                // Check if it's an Ultra HDR JPEG (Google Gain Map)
                let is_ultra_hdr = match is_ultra_hdr_jpeg_file(path) {
                    Ok(value) => value,
                    Err(err) => {
                        probe_audit!(
                            "ultrahdr_recommendation_probe_failed",
                            path,
                            "UltraHDR recommendation probe failed: {err}",
                            err = err,
                        );
                        false
                    }
                };
                if is_ultra_hdr {
                    return JxlIndicator {
                        should_convert: true,
                        reason: format!(
                            "Ultra HDR JPEG detected ({quality_info}); recommend gain-map HDR \
                             synthesis"
                        ),
                        command: format!(
                            "cjxl '{file_path}' '{output_path}' --lossless_jpeg=1 -e \
                             {default_effort}"
                        ),
                        benefit: "Produces a single, true 32-bit HDR JXL file (OpenEXR via \
                                  Gainmap mathematics)"
                            .to_string(),
                    };
                }

                JxlIndicator {
                    should_convert: true,
                    reason: format!("JPEG ({quality_info}), lossless encode to JXL"),
                    command: format!(
                        "cjxl '{file_path}' '{output_path}' --lossless_jpeg=1 -e {default_effort}"
                    ),
                    benefit: "Keeps original JPEG DCT coefficients, reversible, ~20% size \
                              reduction"
                        .to_string(),
                }
            } else {
                JxlIndicator {
                    should_convert: true,
                    reason: "JPEG can be losslessly encoded to JXL".to_string(),
                    command: format!(
                        "cjxl '{file_path}' '{output_path}' --lossless_jpeg=1 -e {default_effort}"
                    ),
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
                        "cjxl '{file_path}' '{output_path}' -d 0.0 --modular=1 -e {default_effort}"
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
                        "cjxl '{file_path}' '{output_path}' -d 0.0 --modular=1 -e {default_effort}"
                    ),
                    benefit: "JXL modular mode is typically more efficient than AVIF lossless"
                        .to_string(),
                }
            } else {
                JxlIndicator {
                    should_convert: false,
                    reason: "AVIF is already a modern efficient format; no conversion needed"
                        .to_string(),
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

fn calculate_image_features(img: &DynamicImage, file_size: u64) -> Result<ImageFeatures> {
    let (width, height) = img.dimensions();
    let channels = match img.color() {
        image::ColorType::L8 | image::ColorType::L16 => 1u32,
        image::ColorType::La8 | image::ColorType::La16 => 2u32,
        image::ColorType::Rgb8 | image::ColorType::Rgb16 | image::ColorType::Rgb32F => 3u32,
        _ => 4u32,
    };
    let bits_per_channel = match img.color() {
        image::ColorType::L16
        | image::ColorType::La16
        | image::ColorType::Rgb16
        | image::ColorType::Rgba16 => 16u32,
        image::ColorType::Rgb32F | image::ColorType::Rgba32F => 32u32,
        _ => 8u32,
    };

    let raw_size = u64::from(width)
        * u64::from(height)
        * (u64::from(channels))
        * (u64::from(bits_per_channel) / 8);

    let compression_ratio = if raw_size > 0 {
        #[cfg(feature = "high-precision")]
        {
            Some((Rational::from(file_size) / Rational::from(raw_size)).to_f64())
        }
        #[cfg(not(feature = "high-precision"))]
        {
            Some(
                crate::numeric_cast::u64_to_f64(file_size)
                    / crate::numeric_cast::u64_to_f64(raw_size),
            )
        }
    } else {
        None
    };

    let entropy = Some(calculate_entropy(img)?);

    Ok(ImageFeatures {
        entropy,
        compression_ratio,
    })
}

fn estimate_compression_ratio_from_geometry(
    width: u32,
    height: u32,
    has_alpha: bool,
    color_depth: Option<u8>,
    file_size: u64,
) -> Option<f64> {
    if width == 0 || height == 0 || file_size == 0 {
        return None;
    }

    let channels = if has_alpha { 4u64 } else { 3u64 };
    let bits_per_channel = u64::from(
        crate::media_conversion_gate::color_depth_optional(
            color_depth,
            "estimate_compression_ratio_from_geometry",
        )?
        .max(1),
    );
    let bytes_per_channel = bits_per_channel.div_ceil(8);
    let raw_size = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(channels)
        .saturating_mul(bytes_per_channel);

    if raw_size == 0 {
        return None;
    }

    #[cfg(feature = "high-precision")]
    {
        Some((Rational::from(file_size) / Rational::from(raw_size)).to_f64())
    }
    #[cfg(not(feature = "high-precision"))]
    {
        Some(crate::numeric_cast::u64_to_f64(file_size) / crate::numeric_cast::u64_to_f64(raw_size))
    }
}

fn extract_visual_perception(img: &DynamicImage) -> Visual {
    use image::GenericImageView;
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return Visual::default();
    }

    let luma = img.to_luma8();
    let mut sum_luma = 0.0_f64;
    let mut peak_luma = 0.0_f64;
    let mut sum_x = 0.0_f64;
    let mut sum_y = 0.0_f64;
    let mut total_weight = 0.0_f64;

    for (x, y, pixel) in luma.enumerate_pixels() {
        let val = f64::from(pixel[0]);
        sum_luma += val;
        if val > peak_luma {
            peak_luma = val;
        }

        // Weight for center of mass: treat intensity as mass
        let w = val / 255.0;
        sum_x = f64::from(x).mul_add(w, sum_x);
        sum_y = f64::from(y).mul_add(w, sum_y);
        total_weight += w;
    }

    let total_pixels = f64::from(width) * f64::from(height);
    let avg_luma = (sum_luma / total_pixels) / 255.0;

    let gray_center_of_mass = if total_weight > 0.0 {
        (
            (sum_x / total_weight) / f64::from(width),
            (sum_y / total_weight) / f64::from(height),
        )
    } else {
        (0.5, 0.5)
    };

    Visual {
        average_luma: avg_luma,
        peak_luma: peak_luma / 255.0,
        gray_center_of_mass,
    }
}

fn extract_universal_physics_and_perception(
    path: &Path,
) -> Result<(Visual, Option<Vec<f32>>, Option<f64>)> {
    // Attempt 1: Native decode (fastest, most accurate)
    match open_image_reader_with_magic_bytes(path) {
        Ok(reader) => match reader.decode() {
            Ok(img) => {
                return Ok((
                    extract_visual_perception(&img),
                    Some(crate::real_physics::extract_image_physics_225(&img)),
                    Some(calculate_entropy(&img)?),
                ));
            }
            Err(err) => {
                probe_audit!(
                    "native_decode_physics_failed",
                    path,
                    "native decode failed for physics/perception extraction: {err}",
                    err = err,
                );
            }
        },
        Err(err) => {
            probe_audit!(
                "native_reader_physics_open_failed",
                path,
                "native reader open failed for physics/perception extraction: {err}",
                err = err,
            );
        }
    }

    // Attempt 2: FFmpeg fallback (handles HEIC, JXL, AVIF)
    let output = crate::ffmpeg_builder::FfmpegBuilder::new()
        .input(path)
        .frames_v(1)
        .arg("-vf")
        .arg("crop=min(iw\\,512):min(ih\\,512)")
        .format("image2")
        .arg("-vcodec")
        .arg("mjpeg")
        .output_pipe()
        .build()
        .output()?;

    if output.status.success() && !output.stdout.is_empty() {
        let img = image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Jpeg)
            .map_err(|e| {
                ImgQualityError::AnalysisError(format!(
                    "Failed to decode FFmpeg sampled image: {e}"
                ))
            })?;
        return Ok((
            extract_visual_perception(&img),
            Some(crate::real_physics::extract_image_physics_225(&img)),
            Some(calculate_entropy(&img)?),
        ));
    }

    Err(ImgQualityError::AnalysisError(format!(
        "Universal physics extraction failed for {}",
        path.display()
    )))
}

fn calculate_entropy(img: &DynamicImage) -> Result<f64> {
    let gray = img.to_luma8();
    let pixels = gray.as_raw();
    if pixels.is_empty() {
        return Err(ImgQualityError::AnalysisError(
            "Empty pixel buffer for entropy calculation".into(),
        ));
    }

    let mut histogram = [0u64; 256];
    for &pixel in pixels {
        if let Some(h) = histogram.get_mut(usize::from(pixel)) {
            *h += 1;
        }
    }

    let total = crate::numeric_cast::usize_to_f64(pixels.len());
    let mut entropy = 0.0_f64;

    for &count in &histogram {
        if count > 0 {
            let p = crate::numeric_cast::u64_to_f64(count) / total;
            entropy = p.mul_add(-p.log2(), entropy);
        }
    }
    Ok(entropy)
}

fn format_to_string(format: ImageFormat) -> String {
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
        ImageFormat::Qoi => "QOI".to_string(),
        _ => format!("{format:?}"),
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

fn pix_fmt_has_alpha(pix_fmt: &str) -> bool {
    let pix_fmt = pix_fmt.to_lowercase();
    pix_fmt.contains("yuva")
        || pix_fmt.contains("rgba")
        || pix_fmt.contains("gbrap")
        || pix_fmt.starts_with("p4")
}

fn is_animated_format(path: &Path, format: ImageFormat) -> Result<bool> {
    match format {
        ImageFormat::Gif => check_gif_animation(path),
        ImageFormat::WebP => check_webp_animation(path),
        ImageFormat::Png => check_png_animation(path),
        _ => Ok(false),
    }
}

fn check_png_animation(path: &Path) -> Result<bool> {
    crate::common_utils::validate_file_size_limit(
        path,
        crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
    )
    .map_err(|error| ImgQualityError::AnalysisError(error.to_string()))?;
    let bytes = std::fs::read(path)?;
    Ok(crate::image::png_validation::parse_apng_animation(&bytes)?
        .is_some_and(|info| info.frame_count > 1))
}

fn check_gif_animation(path: &Path) -> Result<bool> {
    crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;
    let bytes = std::fs::read(path)?;

    // Stage 1: Structural Count (spec-compliant chunk walking)
    let structural_count = crate::image_formats::gif::count_frames_from_bytes(&bytes)?;
    if structural_count > 1 {
        return Ok(true);
    }

    // Stage 2: Feature Scan (Signal B)
    // Look for GCE markers [0x21, 0xF9, 0x04] globally
    let gce_marker = &[0x21, 0xF9, 0x04];
    let gce_hints = crate::numeric_cast::usize_to_u32_strict(
        bytes.windows(3).filter(|w| *w == gce_marker).count(),
        "gce_hints",
    )
    .ok_or_else(|| {
        ImgQualityError::AnalysisError(
            "Numerical anomaly in GCE hint count; information invalidated".to_string(),
        )
    })?;

    if gce_hints > structural_count {
        // [Disagreement] Internal Deep Research
        if deep_research_gif_animation(&bytes, gce_hints) {
            crate::log_detail!(format!(
                "Analyzer Audit: Initiating GIF deep frame research \
                 (structural={structural_count}, gce_hints={gce_hints}) for {path_display}",
                path_display = path.display(),
            ));
            return Ok(true);
        }
    }

    // Stage 3: Penetrating decode fallback (ground-truth frame count)
    // Some edge GIFs can bypass structural/GCE heuristics; use decode-based
    // verification only when metadata is suspiciously static to keep cost
    // bounded.
    if structural_count <= 1
        && let crate::media_penetration::PenetrationResult::Verified(real_count) =
            crate::media_penetration::detect_real_frame_count(
                path,
                Some(u64::from(structural_count)),
            )
        && real_count > 1
    {
        crate::log_detail!(format!(
            "Analyzer Audit: Deep GIF penetration testing active (structural={structural_count}, \
             real={real_count}) for {path_display}",
            path_display = path.display(),
        ));
        return Ok(true);
    }

    Ok(structural_count > 1)
}

fn check_webp_animation(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    let structural_count = crate::image_formats::webp::count_frames_from_bytes(&bytes)?;
    Ok(structural_count > 1)
}

/// Internal Deep Research: GIF
/// Validates if GCE markers are consistent with GIF block structure.
fn deep_research_gif_animation(bytes: &[u8], gce_hints: u32) -> bool {
    if gce_hints <= 1 {
        return false;
    }

    // Look for GCE patterns and verify if they are followed by valid block
    // terminators GCE = [21 F9 04 ... 00]
    let mut confirmed = 0_i32;
    let mut i = 0;
    while i + 7 < bytes.len() {
        if bytes.get(i..i + 3) == Some(&[0x21, 0xF9, 0x04]) && bytes.get(i + 7) == Some(&0x00) {
            confirmed += 1_i32;
        }
        i += 1;
    }

    confirmed > 1
}

/// Public entry for retrying animation duration (e.g. from main when
/// `analysis.duration_secs` is None). Tries ffprobe, `ImageMagick`, WebP native
/// parse, and GIF frame-count estimate.
#[must_use]
pub fn get_animation_duration_for_path(path: &Path) -> Option<f32> {
    get_animation_duration(path)
}

fn get_animation_duration(path: &Path) -> Option<f32> {
    let format = crate::image::format_detect::detect_true_format(path).ok()?;

    if format == crate::image::format_detect::FormatKind::Gif {
        match crate::image_formats::gif::get_timing_stats(path) {
            Ok(Some(stats)) => {
                return Some(crate::numeric_cast::f64_to_f32_lossy(stats.duration_secs));
            }
            Ok(None) => {}
            Err(err) => {
                probe_audit!(
                    "gif_timing_stats_read_failed",
                    path,
                    "GIF timing stats probe failed: {err}",
                    err = err,
                );
            }
        }
    }

    if format == crate::image::format_detect::FormatKind::Png {
        match std::fs::read(path) {
            Ok(data) => {
                if let Some(stats) = crate::image_detection::apng_timing_stats_from_bytes(&data) {
                    return Some(crate::numeric_cast::f64_to_f32_lossy(stats.duration_secs));
                }
            }
            Err(err) => {
                probe_audit!(
                    "apng_timing_read_failed",
                    path,
                    "APNG timing read failed: {err}",
                    err = err,
                );
            }
        }
    }

    if format == crate::image::format_detect::FormatKind::WebP {
        match std::fs::read(path) {
            Ok(data) => match crate::image_formats::webp::timing_stats_from_bytes(&data) {
                Ok(Some(stats)) => {
                    return Some(crate::numeric_cast::f64_to_f32_lossy(stats.duration_secs));
                }
                Ok(None) => {}
                Err(err) => {
                    probe_audit!(
                        "webp_timing_stats_parse_failed",
                        path,
                        "WebP timing stats probe failed: {err}",
                        err = err,
                    );
                }
            },
            Err(err) => {
                probe_audit!(
                    "webp_timing_read_failed",
                    path,
                    "WebP timing read failed: {err}",
                    err = err,
                );
            }
        }
    }

    let mut final_duration = None;

    // Special handling for JXL: FFmpeg's jpegxl_anim decoder is incomplete
    // Convert to temporary APNG first, then probe duration
    if format == crate::image::format_detect::FormatKind::Jxl {
        match try_jxl_via_apng(path) {
            Ok(duration) => final_duration = duration,
            Err(err) => {
                probe_audit!(
                    "jxl_duration_probe_failed",
                    path,
                    "JXL duration probe failed: {err}",
                    err = err,
                );
            }
        }
    } else {
        match try_ffprobe_json(path) {
            Ok(Some(duration)) => final_duration = Some(duration),
            Ok(None) => {}
            Err(err) => {
                probe_audit!(
                    "ffprobe_json_duration_failed",
                    path,
                    "ffprobe JSON duration probe failed before default fallback: {err}",
                    err = err,
                );
            }
        }
        if final_duration.is_none() {
            match try_ffprobe_default(path) {
                Ok(Some(duration)) => final_duration = Some(duration),
                Ok(None) => {}
                Err(err) => {
                    probe_audit!(
                        "ffprobe_default_duration_failed",
                        path,
                        "ffprobe default duration probe failed: {err}",
                        err = err,
                    );
                }
            }
        }
    }

    if format != crate::image::format_detect::FormatKind::Jxl
        && final_duration.is_none()
        && let Some(duration) = try_imagemagick_identify(path)
    {
        final_duration = Some(duration);
    }

    if format == crate::image::format_detect::FormatKind::WebP && final_duration.is_none() {
        match std::fs::read(path) {
            Ok(data) => {
                if let Some(secs) = crate::image_formats::webp::duration_secs_from_bytes(&data) {
                    final_duration = Some(secs);
                }
            }
            Err(err) => {
                probe_audit!(
                    "webp_duration_read_failed",
                    path,
                    "WebP duration read failed: {err}",
                    err = err,
                );
            }
        }
    }

    if let Some(d) = final_duration {
        // Enforce the single-frame static image check for ALL modern formats (WebP,
        // AVIF, HEIC, etc.) If the duration is suspiciously short (e.g., <
        // 0.25s) but not already 0, we run an exact packet count. A duration of
        // 0.04s is exactly 1 frame at 25fps.
        if d > 0.0 && d < crate::constants::DURATION_THRESHOLD_SUSPICIOUS {
            match try_get_frame_count(path) {
                Ok(Some(frame_count)) if frame_count <= 1 => {
                    crate::log_detail!(
                        &crate::infra::static_logs::messages::MSG_ANALYZER_STATIC_MEDIA
                            .replace("{}", &path.display().to_string())
                    );
                    return Some(crate::constants::DURATION_UNKNOWN_PLACEHOLDER_SECS);
                }
                Ok(_) => {}
                Err(err) => {
                    probe_audit!(
                        "suspicious_duration_frame_count_failed",
                        path,
                        "frame-count probe failed while checking suspicious duration: {err}",
                        err = err,
                    );
                }
            }
        }
        return Some(d);
    }

    if format == crate::image::format_detect::FormatKind::Gif {
        match try_get_frame_count(path) {
            Ok(Some(frame_count)) => {
                if frame_count <= 1 {
                    crate::log_detail!(
                        &crate::infra::static_logs::messages::MSG_ANALYZER_STATIC_GIF
                            .replace("{}", &path.display().to_string())
                    );
                    return Some(crate::constants::DURATION_UNKNOWN_PLACEHOLDER_SECS);
                }
                probe_audit!(
                    "gif_duration_unavailable",
                    path,
                    "animated GIF duration unavailable after probing (frames={frame_count}); \
                     leaving unknown",
                    frame_count = frame_count,
                );
                return None;
            }
            Ok(None) => {}
            Err(err) => {
                probe_audit!(
                    "gif_duration_frame_count_failed",
                    path,
                    "GIF frame-count fallback failed: {err}",
                    err = err,
                );
            }
        }
    }

    None
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn try_jxl_via_apng(path: &Path) -> Result<Option<f32>> {
    // Check if djxl is available
    if which::which("djxl").is_err() {
        crate::media_conversion_gate::probe_layer_audit(
            "djxl_missing_for_jxl_duration",
            path,
            "djxl not found; cannot process animated JXL",
        );
        return Ok(None);
    }

    // Create temporary APNG file
    let temp_apng = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "image_analyzer_jxl_apng",
        None,
        Some(".apng"),
    )
    .map_err(|e| {
        probe_audit!(
            "jxl_temp_apng_create_failed",
            path,
            "failed to create temporary APNG: {e}",
            e = e,
        );
        e
    })?;
    let temp_apng_path = temp_apng.path();

    // Convert JXL to APNG using djxl
    let djxl_result = crate::jxl_builder::DjxlBuilder::new()
        .input(path)
        .output(temp_apng_path)
        .build()
        .output()
        .map_err(|e| {
            probe_audit!(
                "djxl_launch_failed",
                path,
                "failed to launch djxl for bitstream research: {e}",
                e = e,
            );
            ImgQualityError::AnalysisError(format!(
                "failed to launch djxl for {}: {e}",
                path.display()
            ))
        })?;

    if !djxl_result.status.success() || !temp_apng_path.exists() {
        crate::media_conversion_gate::probe_layer_audit(
            "djxl_jxl_to_apng_failed",
            path,
            crate::infra::static_logs::messages::MSG_ANALYZER_DJXL_FAIL,
        );
        return Ok(None);
    }

    crate::log_info!(
        messages::LABEL_DETECTION,
        "JXL detected, converted to temporary APNG for duration detection"
    );

    // APNG doesn't have duration in format metadata, we need to calculate from
    // frames and fps Use ffprobe with -count_frames to get nb_read_frames
    let probe_output = crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(temp_apng_path)
        .loglevel(constants::FFMPEG_LOGLEVEL_ERROR)
        .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
        .count_frames()
        .show_entries("stream=nb_read_frames,r_frame_rate")
        .print_format(constants::FFMPEG_PRINT_FORMAT_JSON)
        .build()
        .output()
        .map_err(|e| {
            crate::log_detail!(format!(
                "Analyzer Audit: APNG deep research failed for {apng_path}; falling back to \
                 static assumptions: {e}",
                apng_path = temp_apng_path.display(),
            ));
            ImgQualityError::AnalysisError(format!(
                "failed to launch ffprobe for temporary APNG {}: {e}",
                temp_apng_path.display()
            ))
        })?;

    if probe_output.status.success() {
        let json_str = String::from_utf8_lossy(&probe_output.stdout);
        match serde_json::from_str::<serde_json::Value>(&json_str) {
            Ok(json) => {
                if let Some(stream) = json
                    .get("streams")
                    .and_then(|s| s.as_array())
                    .and_then(|s| s.first())
                {
                    let Some(nb_frames_raw) = stream.get("nb_read_frames").and_then(|v| v.as_str())
                    else {
                        crate::media_conversion_gate::probe_layer_audit(
                            "jxl_nb_read_frames_missing",
                            path,
                            "nb_read_frames missing from ffprobe output; skipping duration \
                             calculation",
                        );
                        return Ok(None);
                    };
                    let nb_frames = nb_frames_raw.parse::<u64>().map_err(|err| {
                        ImgQualityError::AnalysisError(format!(
                            "failed to parse JXL temporary APNG frame count {nb_frames_raw:?}: \
                             {err}"
                        ))
                    })?;

                    let Some(r_frame_rate) = stream.get("r_frame_rate").and_then(|v| v.as_str())
                    else {
                        let _ = crate::media_conversion_gate::probe_r_frame_rate_optional(path);
                        return Ok(None);
                    };

                    // Parse frame rate (format: "num/den")
                    let fps = match crate::ffprobe::parse_frame_rate(r_frame_rate) {
                        Ok(v) => Some(v),
                        Err(e) => crate::media_conversion_gate::probe_fps_parse_optional(
                            r_frame_rate,
                            path,
                            e,
                        ),
                    };

                    if nb_frames > 0
                        && let Some(fps) = fps.filter(|rate| rate.is_finite() && *rate > 0.0)
                    {
                        let duration = crate::numeric_cast::u64_to_f64(nb_frames) / fps;
                        crate::log_detail!(format!(
                            "Analyzer Audit: JPEG XL animation detected via codestream probe \
                             (frames={nb_frames}, fps={fps:.2}, duration={duration:.2}s)",
                        ));
                        return Ok(Some(crate::numeric_cast::f64_to_f32_lossy(duration)));
                    }
                }
            }
            Err(err) => {
                warn_detail!(
                    "Failed to parse ffprobe JSON for temporary APNG {}: {}",
                    temp_apng_path.display(),
                    err
                );
                return Err(ImgQualityError::AnalysisError(format!(
                    "failed to parse ffprobe JSON for temporary APNG {}: {err}",
                    temp_apng_path.display()
                )));
            }
        }
    } else {
        warn_detail!(
            "ffprobe returned non-zero status for temporary APNG {}: {}",
            temp_apng_path.display(),
            String::from_utf8_lossy(&probe_output.stderr).trim()
        );
    }

    // Fallback: try ffprobe methods
    // temp_apng will be automatically cleaned up when dropped
    match try_ffprobe_json(temp_apng_path)? {
        Some(v) => Ok(Some(v)),
        None => try_ffprobe_default(temp_apng_path),
    }
}

fn try_ffprobe_json(path: &Path) -> Result<Option<f32>> {
    let output = crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(path)
        .loglevel(constants::FFMPEG_LOGLEVEL_ERROR)
        .print_format(constants::FFMPEG_PRINT_FORMAT_JSON)
        .show_format()
        .build()
        .output()
        .map_err(|e| {
            warn_detail!(
                "Failed to launch ffprobe JSON probe for {}: {}",
                path.display(),
                e
            );
            ImgQualityError::AnalysisError(format!(
                "failed to launch ffprobe JSON probe for {}: {e}",
                path.display()
            ))
        })?;

    if !output.status.success() {
        warn_detail!(
            "ffprobe JSON probe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(ImgQualityError::AnalysisError(format!(
            "ffprobe JSON probe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    // Parse with serde_json — the hand-rolled string-offset approach breaks on
    // whitespace variants (e.g. `"duration" : "1.5"`) and is completely unnecessary
    // since serde_json is already a project dependency.
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        warn_detail!("Failed to parse ffprobe JSON for {}: {}", path.display(), e);
        ImgQualityError::AnalysisError(format!(
            "failed to parse ffprobe JSON for {}: {e}",
            path.display()
        ))
    })?;

    let Some(duration_str) = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
    else {
        return Ok(None);
    };

    match duration_str.parse::<f32>() {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            warn_detail!(
                "Failed to parse ffprobe JSON duration '{}' for {}: {}",
                duration_str,
                path.display(),
                e
            );
            Err(ImgQualityError::AnalysisError(format!(
                "failed to parse ffprobe JSON duration {duration_str:?} for {}: {e}",
                path.display()
            )))
        }
    }
}

fn try_ffprobe_default(path: &Path) -> Result<Option<f32>> {
    let output = crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(path)
        .loglevel(constants::FFMPEG_LOGLEVEL_ERROR)
        .show_entries("format=duration")
        .print_format("default=noprint_wrappers=1:nokey=1")
        .build()
        .output()
        .map_err(|e| {
            warn_detail!(
                "Failed to launch ffprobe default duration probe for {}: {}",
                path.display(),
                e
            );
            ImgQualityError::AnalysisError(format!(
                "failed to launch ffprobe default duration probe for {}: {e}",
                path.display()
            ))
        })?;

    if !output.status.success() {
        warn_detail!(
            "ffprobe default duration probe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(ImgQualityError::AnalysisError(format!(
            "ffprobe default duration probe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() {
        return Ok(None);
    }
    match duration_str.parse::<f32>() {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            warn_detail!(
                "Failed to parse ffprobe default duration '{}' for {}: {}",
                duration_str,
                path.display(),
                e
            );
            Err(ImgQualityError::AnalysisError(format!(
                "failed to parse ffprobe default duration {duration_str:?} for {}: {e}",
                path.display()
            )))
        }
    }
}

/// Returns (`duration_secs`, `frame_count`) from `ImageMagick` `identify
/// -format "%T"`.
///
/// Works for any format `ImageMagick` can read and that has per-frame delay
/// (e.g. GIF, WebP, AVIF, JXL, APNG). Use as fallback when ffprobe has no
/// stream/format duration. Emits a warning log when used.
#[must_use]
pub fn get_animation_duration_and_frames_imagemagick(path: &Path) -> Option<(f64, u64)> {
    warn_detail!(
        "[Duration Fallback] Using ImageMagick identify for animation duration: {}",
        path.display()
    );

    let output = match crate::image_builders::MagickBuilder::new()
        .arg("identify")
        .arg("-format")
        .arg("%T\n")
        .input(path)
        .build()
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            warn_detail!(
                "[Duration Fallback] Failed to spawn ImageMagick identify for {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn_detail!(
            "[Duration Fallback] ImageMagick identify failed for {}: {}",
            path.display(),
            stderr.trim()
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut total_cs = 0u32;
    let mut frame_count = 0u32;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse::<u32>() {
            Ok(delay_cs) => {
                total_cs += delay_cs;
                frame_count += 1;
            }
            Err(e) => {
                warn_detail!(
                    "[Duration Fallback] Failed to parse delay '{}' for {}: {}",
                    trimmed,
                    path.display(),
                    e
                );
            }
        }
    }

    if frame_count == 0 {
        warn_detail!(
            "[Duration Fallback] ImageMagick identify returned 0 frames for {}",
            path.display()
        );
        return None;
    }

    let duration_secs = f64::from(total_cs) / 100.0_f64;
    crate::media_conversion_gate::probe_imagemagick_animation_detected_audit(
        path,
        u64::from(frame_count),
        duration_secs,
    );
    info_detail!(
        "[Duration Fallback] ImageMagick animation detected: {} frames, {:.2}s ({})",
        frame_count,
        duration_secs,
        path.display()
    );
    Some((duration_secs, u64::from(frame_count)))
}

fn try_imagemagick_identify(path: &Path) -> Option<f32> {
    if let Some((duration_secs, frame_count)) = get_animation_duration_and_frames_imagemagick(path)
    {
        crate::media_conversion_gate::ui_probe_stats_stderr(format!(
            "ImageMagick: animation detected ({frame_count} frames, {duration_secs:.2}s)"
        ));
        return Some(crate::numeric_cast::f64_to_f32_lossy(duration_secs));
    }
    None
}

fn try_get_frame_count(path: &Path) -> Result<Option<u32>> {
    let output = crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(path)
        .loglevel(constants::FFMPEG_LOGLEVEL_ERROR)
        .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
        .count_frames()
        .show_entries("stream=nb_read_packets")
        .print_format("csv=p=0")
        .build()
        .output()
        .map_err(|e| {
            warn_detail!(
                "Failed to launch ffprobe frame-count probe for {}: {}",
                path.display(),
                e
            );
            ImgQualityError::AnalysisError(format!(
                "failed to launch ffprobe frame-count probe for {}: {e}",
                path.display()
            ))
        })?;

    if !output.status.success() {
        warn_detail!(
            "ffprobe frame-count probe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(ImgQualityError::AnalysisError(format!(
            "ffprobe frame-count probe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let count_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if count_str.is_empty() {
        return Ok(None);
    }
    count_str.parse::<u32>().map(Some).map_err(|e| {
        warn_detail!(
            "Failed to parse ffprobe frame count '{}' for {}: {}",
            count_str,
            path.display(),
            e
        );
        ImgQualityError::AnalysisError(format!(
            "failed to parse ffprobe frame count {count_str:?} for {}: {e}",
            path.display()
        ))
    })
}

/// Determines if the image is stored in a lossless way for conversion
/// decisions. Uses `image_detection::detect_compression` for PNG, TIFF, WebP,
/// AVIF (and HEIC/JXL in their own analyzers).
fn detect_lossless(format: ImageFormat, path: &Path) -> Result<bool> {
    use crate::image_detection::{
        CompressionType, DetectedFormat, detect_compression, detect_format_from_bytes,
    };

    match format {
        ImageFormat::Png => {
            if super::png_validation::is_true_png(path)? {
                return Ok(true);
            }
            let detected_format = detect_format_from_bytes(path)?;
            let compression = detect_compression(&detected_format, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        // GIF uses palette quantization — inherently lossless for its own 256-color space.
        // BMP, Pnm, Tga, Hdr, Farbfeld are all uncompressed/lossless pixel formats.
        // QOI is lossless-only by design.
        ImageFormat::Gif
        | ImageFormat::Bmp
        | ImageFormat::Pnm
        | ImageFormat::Tga
        | ImageFormat::Hdr
        | ImageFormat::Farbfeld
        | ImageFormat::Qoi => Ok(true),
        // ICO can embed quantized PNGs — route through dedicated detector
        ImageFormat::Ico => {
            let compression = detect_compression(&DetectedFormat::ICO, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        // OpenEXR supports both lossless (NONE/RLE/ZIP/PIZ) and lossy (PXR24/B44/DWAA/DWAB)
        // Must parse the compression attribute — do not assume lossless.
        ImageFormat::OpenExr => {
            let compression = detect_compression(&DetectedFormat::EXR, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        ImageFormat::Tiff => {
            let compression = detect_compression(&DetectedFormat::TIFF, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        ImageFormat::WebP => check_webp_lossless(path),
        ImageFormat::Avif => {
            let compression = detect_compression(&DetectedFormat::AVIF, path)?;
            Ok(compression == CompressionType::Lossless)
        }
        // Any unknown future format: be conservative — don't assume lossless.
        _ => Ok(false),
    }
}

fn check_webp_lossless(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    Ok(crate::image_formats::webp::is_lossless_from_bytes(&bytes))
}

/// Returns `true` when `MFB_ENABLE_PIXEL_HEURISTIC` is set to a truthy value.
/// Default: `false` (pixel heuristics disabled).
#[must_use]
fn pixel_heuristic_enabled() -> bool {
    match std::env::var("MFB_ENABLE_PIXEL_HEURISTIC") {
        Ok(v) => matches!(
            v.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        _ => false,
    }
}

/// Pixel-level fallback for `is_lossless` when format-level detection returns
/// Err or is unavailable.
///
/// **Default off.** Enable with `MFB_ENABLE_PIXEL_HEURISTIC=1`.
/// When disabled, callers must handle the format-level `Err` directly.
fn pixel_fallback_lossless(path: &Path) -> Result<bool> {
    // Gate: pixel heuristics disabled by default to prevent silent guessing.
    if !pixel_heuristic_enabled() {
        return Err(ImgQualityError::AnalysisError(format!(
            "pixel heuristic disabled (set MFB_ENABLE_PIXEL_HEURISTIC=1 to enable) for {}",
            path.display()
        )));
    }

    warn_detail!(
        "[Lossless Fallback] Format-level detection failed; using pixel-level heuristic for {}",
        path.display()
    );

    let analysis = crate::image_quality_detector::analyze_image_quality_from_path(path)
        .ok_or_else(|| {
            ImgQualityError::ImageReadError(format!(
                "Failed to analyze pixel-level quality for {}",
                path.display()
            ))
        })?;

    let affinity = analysis.lossless_affinity_score().ok_or_else(|| {
        ImgQualityError::NumericError(format!(
            "Lossless affinity calculation failed for {}: missing quality metrics",
            path.display()
        ))
    })?;

    let is_confident = analysis
        .confidence
        .is_some_and(|c| c >= crate::constants::HEURISTIC_LOSSLESS_CONFIDENCE_MIN);
    let adopted = is_confident && affinity >= crate::constants::AFFINITY_THRESHOLD_LOSSLESS;

    // "Loud and Honest" Logging: Provide full transparency for the heuristic
    // decision
    crate::log_detail!(
        "[{}] Affinity: {:.3} (threshold: {}) │ Confident: {} │ Complexity: {} │ Edges: {} │ \
         Color: {} │ Noise: {} │ Type: {}",
        crate::infra::static_logs::messages::LABEL_HEURISTIC,
        affinity,
        crate::constants::AFFINITY_THRESHOLD_LOSSLESS,
        is_confident,
        crate::media_conversion_gate::ui_f64_or_na(analysis.complexity, "heuristic_complexity", 3,),
        crate::media_conversion_gate::ui_f64_or_na(
            analysis.edge_density,
            "heuristic_edge_density",
            3,
        ),
        crate::media_conversion_gate::ui_f64_or_na(
            analysis.color_diversity,
            "heuristic_color_diversity",
            3,
        ),
        crate::media_conversion_gate::ui_f64_or_na(
            analysis.noise_level,
            "heuristic_noise_level",
            3,
        ),
        analysis.content_type.name
    );

    if adopted {
        probe_audit!(
            "pixel_lossless_heuristic_adopted",
            path,
            "classified as lossless via pixel heuristic (affinity: {affinity:.3})",
            affinity = affinity,
        );
    }

    Ok(adopted)
}

fn is_jxl_file(path: &Path) -> Result<bool> {
    // Rely strictly on magic bytes to avoid extension mismatch false positives
    let bytes = std::fs::read(path).map_err(|err| {
        probe_audit!(
            "jxl_magic_read_failed",
            path,
            "JXL magic read failed: {err}",
            err = err,
        );
        ImgQualityError::AnalysisError(format!(
            "JXL magic read failed for {}: {err}",
            path.display()
        ))
    })?;
    if bytes.get(0..2) == Some(b"\xFF\x0A") {
        return Ok(true);
    }
    if bytes.len() >= 12 && bytes.get(4..8) == Some(b"JXL ") {
        return Ok(true);
    }
    Ok(false)
}

type JxlCanvas = (u32, u32, bool, Option<u8>);

fn resolve_jxl_canvas_from_ffprobe(path: &Path) -> Result<JxlCanvas> {
    let probe = probe_video(path).map_err(|err| {
        probe_audit!(
            "jxl_canvas_ffprobe_failed",
            path,
            "ffprobe JXL canvas probe failed: {err}",
            err = err,
        );
        ImgQualityError::AnalysisError(format!(
            "ffprobe JXL canvas probe failed for {}: {err}",
            path.display()
        ))
    })?;
    Ok((
        probe.width,
        probe.height,
        pix_fmt_has_alpha(&probe.pix_fmt),
        probe.confirmed_bit_depth(),
    ))
}

fn resolve_jxl_canvas(path: &Path) -> Result<JxlCanvas> {
    use crate::builder_base::ToolBuilder;

    if crate::tool_builders::JxlinfoBuilder::new().check_available() {
        let output = crate::tool_builders::JxlinfoBuilder::new()
            .input(path)
            .build()
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                match parse_jxlinfo_output(&stdout) {
                    Ok(v) => return Ok(v),
                    Err(e) => {
                        let _ = crate::media_conversion_gate::probe_jxlinfo_dimensions_optional(
                            path, e,
                        );
                    }
                }
            }
            Ok(_) => {
                let _ = crate::media_conversion_gate::probe_jxlinfo_dimensions_optional(
                    path,
                    "jxlinfo command failed",
                );
            }
            Err(_) => {
                let _ = crate::media_conversion_gate::probe_jxlinfo_dimensions_optional(
                    path,
                    "jxlinfo command spawn failed",
                );
            }
        }
    }
    match std::fs::read(path) {
        Ok(data) => {
            match ::jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(&data)) {
                Ok(image) => {
                    let width = image.width();
                    let height = image.height();
                    let has_alpha =
                        image.image_header().metadata.ec_info.iter().any(|info| {
                            matches!(info.ty, ::jxl_oxide::ExtraChannelType::Alpha { .. })
                        });
                    let bit_depth = if let Some(b) = crate::numeric_cast::u32_to_u8_strict(
                        image.image_header().metadata.bit_depth.bits_per_sample(),
                        "jxl_bit_depth",
                    ) {
                        Some(b)
                    } else {
                        tracing::warn!(target: "jxl_oxide_probe", "Invalid bit depth parsed in jxl metadata");
                        None
                    };
                    return Ok((width, height, has_alpha, bit_depth));
                }
                Err(err) => {
                    tracing::debug!(
                        target: "jxl_oxide_probe",
                        path = %path.display(),
                        error = %err,
                        "jxl-oxide failed to parse codestream; falling back to ffprobe"
                    );
                }
            }
        }
        Err(err) => {
            tracing::debug!(
                target: "jxl_oxide_probe",
                path = %path.display(),
                error = %err,
                "failed to read file for jxl-oxide; falling back to ffprobe"
            );
        }
    }
    resolve_jxl_canvas_from_ffprobe(path).inspect_err(|_err| {
        crate::log_detail!(crate::infra::static_logs::messages::MSG_ANALYZER_JXLLINFO_SUGGESTION);
    })
}

fn analyze_jxl_image(path: &Path, file_size: u64) -> Result<ImageAnalysis> {
    use crate::image_detection::{DetectedFormat, detect_animation};

    let (width, height, has_alpha, color_depth) = resolve_jxl_canvas(path)?;

    let metadata = extract_metadata(path);

    let is_lossless = match crate::image_detection::detect_compression(
        &crate::image_detection::DetectedFormat::JXL,
        path,
    ) {
        Ok(c) => c == crate::image_detection::CompressionType::Lossless,
        Err(_) => match pixel_fallback_lossless(path) {
            Ok(v) => v,
            Err(e) => crate::media_conversion_gate::probe_pixel_lossless_or_false(path, e),
        },
    };

    let PreciseColorMetadata {
        color_space,
        color_context,
        precise_bit_depth,
    } = extract_precise_color_metadata(path);

    // Detect animation via ffprobe/jxlinfo. If detection fails we record the
    // failure in `analysis_error`; the compatibility bool stays false, but that
    // is not a measured static verdict and duration_secs remains unknown.
    let (is_animated, animation_error) = match detect_animation(path, &DetectedFormat::JXL) {
        Ok((animated, _frame_count, _fps)) => (animated, None),
        Err(e) => {
            probe_audit!(
                "jxl_animation_detection_failed",
                path,
                "JXL animation detection failed: {e}; recording uncertainty",
                e = e,
            );
            (false, Some(format!("JXL animation detection failed: {e}")))
        }
    };
    let duration_secs = if is_animated {
        get_animation_duration(path)
    } else {
        None
    };

    let (perception, physics_225, recovered_entropy) =
        match extract_universal_physics_and_perception(path) {
            Ok(data) => data,
            Err(e) => {
                probe_audit!(
                    "jxl_physics_extraction_failed",
                    path,
                    "JXL physics extraction failed: {e}",
                    e = e,
                );
                (Visual::default(), None, None)
            }
        };

    let (precision, detected_bit_depth) = match detect_image(path) {
        Ok(d) => (d.precision, d.bit_depth),
        Err(e) => {
            probe_audit!(
                "jxl_precision_detection_failed",
                path,
                "JXL precision detection failed: {e}; using empty metadata",
                e = e,
            );
            (PrecisionMetadata::default(), None)
        }
    };

    let resolved_color_depth = detected_bit_depth.or(color_depth).or(precise_bit_depth);

    Ok(ImageAnalysis {
        cache_version: IMAGE_ANALYSIS_CACHE_VERSION,
        file_path: path.display().to_string(),
        format: "JXL".to_string(),
        width,
        height,
        file_size,
        color_depth: resolved_color_depth,
        color_space,
        has_alpha,
        is_animated,
        duration_secs,
        is_lossless,
        jpeg_analysis: None,
        heic_analysis: None,
        features: ImageFeatures {
            entropy: recovered_entropy,
            compression_ratio: estimate_compression_ratio_from_geometry(
                width,
                height,
                has_alpha,
                resolved_color_depth,
                file_size,
            ),
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
        color_context,
        precision,
        history: crate::common_utils::get_current_history(),
        perception,
        physics_225,
        analysis_error: animation_error,
    })
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn analyze_avif_image(path: &Path, file_size: u64) -> ImageAnalysis {
    use crate::image_detection::{
        CompressionType, DetectedFormat, detect_animation, detect_compression,
    };

    // Use ffprobe directly for AVIF: the `image` crate's AVIF decoder rejects many
    // valid files (10-bit, HDR color spaces, certain profiles). ffprobe handles
    // them correctly and also provides pix_fmt for accurate alpha and bit-depth
    // detection.
    let (width, height, has_alpha, color_depth): (u32, u32, bool, Option<u8>) =
        match probe_video(path) {
            Ok(probe) => {
                let pix_fmt = probe.pix_fmt.to_lowercase();
                let alpha = pix_fmt.contains("yuva")
                    || pix_fmt.contains("rgba")
                    || pix_fmt.contains("gbrap")
                    || pix_fmt.starts_with("p4");
                if probe.confirmed_bit_depth().is_none() {
                    info_detail!(
                        "ffprobe did not report explicit bit_depth for AVIF {}; recording \
                         color_depth as unknown (no forgery from pix_fmt inference)",
                        path.display()
                    );
                }
                (
                    probe.width,
                    probe.height,
                    alpha,
                    probe.confirmed_bit_depth(),
                )
            }
            Err(probe_err) => match crate::image_detection::open_image_with_limits(path) {
                Ok(img) => {
                    warn_detail!(
                        "ffprobe AVIF probe failed for {}; falling back to image decode: {}",
                        path.display(),
                        probe_err
                    );
                    let (w, h) = img.dimensions();
                    (
                        w,
                        h,
                        has_alpha_channel(&img),
                        match crate::conversion::media_info_without_ffprobe(path) {
                            Ok(info) => info.and_then(|info| info.bit_depth),
                            Err(err) => {
                                probe_audit!(
                                    "avif_media_info_fallback_failed",
                                    path,
                                    "AVIF media-info fallback failed after image decode: {err}",
                                    err = err,
                                );
                                None
                            }
                        },
                    )
                }
                Err(image_err) => {
                    warn_detail!(
                        "Both ffprobe and image decode failed for AVIF {}: ffprobe={}, image={}",
                        path.display(),
                        probe_err,
                        image_err
                    );
                    (0u32, 0u32, false, None)
                }
            },
        };

    let is_lossless = match detect_compression(&DetectedFormat::AVIF, path) {
        Ok(ct) => ct == CompressionType::Lossless,
        Err(e) => {
            warn_detail!(
                "AVIF compression analysis failed for {}; falling back to pixel heuristic: {}",
                path.display(),
                e
            );
            match pixel_fallback_lossless(path) {
                Ok(v) => v,
                Err(pe) => crate::media_conversion_gate::probe_pixel_lossless_or_false(path, pe),
            }
        }
    };

    let PreciseColorMetadata {
        color_space,
        color_context,
        precise_bit_depth,
    } = extract_precise_color_metadata(path);

    // Detect animation via ISOBMFF ftyp brand (avis/msf1). On failure we record
    // the error in `analysis_error`; the compatibility bool stays false, but that
    // is not a measured "static, 1 frame" fact and duration_secs remains unknown.
    let (is_animated, animation_error) = match detect_animation(path, &DetectedFormat::AVIF) {
        Ok((animated, _frame_count, _fps)) => (animated, None),
        Err(e) => {
            probe_audit!(
                "avif_animation_detection_failed",
                path,
                "AVIF animation detection failed: {e}; recording uncertainty",
                e = e,
            );
            (false, Some(format!("AVIF animation detection failed: {e}")))
        }
    };
    let duration_secs = if is_animated {
        get_animation_duration(path)
    } else {
        None
    };

    let metadata = extract_metadata(path);
    // Extract real physics and perception
    let (perception, physics_225, recovered_entropy) =
        match extract_universal_physics_and_perception(path) {
            Ok(data) => data,
            Err(e) => {
                probe_audit!(
                    "avif_deep_analysis_failed",
                    path,
                    "AVIF deep analysis failed: {e}",
                    e = e,
                );
                (Visual::default(), None, None)
            }
        };

    let (precision, detected_bit_depth) = match detect_image(path) {
        Ok(d) => (d.precision, d.bit_depth),
        Err(e) => {
            probe_audit!(
                "avif_precision_detection_failed",
                path,
                "AVIF precision detection failed: {e}; using empty metadata",
                e = e,
            );
            (PrecisionMetadata::default(), None)
        }
    };

    let resolved_color_depth = detected_bit_depth.or(color_depth).or(precise_bit_depth);

    ImageAnalysis {
        cache_version: IMAGE_ANALYSIS_CACHE_VERSION,
        file_path: path.display().to_string(),
        format: "AVIF".to_string(),
        width,
        height,
        file_size,
        color_depth: resolved_color_depth,
        color_space,
        has_alpha,
        is_animated,
        duration_secs,
        is_lossless,
        jpeg_analysis: None,
        heic_analysis: None,
        features: ImageFeatures {
            entropy: recovered_entropy,
            compression_ratio: estimate_compression_ratio_from_geometry(
                width,
                height,
                has_alpha,
                resolved_color_depth,
                file_size,
            ),
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
        color_context,
        precision,
        history: crate::common_utils::get_current_history(),
        perception,
        physics_225,
        analysis_error: animation_error,
    }
}

fn parse_jxlinfo_output(
    output: &str,
) -> crate::unified_error::Result<(u32, u32, bool, Option<u8>)> {
    let mut width = 0u32;
    let mut height = 0u32;
    let mut has_alpha = false;
    let mut color_depth = None;

    for line in output.lines() {
        let line = line.trim();

        if let Some(dims) = line
            .split(',')
            .find(|s| s.contains('x') && s.chars().any(|c| c.is_ascii_digit()))
        {
            let dims = dims.trim();
            let parts: Vec<&str> = dims.split('x').collect();
            if let (Some(w_part), Some(h_part)) = (parts.first(), parts.get(1)) {
                let w_str: String = w_part.chars().filter(char::is_ascii_digit).collect();
                let h_str: String = h_part.chars().filter(char::is_ascii_digit).collect();
                width = crate::numeric_cast::parse_strict::<u32>(&w_str, "jxlinfo_width")
                    .ok_or_else(|| {
                        crate::unified_error::UnifiedError::NumericError(format!(
                            "Failed to parse width from jxlinfo output: {w_str}"
                        ))
                    })?;
                height = crate::numeric_cast::parse_strict::<u32>(&h_str, "jxlinfo_height")
                    .ok_or_else(|| {
                        crate::unified_error::UnifiedError::NumericError(format!(
                            "Failed to parse height from jxlinfo output: {h_str}"
                        ))
                    })?;
            }
        }

        if line.contains("alpha") && !line.contains("no alpha") {
            has_alpha = true;
        }

        if let Some(bit_depth) = parse_jxlinfo_bit_depth(line) {
            color_depth = Some(bit_depth);
        }
    }

    Ok((width, height, has_alpha, color_depth))
}

fn parse_jxlinfo_bit_depth(line: &str) -> Option<u8> {
    let marker = line.find("-bit")?;
    let digits_rev: String = line[..marker]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits_rev.is_empty() {
        return None;
    }

    let digits: String = digits_rev.chars().rev().collect();
    crate::numeric_cast::parse_strict::<u8>(&digits, "jxlinfo_bit_depth")
}

fn extract_metadata(path: &Path) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();

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

    // Orientation, Make, Model extraction
    let mut builder = crate::ExiftoolBuilder::new();
    builder
        .arg("-s") // Use -s (short names) to get "Tag: Value"
        .arg("-Orientation")
        .arg("-Make")
        .arg("-Model")
        .input(path);
    match builder.build().output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_lowercase();
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        metadata.insert(key, value);
                    }
                }
            }
        }
        Err(err) => {
            probe_audit!(
                "exiftool_metadata_probe_failed",
                path,
                "ExifTool metadata probe failed: {err}",
                err = err,
            );
        }
    }

    // ISOBMFF structural metadata (irot/imir)
    match std::fs::read(path) {
        Ok(data) => {
            let isobmff_meta = crate::common_utils::extract_isobmff_metadata(&data);
            metadata.extend(isobmff_meta);
        }
        Err(err) => {
            probe_audit!(
                "isobmff_metadata_read_failed",
                path,
                "ISOBMFF structural metadata read failed: {err}",
                err = err,
            );
        }
    }

    metadata
}

struct PreciseColorMetadata {
    color_space: Option<String>,
    color_context: ConversionColorContext,
    precise_bit_depth: Option<u8>,
}

fn precise_color_metadata_from_color_info(
    ext_lower: Option<&str>,
    fallback_media_bit_depth: Option<u8>,
    color_info: ColorInfo,
) -> PreciseColorMetadata {
    let color_space = color_info.color_space.clone();
    let precise_bit_depth = color_info.confirmed_bit_depth();
    let assessment = color_info.assessment();
    let precision_profile =
        ImagePrecisionProfile::from_media_context(ext_lower, &color_info, fallback_media_bit_depth);

    // `color_context` is a carrier for HDR signaling, wide-gamut color info, and
    // precision-preservation hints used by downstream conversion paths.
    let should_carry_conversion_color =
        precision_profile.should_preserve_high_precision() || assessment.has_wide_gamut_signal();
    let color_context = if should_carry_conversion_color {
        if assessment.has_hdr_signaling() {
            ConversionColorContext::true_hdr(color_info)
        } else {
            ConversionColorContext::precision_or_wide_gamut_hint(color_info)
        }
    } else {
        ConversionColorContext::default()
    };

    PreciseColorMetadata {
        color_space,
        color_context,
        precise_bit_depth,
    }
}

fn extract_precise_color_metadata(path: &Path) -> PreciseColorMetadata {
    let color_info = crate::ffprobe_json::extract_color_info(path);
    let ext_lower = path.extension().and_then(|ext| ext.to_str());
    let fallback_media_bit_depth = match crate::conversion::media_info_without_ffprobe(path) {
        Ok(info) => info.and_then(|info| info.bit_depth),
        Err(err) => {
            probe_audit!(
                "precise_color_media_info_fallback_failed",
                path,
                "precise color bit-depth fallback failed: {err}",
                err = err,
            );
            None
        }
    };

    precise_color_metadata_from_color_info(ext_lower, fallback_media_bit_depth, color_info)
}

#[must_use]
pub fn get_recommendation(analysis: &ImageAnalysis) -> UpgradeRecommendation {
    let indicator = &analysis.jxl_indicator;
    format_recommendation(indicator, &analysis.format, analysis.is_lossless)
}

/// 🚀 New Entry Point: Subscribes to `MediaIndexRow` (Database-driven decision)
///
/// # Errors
/// Returns an error if the recommendation cannot be generated.
pub fn get_recommendation_from_row(row: &MediaIndexRow) -> Result<UpgradeRecommendation> {
    let features: DetectionResult = serde_json::from_str(&row.raw_features_json).map_err(|e| {
        ImgQualityError::AnalysisError(format!("Failed to parse features JSON: {e}"))
    })?;
    let is_lossless = features.compression == CompressionType::Lossless;

    let indicator = jxl_indicator_from_features(&features, &row.rel_path);

    Ok(format_recommendation(&indicator, &row.format, is_lossless))
}

fn format_recommendation(
    indicator: &JxlIndicator,
    format: &str,
    is_lossless: bool,
) -> UpgradeRecommendation {
    if indicator.should_convert {
        UpgradeRecommendation {
            current_format: format.to_string(),
            recommended_format: "JXL".to_string(),
            reason: indicator.reason.clone(),
            expected_size_reduction: if is_lossless {
                crate::constants::EXPECTED_REDUCTION_LOSSLESS_JXL
            } else {
                crate::constants::EXPECTED_REDUCTION_LOSSY_JXL
            },
            quality_preservation: if is_lossless {
                "Mathematically Lossless".to_string()
            } else {
                "Lossless JPEG Transcode".to_string()
            },
            command: indicator.command.clone(),
        }
    } else {
        UpgradeRecommendation {
            current_format: format.to_string(),
            recommended_format: format.to_string(),
            reason: indicator.reason.clone(),
            expected_size_reduction: 0.0,
            quality_preservation: crate::media_conversion_gate::ui_metric_not_applicable_label(
                "upgrade_recommendation_quality_preservation",
            ),
            command: String::new(),
        }
    }
}

/// Build a `JxlIndicator` from indexed DB features, mirroring
/// `generate_jxl_indicator`. This is the production code path used when the
/// analyzer output is not in scope (DB-driven flow).
fn jxl_indicator_from_features(features: &DetectionResult, rel_path: &str) -> JxlIndicator {
    let output_path = format!("{rel_path}.jxl");
    let is_lossless = features.compression == CompressionType::Lossless;
    let default_effort = crate::constants::JXL_DEFAULT_EFFORT;

    match features.format {
        DetectedFormat::PNG | DetectedFormat::GIF | DetectedFormat::TIFF => JxlIndicator {
            should_convert: true,
            reason: "Lossless image; strongly recommend converting to JXL".to_string(),
            command: format!(
                "cjxl '{rel_path}' '{output_path}' -d 0.0 --modular=1 -e {default_effort}"
            ),
            benefit: crate::constants::JXL_BENEFIT_DESCRIPTION.to_string(),
        },
        DetectedFormat::JPEG => JxlIndicator {
            should_convert: true,
            reason: "JPEG can be losslessly encoded to JXL".to_string(),
            command: format!(
                "cjxl '{rel_path}' '{output_path}' --lossless_jpeg=1 -e {default_effort}"
            ),
            benefit: "Keeps original JPEG DCT coefficients, reversible".to_string(),
        },
        DetectedFormat::WebP if is_lossless => JxlIndicator {
            should_convert: true,
            reason: "Lossless WebP; recommend converting to JXL".to_string(),
            command: format!(
                "cjxl '{rel_path}' '{output_path}' -d 0.0 --modular=1 -e {default_effort}"
            ),
            benefit: "JXL is typically more efficient than lossless WebP".to_string(),
        },
        _ => JxlIndicator {
            should_convert: false,
            reason: "Already efficient or unsupported conversion".to_string(),
            command: String::new(),
            benefit: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_detection::{
        CompressionType, DetectedFormat, DetectionResult, ImageType, PrecisionMetadata,
    };

    #[test]
    fn extensionless_jpeg_uses_content_detected_fast_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("extensionless");
        let image = image::RgbImage::from_pixel(3, 2, image::Rgb([24, 96, 192]));
        image
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .expect("write JPEG fixture without an extension");

        let analysis = analyze_image_internal(&path).expect("analyze extensionless JPEG");

        assert_eq!(analysis.format, "JPEG");
        assert_eq!((analysis.width, analysis.height), (3, 2));
        assert!(analysis.jpeg_analysis.is_some());
        assert!(analysis.precision.quality_estimate.is_some());
    }

    #[test]
    fn jxl_canvas_ffprobe_errors_are_not_collapsed_to_none() {
        let err = resolve_jxl_canvas_from_ffprobe(Path::new("missing.jxl")).unwrap_err();

        assert!(
            err.to_string().contains("ffprobe JXL canvas probe failed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn jxl_magic_read_errors_are_not_false_negatives() {
        let err = is_jxl_file(Path::new("missing.jxl")).unwrap_err();

        assert!(
            err.to_string().contains("JXL magic read failed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn try_ffprobe_json_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.webp");

        let err =
            try_ffprobe_json(&missing).expect_err("missing ffprobe JSON target must be an error");

        assert!(err.to_string().contains("missing.webp"));
    }

    #[test]
    fn try_get_frame_count_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.gif");

        let err = try_get_frame_count(&missing)
            .expect_err("missing ffprobe frame-count target must be an error");

        assert!(err.to_string().contains("missing.gif"));
    }

    fn estimate_psnr_from_quality(quality: u8) -> f64 {
        match quality {
            crate::constants::JPEG_QUALITY_TIER_HIGH..=100 => {
                (f64::from(quality) - f64::from(crate::constants::JPEG_QUALITY_TIER_HIGH)).mul_add(
                    crate::constants::JPEG_MAP_PSNR_H95_SLOPE,
                    crate::constants::JPEG_QUALITY_MAPPING_V1_PSNR_BASE,
                )
            }
            crate::constants::JPEG_QUALITY_TIER_MEDIUM_HIGH..=94 => (f64::from(quality)
                - f64::from(crate::constants::JPEG_QUALITY_TIER_MEDIUM_HIGH))
            .mul_add(
                crate::constants::JPEG_MAP_PSNR_H85_SLOPE,
                crate::constants::JPEG_MAP_PSNR_H85_BASE,
            ),
            crate::constants::JPEG_QUALITY_TIER_MEDIUM..=84 => (f64::from(quality)
                - f64::from(crate::constants::JPEG_QUALITY_TIER_MEDIUM))
            .mul_add(
                crate::constants::JPEG_MAP_PSNR_H75_SLOPE,
                crate::constants::JPEG_MAP_PSNR_H75_BASE,
            ),
            crate::constants::JPEG_QUALITY_TIER_LOW..=74 => {
                (f64::from(quality) - f64::from(crate::constants::JPEG_QUALITY_TIER_LOW)).mul_add(
                    crate::constants::JPEG_MAP_PSNR_H60_SLOPE,
                    crate::constants::JPEG_MAP_PSNR_H60_BASE,
                )
            }
            _ => f64::from(quality).mul_add(
                crate::constants::JPEG_MAP_PSNR_LOW_SLOPE,
                crate::constants::JPEG_MAP_PSNR_LOW_BASE,
            ),
        }
    }

    fn estimate_ssim_from_quality(quality: u8) -> f64 {
        match quality {
            crate::constants::JPEG_QUALITY_TIER_HIGH..=100 => {
                (f64::from(quality) - f64::from(crate::constants::JPEG_QUALITY_TIER_HIGH)).mul_add(
                    crate::constants::JPEG_MAP_SSIM_H95_SLOPE,
                    crate::constants::JPEG_QUALITY_MAPPING_V1_SSIM_BASE,
                )
            }
            crate::constants::JPEG_QUALITY_TIER_MEDIUM_HIGH..=94 => (f64::from(quality)
                - f64::from(crate::constants::JPEG_QUALITY_TIER_MEDIUM_HIGH))
            .mul_add(
                crate::constants::JPEG_MAP_SSIM_H85_SLOPE,
                crate::constants::JPEG_MAP_SSIM_H85_BASE,
            ),
            crate::constants::JPEG_QUALITY_TIER_MEDIUM..=84 => (f64::from(quality)
                - f64::from(crate::constants::JPEG_QUALITY_TIER_MEDIUM))
            .mul_add(
                crate::constants::JPEG_MAP_SSIM_H75_SLOPE,
                crate::constants::JPEG_MAP_SSIM_H75_BASE,
            ),
            crate::constants::JPEG_QUALITY_TIER_LOW..=74 => {
                (f64::from(quality) - f64::from(crate::constants::JPEG_QUALITY_TIER_LOW)).mul_add(
                    crate::constants::JPEG_MAP_SSIM_H60_SLOPE,
                    crate::constants::JPEG_MAP_SSIM_H60_BASE,
                )
            }
            _ => f64::from(quality).mul_add(
                crate::constants::JPEG_MAP_SSIM_LOW_SLOPE,
                crate::constants::JPEG_MAP_SSIM_LOW_BASE,
            ),
        }
    }

    #[test]
    fn test_psnr_estimation() {
        let psnr_high = estimate_psnr_from_quality(95);
        let psnr_mid = estimate_psnr_from_quality(75);
        let psnr_low = estimate_psnr_from_quality(50);

        assert!(psnr_high > psnr_mid);
        assert!(psnr_mid > psnr_low);
        assert!(psnr_high >= 40.0_f64);
        assert!(psnr_low >= 25.0_f64);
    }

    #[test]
    fn test_ssim_estimation() {
        let ssim_high = estimate_ssim_from_quality(95);
        let ssim_mid = estimate_ssim_from_quality(75);
        let ssim_low = estimate_ssim_from_quality(50);

        assert!(ssim_high > ssim_mid);
        assert!(ssim_mid > ssim_low);
        assert!(ssim_high >= 0.95_f64);
        assert!(ssim_low >= 0.70_f64);
    }

    #[test]
    fn test_quality_boundaries() {
        let psnr_max = estimate_psnr_from_quality(100);
        let psnr_min = estimate_psnr_from_quality(1);

        assert!(psnr_max > psnr_min);
        assert!(psnr_max.is_finite());
        assert!(psnr_min.is_finite());
    }

    #[test]
    fn test_png_recommendation() {
        let analysis = ImageAnalysis {
            file_path: "test.png".to_string(),
            format: "PNG".to_string(),
            width: 1920,
            height: 1080,
            file_size: 1_000_000,
            color_depth: Some(8),
            color_space: None,
            has_alpha: false,
            is_animated: false,
            duration_secs: None,
            is_lossless: true,
            jpeg_analysis: None,
            heic_analysis: None,
            features: ImageFeatures {
                entropy: Some(7.5),
                compression_ratio: Some(0.5),
            },
            jxl_indicator: JxlIndicator {
                should_convert: true,
                reason: "Lossless image; strongly recommend converting to JXL".to_string(),
                command: "cjxl 'test.png' 'test.jxl' -d 0.0 -e 7".to_string(),
                benefit: crate::constants::JXL_BENEFIT_DESCRIPTION.to_string(),
            },
            psnr: None,
            ssim: None,
            metadata: HashMap::new(),
            color_context: ConversionColorContext::default(),
            precision: PrecisionMetadata::default(),
            history: ProcessHistory::default(),
            perception: Visual::default(),
            physics_225: None,
            analysis_error: None,
            cache_version: 0,
        };

        let rec = get_recommendation(&analysis);
        assert_eq!(rec.recommended_format, "JXL");
        assert_eq!(rec.quality_preservation, "Mathematically Lossless");
    }

    #[test]
    fn test_jpeg_recommendation() {
        let features = DetectionResult {
            file_path: "test.jpg".to_string(),
            format: DetectedFormat::JPEG,
            image_type: ImageType::Static,
            compression: CompressionType::Lossy,
            width: 1920,
            height: 1080,
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 500_000,
            frame_count: None,
            fps: None,
            duration: None,
            estimated_quality: Some(85),
            entropy: Some(7.2),
            precision: PrecisionMetadata::default(),
        };

        let indicator = jxl_indicator_from_features(&features, "test.jpg");
        assert!(indicator.should_convert);
        assert!(indicator.reason.contains("losslessly encoded"));
        assert!(indicator.command.contains("--lossless_jpeg=1"));
    }

    #[test]
    fn test_jxl_recommendation_negative() {
        let features = DetectionResult {
            file_path: "test.jxl".to_string(),
            format: DetectedFormat::JXL,
            image_type: ImageType::Static,
            compression: CompressionType::Lossless,
            width: 1920,
            height: 1080,
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 400_000,
            frame_count: None,
            fps: None,
            duration: None,
            estimated_quality: None,
            entropy: None,
            precision: PrecisionMetadata::default(),
        };

        let indicator = jxl_indicator_from_features(&features, "test.jxl");
        assert!(!indicator.should_convert);
        assert!(indicator.reason.contains("Already efficient"));
    }

    #[test]
    fn test_webp_lossy_recommendation_negative() {
        let features = DetectionResult {
            file_path: "test.webp".to_string(),
            format: DetectedFormat::WebP,
            image_type: ImageType::Static,
            compression: CompressionType::Lossy,
            width: 1920,
            height: 1080,
            bit_depth: Some(8),
            has_alpha: false,
            file_size: 300_000,
            frame_count: None,
            fps: None,
            duration: None,
            estimated_quality: Some(80),
            entropy: Some(7.0),
            precision: PrecisionMetadata::default(),
        };

        let indicator = jxl_indicator_from_features(&features, "test.webp");
        assert!(!indicator.should_convert);
    }

    #[test]
    fn test_parse_jxlinfo_output_handles_arbitrary_bit_depth() {
        let parsed = parse_jxlinfo_output("JPEG XL image, 3840x2160, 12-bit, alpha")
            .unwrap_or_else(|e| panic!("parse_jxlinfo_output failed: {e:?}"));

        assert_eq!(parsed, (3840, 2160, true, Some(12)));
    }

    #[test]
    fn test_parse_jxlinfo_bit_depth_ignores_non_bit_lines() {
        assert_eq!(parse_jxlinfo_bit_depth("JPEG XL image, 3840x2160"), None);
    }

    #[test]
    fn test_precise_color_metadata_carries_float_container_precision_hint() {
        let metadata =
            precise_color_metadata_from_color_info(Some("hdr"), None, ColorInfo::default());

        assert!(metadata.color_context.conversion_color_info().is_some());
        assert_eq!(
            metadata.color_context.role(),
            Some(ConversionColorRole::PrecisionOrWideGamutHint)
        );
        assert_eq!(metadata.precise_bit_depth, None);
    }

    #[test]
    fn test_precise_color_metadata_carries_unknown_tiff_precision_hint() {
        let metadata =
            precise_color_metadata_from_color_info(Some("tiff"), None, ColorInfo::default());

        assert!(metadata.color_context.conversion_color_info().is_some());
        assert!(!metadata.color_context.has_true_hdr_metadata());
        assert!(metadata.color_context.is_precision_or_wide_gamut_hint());
        assert_eq!(metadata.precise_bit_depth, None);
    }

    #[test]
    fn test_precise_color_metadata_keeps_wide_gamut_bt2020_metadata() {
        let metadata = precise_color_metadata_from_color_info(
            Some("png"),
            None,
            ColorInfo {
                bit_depth: Some(8),
                color_primaries: Some("bt2020".to_string()),
                ..Default::default()
            },
        );

        assert!(metadata.color_context.conversion_color_info().is_some());
        assert_eq!(
            metadata.color_context.role(),
            Some(ConversionColorRole::PrecisionOrWideGamutHint)
        );
        assert_eq!(metadata.precise_bit_depth, Some(8));
    }

    #[test]
    fn test_image_analysis_hdr_helpers_distinguish_true_hdr_from_precision_hints() {
        let precision_hint_only = ImageAnalysis {
            color_context: ColorInfo {
                bit_depth: Some(10),
                bit_depth_inferred_from_pix_fmt: true,
                ..Default::default()
            }
            .into(),
            ..Default::default()
        };
        assert!(precision_hint_only.has_precision_or_hdr_hints());
        assert!(!precision_hint_only.has_true_hdr_metadata());
        assert_eq!(
            precision_hint_only.conversion_color_role(),
            Some(ConversionColorRole::PrecisionOrWideGamutHint)
        );
        assert!(precision_hint_only.conversion_color_info().is_some());

        let true_hdr = ImageAnalysis {
            color_context: ColorInfo {
                bit_depth: Some(10),
                color_transfer: Some("smpte2084".to_string()),
                color_primaries: Some("bt2020".to_string()),
                ..Default::default()
            }
            .into(),
            ..Default::default()
        };
        assert!(true_hdr.has_precision_or_hdr_hints());
        assert!(true_hdr.has_true_hdr_metadata());
        assert_eq!(
            true_hdr.conversion_color_role(),
            Some(ConversionColorRole::TrueHdrMetadata)
        );
        assert!(true_hdr.conversion_color_info().is_some());
    }

    #[test]
    fn test_conversion_color_context_does_not_promote_empty_color_info_into_hint() {
        let context = ConversionColorContext::from(ColorInfo::default());

        assert!(!context.has_precision_or_hdr_hints());
        assert!(!context.has_true_hdr_metadata());
        assert_eq!(context.role(), None);
        assert!(context.conversion_color_info().is_none());
    }

    #[test]
    fn test_conversion_color_context_serializes_as_legacy_option_shape() {
        let serialized = serde_json::to_value(
            ConversionColorContext::precision_or_wide_gamut_hint(ColorInfo {
                bit_depth: Some(10),
                bit_depth_inferred_from_pix_fmt: true,
                ..Default::default()
            }),
        )
        .unwrap_or_else(|e| panic!("color context serialization failed: {e}"));

        assert_eq!(serialized["bit_depth"], serde_json::json!(10));
        assert_eq!(
            serialized["bit_depth_inferred_from_pix_fmt"],
            serde_json::json!(true)
        );
        assert!(serialized.get("role").is_none());
    }

    #[test]
    fn test_image_analysis_deserializes_legacy_hdr_info_field_into_color_context() {
        let mut legacy_value = serde_json::to_value(ImageAnalysis {
            color_context: ColorInfo {
                bit_depth: Some(10),
                bit_depth_inferred_from_pix_fmt: true,
                ..Default::default()
            }
            .into(),
            ..Default::default()
        })
        .unwrap_or_else(|e| panic!("analysis serialization failed: {e}"));
        let color_context = legacy_value
            .as_object_mut()
            .and_then(|value| value.remove("color_context"))
            .unwrap_or_else(|| panic!("serialized analysis should contain color_context"));
        legacy_value
            .as_object_mut()
            .unwrap_or_else(|| panic!("serialized analysis should be an object"))
            .insert("hdr_info".to_string(), color_context);

        let analysis: ImageAnalysis = serde_json::from_value(legacy_value)
            .unwrap_or_else(|e| panic!("legacy hdr_info alias should deserialize: {e}"));

        assert!(analysis.has_precision_or_hdr_hints());
        assert!(!analysis.has_true_hdr_metadata());
        assert_eq!(
            analysis.conversion_color_role(),
            Some(ConversionColorRole::PrecisionOrWideGamutHint)
        );
        assert_eq!(
            analysis
                .conversion_color_info()
                .and_then(|info| info.bit_depth),
            Some(10)
        );
    }

    #[test]
    fn test_quality_summary_does_not_claim_hdr_from_bit_depth_alone() {
        let analysis = ImageAnalysis {
            file_path: "test.heic".to_string(),
            format: "heic".to_string(),
            width: 1920,
            height: 1080,
            file_size: 500_000,
            color_depth: Some(10),
            color_space: None,
            has_alpha: false,
            is_animated: false,
            duration_secs: None,
            is_lossless: false,
            jpeg_analysis: None,
            heic_analysis: Some(HeicAnalysis {
                bit_depth: Some(10),
                codec: "HEVC".to_string(),
                is_lossless: false,
                has_alpha: false,
                image_count: 1,
                hdr: crate::image_heic_analysis::HeicHdrInfo::default(),
                aux: crate::image_heic_analysis::HeicAuxInfo::default(),
            }),
            features: ImageFeatures {
                entropy: None,
                compression_ratio: None,
            },
            jxl_indicator: JxlIndicator {
                should_convert: false,
                reason: String::new(),
                command: String::new(),
                benefit: String::new(),
            },
            psnr: None,
            ssim: None,
            metadata: HashMap::new(),
            color_context: ConversionColorContext::default(),
            precision: PrecisionMetadata::default(),
            history: ProcessHistory::default(),
            perception: Visual::default(),
            physics_225: None,
            analysis_error: None,
            cache_version: 0,
        };

        assert_eq!(analysis.quality_summary(), "HEVC 10-bit");
    }
}
