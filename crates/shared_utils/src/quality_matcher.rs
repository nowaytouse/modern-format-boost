//! Quality Matcher Module
//!
//! Unified quality matching algorithm for all `modern_format_boost` tools.
//! Calculates optimal encoding parameters (CRF/distance) based on input quality analysis.

#[cfg(feature = "high-precision")]
use rug::Rational;
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderType {
    Av1,
    Hevc,
    Jxl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceCodec {
    H264,
    H265,
    Vvc,
    Vp8,
    Vp9,
    Av1,
    Av2,

    Mpeg4,
    Mpeg2,
    Mpeg1,
    Wmv,
    Theora,
    RealVideo,
    FlashVideo,

    ProRes,
    DnxHD,
    Mjpeg,

    Ffv1,
    UtVideo,
    HuffYuv,
    RawVideo,
    Lagarith,
    MagicYuv,

    Gif,
    Apng,
    WebpAnimated,

    Jpeg,
    JpegXl,
    Png,
    WebpStatic,
    Avif,
    Heic,
    Bmp,
    Tiff,

    #[default]
    Unknown,
}

impl SourceCodec {
    /// Relative encoding efficiency vs. H.264 (1.0). Lower value = more efficient at same quality.
    /// H.265/HEVC ≈ 0.65 and AV1 ≈ 0.50 are empirical from bitrate comparison studies; no single
    /// canonical reference—values tuned for CRF mapping consistency across codecs.
    #[must_use]
    pub const fn efficiency_factor(&self) -> f64 {
        match self {
            Self::Av1 => 0.50,
            Self::Vp9 => 0.70,
            Self::Vp8 => 0.85,
            Self::Vvc | Self::Av2 => 0.35,
            Self::H265 | Self::Heic => 0.65,

            Self::Mpeg4 => 1.3,
            Self::Mpeg1 | Self::Mjpeg => 2.5,
            Self::Wmv => 1.1,
            Self::Theora | Self::Tiff => 1.2,
            Self::RealVideo => 2.0,
            Self::FlashVideo | Self::Png => 1.5,
            Self::Mpeg2 | Self::ProRes | Self::DnxHD | Self::Apng => 1.8,

            Self::Gif | Self::Bmp => 3.0,
            Self::WebpAnimated => 0.9,
            Self::JpegXl => 0.6,
            Self::WebpStatic => 0.75,
            Self::Avif => 0.55,

            Self::H264
            | Self::Ffv1
            | Self::UtVideo
            | Self::HuffYuv
            | Self::RawVideo
            | Self::Lagarith
            | Self::MagicYuv
            | Self::Jpeg
            | Self::Unknown => 1.0,
        }
    }

    #[must_use]
    pub const fn is_lossless(&self) -> bool {
        matches!(
            self,
            Self::Ffv1
                | Self::UtVideo
                | Self::HuffYuv
                | Self::RawVideo
                | Self::Lagarith
                | Self::MagicYuv
                | Self::Png
                | Self::Apng
                | Self::Bmp
        )
    }

    #[must_use]
    pub const fn is_modern(&self) -> bool {
        matches!(
            self,
            Self::H265
                | Self::Av1
                | Self::Vp9
                | Self::Vvc
                | Self::Av2
                | Self::JpegXl
                | Self::Avif
                | Self::Heic
                | Self::WebpStatic
                | Self::WebpAnimated
        )
    }

    #[must_use]
    pub const fn is_cutting_edge(&self) -> bool {
        matches!(self, Self::Vvc | Self::Av2)
    }

    #[must_use]
    pub const fn is_animated(&self) -> bool {
        matches!(self, Self::Gif | Self::Apng | Self::WebpAnimated)
    }

    /// Returns true if the format is known to support animation (GIF, APNG, WebP, AVIF, HEIC, JXL).
    #[must_use]
    pub const fn can_be_animated(&self) -> bool {
        matches!(
            self,
            Self::Gif | Self::Apng | Self::WebpAnimated | Self::Avif | Self::Heic | Self::JpegXl
        )
    }

    #[must_use]
    pub const fn supports_alpha(&self) -> bool {
        matches!(
            self,
            Self::Png
                | Self::Apng
                | Self::WebpStatic
                | Self::WebpAnimated
                | Self::Avif
                | Self::Heic
                | Self::JpegXl
                | Self::Gif
                | Self::ProRes
                | Self::UtVideo
                | Self::HuffYuv
                | Self::Ffv1
                | Self::RawVideo
                | Self::MagicYuv
        )
    }

    #[must_use]
    pub const fn is_image(&self) -> bool {
        matches!(
            self,
            Self::Jpeg
                | Self::JpegXl
                | Self::Png
                | Self::WebpStatic
                | Self::Avif
                | Self::Heic
                | Self::Bmp
                | Self::Tiff
                | Self::Gif
                | Self::Apng
                | Self::WebpAnimated
        )
    }

    #[must_use]
    pub const fn is_video(&self) -> bool {
        !self.is_image() && !matches!(self, Self::Unknown)
    }

    /// Complete list of all supported image extensions.
    #[must_use]
    pub const fn supported_image_extensions() -> &'static [&'static str] {
        &[
            "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif",
            "avif", "bmp", "ico", "svg", "jp2", "j2k", "jxl",
        ]
    }

    /// Image extensions that should be collected for conversion (excludes JXL/AVIF/HEIC depending on tool).
    #[must_use]
    pub const fn image_extensions_for_convert() -> &'static [&'static str] {
        &[
            "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "tiff", "tif", "heic", "heif",
            "avif", "bmp", "ico", "svg", "jp2", "j2k", "jxl",
        ]
    }

    /// Video extensions supported by the pipeline.
    #[must_use]
    pub const fn supported_video_extensions() -> &'static [&'static str] {
        &[
            "mp4", "mov", "avi", "mkv", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "ts", "mts",
            "m2ts", "m2v", "3gp", "3g2", "ogv", "f4v", "asf", "gif", "webp", "avif", "heic",
            "heif", "apng", "png", "jxl",
        ]
    }

    /// Returns the canonical/default file extension for this codec.
    #[must_use]
    pub const fn default_extension(&self) -> &'static str {
        match self {
            Self::H264
            | Self::H265
            | Self::Vvc
            | Self::Vp8
            | Self::Vp9
            | Self::Av1
            | Self::Av2
            | Self::Mpeg4 => "mp4",
            Self::Mpeg2 | Self::Mpeg1 => "mpg",
            Self::Wmv => "wmv",
            Self::Theora => "ogv",
            Self::RealVideo => "rm",
            Self::FlashVideo => "flv",
            Self::ProRes | Self::DnxHD => "mov",
            Self::Ffv1
            | Self::UtVideo
            | Self::HuffYuv
            | Self::RawVideo
            | Self::Lagarith
            | Self::MagicYuv => "mkv",
            Self::Gif => "gif",
            Self::Apng => "apng",
            Self::WebpAnimated | Self::WebpStatic => "webp",
            Self::Mjpeg | Self::Jpeg => "jpg",
            Self::JpegXl => "jxl",
            Self::Png => "png",
            Self::Avif => "avif",
            Self::Heic => "heic",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Unknown => "bin",
        }
    }

    /// Checks if a file extension is compatible with this codec.
    #[must_use]
    pub fn is_extension_compatible(&self, ext: &str) -> bool {
        let ext = ext.to_lowercase();
        match self {
            Self::H264
            | Self::H265
            | Self::Vvc
            | Self::Vp8
            | Self::Vp9
            | Self::Av1
            | Self::Av2
            | Self::Mpeg4 => {
                matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "avi" | "mkv" | "webm")
            }
            Self::Mpeg2 | Self::Mpeg1 => {
                matches!(ext.as_str(), "mpg" | "mpeg" | "ts" | "mts" | "m2ts" | "m2v")
            }
            Self::Wmv => matches!(ext.as_str(), "wmv" | "asf"),
            Self::Jpeg | Self::Mjpeg => matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif"),
            Self::Png => ext == "png",
            Self::Gif => ext == "gif",
            Self::WebpStatic | Self::WebpAnimated => ext == "webp",
            Self::Heic => matches!(ext.as_str(), "heic" | "heif" | "hif"),
            Self::Avif => ext == "avif",
            Self::JpegXl => ext == "jxl",
            Self::Bmp => ext == "bmp",
            Self::Tiff => matches!(ext.as_str(), "tiff" | "tif"),
            Self::Apng => matches!(ext.as_str(), "apng" | "png"),
            _ => true, // Relaxed for other video formats
        }
    }

    /// Identifies the file format based on internal magic bytes.
    /// This is the "Tight Entry" mechanism that avoids relying on file extensions.
    #[must_use]
    pub fn identify_by_content(path: &std::path::Path) -> Option<Self> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path).ok()?;
        let mut header = [0u8; 64]; // Expanded to 64 bytes to capture VP8X and acTL chunks
        let n = file.read(&mut header).ok()?;
        if n < 2 {
            return None;
        }

        let Some(header_slice) = header.get(..n) else {
            warn!(
                "☢️ [ANOMALY] Failed to slice header for identification (n={})",
                n
            );
            return None;
        };

        let mut codec = Self::identify_by_header(header_slice);

        // Deep WebP animation verification
        // Some WebP files (notably Safari exports) may not place `VP8X` within the first 64 bytes
        // even when the file is animated, causing false `WebpStatic` classification.
        // We scan a bounded prefix for `ANIM`/`ANMF` markers as a fast, reliable fallback.
        if codec == Some(Self::WebpStatic)
            && header.starts_with(b"RIFF")
            && n >= 12
            && header.get(8..12) == Some(b"WEBP")
        {
            const SCAN_LIMIT: usize = 1024 * 1024; // 1 MiB cap (safe & fast)
            let mut buf = Vec::with_capacity(SCAN_LIMIT);
            buf.extend_from_slice(header_slice);

            let remaining = SCAN_LIMIT.saturating_sub(n);
            if remaining > 0 {
                let mut extra = vec![0u8; remaining];
                if let Ok(read_n) = file.read(&mut extra) {
                    let extra_slice = if let Some(s) = extra.get(..read_n) {
                        s
                    } else {
                        warn!(
                            "☢️ [ANOMALY] Failed to slice extra buffer for WebP scan (read_n={})",
                            read_n
                        );
                        &[]
                    };
                    buf.extend_from_slice(extra_slice);
                }
            }

            if buf.windows(4).any(|w| w == b"ANIM") || buf.windows(4).any(|w| w == b"ANMF") {
                codec = Some(Self::WebpAnimated);
            }
        }

        // Deep APNG verification
        // 64 bytes is insufficient for PNG because large chunks (like iCCP or eXIf)
        // can push the acTL chunk far beyond the header. We use Seek to jump over chunk data.
        if codec == Some(Self::Png) && file.seek(SeekFrom::Start(8)).is_ok() {
            let mut chunk_header = [0u8; 8];
            loop {
                if file.read_exact(&mut chunk_header).is_err() {
                    break;
                }
                let b1 = if let Some(b) = chunk_header.first() {
                    *b
                } else {
                    warn!(
                        "☢️ [CORRUPTION] APNG chunk header missing byte 0 at position {:?}",
                        file.stream_position()
                    );
                    break;
                };
                let b2 = if let Some(b) = chunk_header.get(1) {
                    *b
                } else {
                    warn!("☢️ [CORRUPTION] APNG chunk header missing byte 1");
                    break;
                };
                let b3 = if let Some(b) = chunk_header.get(2) {
                    *b
                } else {
                    warn!("☢️ [CORRUPTION] APNG chunk header missing byte 2");
                    break;
                };
                let b4 = if let Some(b) = chunk_header.get(3) {
                    *b
                } else {
                    warn!("☢️ [CORRUPTION] APNG chunk header missing byte 3");
                    break;
                };
                let length = u32::from_be_bytes([b1, b2, b3, b4]);
                let Some(chunk_type) = chunk_header.get(4..8) else {
                    warn!(
                        "☢️ [ANOMALY] Required APNG chunk type missing at position {:?}",
                        file.stream_position()
                    );
                    break;
                };

                if chunk_type == b"acTL" {
                    codec = Some(Self::Apng);
                    break;
                }
                if chunk_type == b"IDAT" {
                    break; // Image data reached; no animation chunk present
                }

                // Seek past the chunk data and its 4-byte CRC
                if file.seek(SeekFrom::Current(i64::from(length) + 4)).is_err() {
                    break;
                }
            }
        }

        codec
    }

    /// Identifies format from a byte slice (header).
    #[must_use]
    pub fn identify_by_header(header: &[u8]) -> Option<Self> {
        if header.len() < 2 {
            return None;
        }

        // 1. Static Image Patterns
        // JPEG: FF D8
        if header.starts_with(&[0xFF, 0xD8]) {
            return Some(Self::Jpeg);
        }
        // PNG: 89 50 4E 47 0D 0A 1A 0A
        if header.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
            // Check for APNG acTL chunk which usually follows immediately after IHDR (byte 33 starts the second chunk)
            if header.len() >= 41 && header.get(37..41) == Some(b"acTL") {
                return Some(Self::Apng);
            }
            return Some(Self::Png);
        }
        // GIF: GIF87a / GIF89a
        if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        // BMP: BM
        if header.starts_with(b"BM") {
            return Some(Self::Bmp);
        }
        // TIFF: II* (LE) or MM* (BE)
        if header.starts_with(&[0x49, 0x49, 0x2A, 0x00])
            || header.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
        {
            return Some(Self::Tiff);
        }
        // JPEG-XL: [FF 0A] or Container [00 00 00 0C 4A 58 4C 20 0D 0A 87 0A]
        if header.starts_with(&[0xFF, 0x0A])
            || header.starts_with(&[
                0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
            ])
        {
            return Some(Self::JpegXl);
        }

        // 2. RIFF Containers (WebP, AVI)
        if header.starts_with(b"RIFF") && header.len() >= 12 {
            let Some(brand) = header.get(8..12) else {
                warn!("☢️ [ANOMALY] RIFF container missing brand field");
                return None;
            };
            if brand == b"WEBP" {
                // Check for VP8X extended header which contains the animation flag
                if header.len() >= 21 && header.get(12..16) == Some(b"VP8X") {
                    // The animation flag is the 2nd bit of the flags byte at offset 20
                    let flags = header.get(20).map_or_else(
                        || {
                            // Keep map_or_else here for clarity on flag retrieval
                            warn!("☢️ [ANOMALY] WebP VP8X header missing flags byte");
                            0
                        },
                        |b| *b,
                    );
                    if (flags & 0x02) != 0 {
                        return Some(Self::WebpAnimated);
                    }
                }
                return Some(Self::WebpStatic);
            }
            if brand == b"AVI " {
                return Some(Self::Mpeg4); // AVI often contains MPEG4 variants
            }
        }

        // 3. ISO Base Media File Format (MP4, MOV, HEIC, AVIF)
        // [Any 4 bytes] + "ftyp"
        if header.len() >= 12 && header.get(4..8) == Some(b"ftyp") {
            let Some(brand) = header.get(8..12) else {
                warn!("☢️ [ANOMALY] Required brand field missing in ISO Base Media header. Information invalidated.");
                return None;
            };
            match brand {
                b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1" => {
                    return Some(Self::Heic)
                }
                b"avif" | b"avis" => return Some(Self::Avif),
                b"isom" | b"mp41" | b"mp42" | b"piso" | b"mp4v" | b"3gp4" | b"3gp5" | b"3g2a" => {
                    return Some(Self::H264)
                }
                _ => {
                    // Refusing to assume H264 for unknown MP4/MOV brands.
                    // Information invalidated to prevent false quality matching.
                    warn!("☢️ [ANOMALY] Unknown ISO Base Media brand '{}'. Refusing to forge codec information.", String::from_utf8_lossy(brand));
                    return None;
                }
            }
        }

        // 4. MKV / WebM (EBML)
        if header.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            return Some(Self::Av1); // MKV/WebM is a catch-all for modern video identification here
        }

        // 5. MPEG Transport/Program Stream
        if header.starts_with(&[0x47]) {
            // Make sure this is not a truncated GIF (which starts with 'G' = 0x47)
            if header.len() >= 3 && &header[0..3] != b"GIF" {
                return Some(Self::Mpeg2); // TS sync byte
            }
        }
        if header.starts_with(&[0x00, 0x00, 0x01, 0xBA]) {
            return Some(Self::Mpeg2); // PS start code
        }

        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MatchMode {
    #[default]
    Quality,
    Size,
    Speed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QualityBias {
    Conservative,
    #[default]
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityAnalysis {
    pub bpp: f64,
    pub source_codec: String,
    pub width: u32,
    pub height: u32,
    pub has_b_frames: bool,
    pub bit_depth: u8,
    pub has_alpha: bool,
    pub duration_secs: Option<f64>,
    pub fps: Option<f64>,
    pub file_size: u64,
    pub estimated_quality: Option<u8>,

    pub video_bitrate: Option<u64>,

    pub gop_size: Option<u32>,

    pub b_frame_count: Option<u8>,

    pub pix_fmt: Option<String>,

    pub color_space: Option<String>,

    pub is_hdr: Option<bool>,

    pub content_type: Option<ContentType>,

    pub spatial_complexity: Option<f64>,

    pub temporal_complexity: Option<f64>,

    pub has_film_grain: Option<bool>,

    pub encoder_preset: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ContentType {
    LiveAction,
    Animation,
    ScreenRecording,
    Gaming,
    FilmGrain,
    #[default]
    Unknown,
}

impl ContentType {
    #[must_use]
    pub const fn crf_adjustment(&self) -> i8 {
        match self {
            Self::Animation => 4,
            Self::ScreenRecording => 5,
            Self::LiveAction | Self::Unknown => 0,
            Self::Gaming => -1,
            Self::FilmGrain => -3,
        }
    }
}

impl Default for QualityAnalysis {
    fn default() -> Self {
        Self {
            bpp: 0.0,
            source_codec: String::new(),
            width: 0,
            height: 0,
            has_b_frames: false,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: None,
            fps: None,
            file_size: 0,
            estimated_quality: None,
            video_bitrate: None,
            gop_size: None,
            b_frame_count: None,
            pix_fmt: None,
            color_space: None,
            is_hdr: None,
            content_type: None,
            spatial_complexity: None,
            temporal_complexity: None,
            has_film_grain: None,
            encoder_preset: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedQuality {
    pub crf: f32,
    pub distance: f32,
    pub effective_bpp: f64,
    pub analysis_details: AnalysisDetails,
}

impl MatchedQuality {
    #[inline]
    #[must_use]
    pub fn crf_hevc_typed(&self) -> Option<crate::types::Crf<crate::types::HevcEncoder>> {
        crate::types::Crf::<crate::types::HevcEncoder>::new(self.crf).ok()
    }

    #[inline]
    #[must_use]
    pub fn crf_av1_typed(&self) -> Option<crate::types::Crf<crate::types::Av1Encoder>> {
        crate::types::Crf::<crate::types::Av1Encoder>::new(self.crf).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisDetails {
    pub raw_bpp: f64,
    pub codec_factor: f64,
    pub resolution_factor: f64,
    pub alpha_factor: f64,
    pub color_depth_factor: f64,

    pub gop_factor: f64,
    pub chroma_factor: f64,
    pub hdr_factor: f64,
    pub content_type_adjustment: i8,

    pub aspect_factor: f64,
    pub complexity_factor: f64,
    pub grain_factor: f64,

    #[serde(default = "default_one")]
    pub bframe_factor: f64,
    #[serde(default = "default_one")]
    pub fps_factor: f64,
    #[serde(default = "default_one")]
    pub duration_factor: f64,

    pub confidence: f64,
    pub match_mode: MatchMode,
    pub quality_bias: QualityBias,
}

const fn default_one() -> f64 {
    1.0
}

impl Default for AnalysisDetails {
    fn default() -> Self {
        Self {
            raw_bpp: 0.0,
            codec_factor: 1.0,
            resolution_factor: 1.0,
            alpha_factor: 1.0,
            color_depth_factor: 1.0,
            gop_factor: 1.0,
            chroma_factor: 1.0,
            hdr_factor: 1.0,
            content_type_adjustment: 0,
            aspect_factor: 1.0,
            complexity_factor: 1.0,
            grain_factor: 1.0,
            bframe_factor: 1.0,
            fps_factor: 1.0,
            duration_factor: 1.0,
            confidence: 0.0,
            match_mode: MatchMode::Quality,
            quality_bias: QualityBias::Balanced,
        }
    }
}

/// Safe BPP range for CRF formula: avoids log2(0), NaN, and overflow. Final CRF is still clamped to [0, 51] for maximum flexibility.
const SAFE_BPP_MIN: f64 = 1e-6;
const SAFE_BPP_MAX: f64 = 50.0;

/// AV1 CRF output range; final clamp is the last line of defense for extreme BPP or content/bias adjustments.
const AV1_CRF_CLAMP_MIN: f32 = 0.0;
const AV1_CRF_CLAMP_MAX: f32 = 51.0;

/// HEVC CRF output range (x265 0–51, we use 0–51 to allow full range in ultimate mode).
const HEVC_CRF_CLAMP_MIN: f32 = 0.0;
const HEVC_CRF_CLAMP_MAX: f32 = 51.0;

/// Calculate AV1 CRF.
///
/// # Errors
/// Returns an error message if calculation fails.
pub fn calculate_av1_crf(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    calculate_av1_crf_with_options(analysis, MatchMode::Quality, QualityBias::Balanced)
}

/// Calculate AV1 CRF with options.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_av1_crf_with_options(
    analysis: &QualityAnalysis,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<MatchedQuality, String> {
    let (mut effective_bpp, details) =
        calculate_effective_bpp_with_options(analysis, EncoderType::Av1, mode, bias)?;

    if effective_bpp <= 0.0_f64 {
        return Err(format!(
            "❌ Cannot calculate AV1 CRF: effective_bpp is {} (must be > 0)\n\
             💡 Possible causes:\n\
             - File size is 0 or unknown\n\
             - video_bitrate not provided\n\
             - Duration/fps detection failed\n\
             - Invalid dimensions\n\
             💡 Confidence: {:.0}%",
            effective_bpp,
            details.confidence * 100.0
        ));
    }
    if !effective_bpp.is_finite() {
        return Err(format!(
            "❌ Cannot calculate AV1 CRF: effective_bpp is non-finite (NaN/Inf)\n\
             💡 Confidence: {:.0}%",
            details.confidence * 100.0
        ));
    }
    // Defensive clamp so formula inputs are always in a safe range; final CRF clamp [15, 40] remains the safeguard.
    effective_bpp = effective_bpp.clamp(SAFE_BPP_MIN, SAFE_BPP_MAX);

    let crf_float = if effective_bpp < 0.03_f64 {
        35.0_f64.min(6.0f64.mul_add(-(effective_bpp * 100.0).max(0.001).log2(), 50.0))
    } else if effective_bpp > 2.0_f64 {
        18.0_f64.max(6.0f64.mul_add(-(effective_bpp * 100.0).log2(), 50.0))
    } else {
        6.0f64.mul_add(-(effective_bpp * 100.0).log2(), 50.0)
    };

    let crf_with_content = crf_float + f64::from(details.content_type_adjustment);

    let crf_with_bias = match bias {
        QualityBias::Conservative => crf_with_content - 2.0_f64,
        QualityBias::Balanced => crf_with_content,
        QualityBias::Aggressive => crf_with_content + 2.0_f64,
    };

    let crf_rounded = (crf_with_bias * 2.0).round() / 2.0_f64;
    // Last line of defense: guarantee CRF in valid range regardless of extreme BPP or content/bias.
    let crf = (crate::numeric_cast::f64_to_f32_lossy(crf_rounded))
        .clamp(AV1_CRF_CLAMP_MIN, AV1_CRF_CLAMP_MAX);

    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    })
}

/// Calculate HEVC CRF.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_hevc_crf(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    calculate_hevc_crf_with_options(analysis, MatchMode::Quality, QualityBias::Balanced)
}

/// Calculate HEVC CRF with options.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_hevc_crf_with_options(
    analysis: &QualityAnalysis,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<MatchedQuality, String> {
    let (mut effective_bpp, details) =
        calculate_effective_bpp_with_options(analysis, EncoderType::Hevc, mode, bias)?;

    if effective_bpp <= 0.0_f64 {
        return Err(format!(
            "❌ Cannot calculate HEVC CRF: effective_bpp is {} (must be > 0)\n\
             💡 Possible causes:\n\
             - File size is 0 or unknown\n\
             - video_bitrate not provided\n\
             - Duration/fps detection failed\n\
             - Invalid dimensions\n\
             💡 Confidence: {:.0}%",
            effective_bpp,
            details.confidence * 100.0
        ));
    }
    if !effective_bpp.is_finite() {
        return Err(format!(
            "❌ Cannot calculate HEVC CRF: effective_bpp is non-finite (NaN/Inf)\n\
             💡 Confidence: {:.0}%",
            details.confidence * 100.0
        ));
    }
    effective_bpp = effective_bpp.clamp(SAFE_BPP_MIN, SAFE_BPP_MAX);

    let crf_float = if effective_bpp < 0.02_f64 {
        35.0_f64.min(5.0f64.mul_add(-(effective_bpp * 100.0).max(0.001).log2(), 46.0))
    } else if effective_bpp > 2.0_f64 {
        15.0_f64.max(5.0f64.mul_add(-(effective_bpp * 100.0).log2(), 46.0))
    } else {
        5.0f64.mul_add(-(effective_bpp * 100.0).log2(), 46.0)
    };

    let crf_with_content = crf_float + f64::from(details.content_type_adjustment);

    let crf_with_bias = match bias {
        QualityBias::Conservative => crf_with_content - 2.0_f64,
        QualityBias::Balanced => crf_with_content,
        QualityBias::Aggressive => crf_with_content + 2.0_f64,
    };

    let crf_rounded = (crf_with_bias * 2.0).round() / 2.0_f64;
    let crf = (crate::numeric_cast::f64_to_f32_lossy(crf_rounded))
        .clamp(HEVC_CRF_CLAMP_MIN, HEVC_CRF_CLAMP_MAX);

    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    })
}

/// Calculate JXL distance.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_jxl_distance(analysis: &QualityAnalysis) -> Result<MatchedQuality, String> {
    calculate_jxl_distance_with_options(analysis, MatchMode::Quality, QualityBias::Balanced)
}

/// Calculate JXL distance with options.
///
/// # Errors
/// Returns an error if calculation fails.
pub fn calculate_jxl_distance_with_options(
    analysis: &QualityAnalysis,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<MatchedQuality, String> {
    if let Some(quality) = analysis.estimated_quality {
        let base_distance = (100.0 - f32::from(quality)) / 10.0;

        let biased_distance = match bias {
            QualityBias::Conservative => base_distance - 0.2,
            QualityBias::Balanced => base_distance,
            QualityBias::Aggressive => base_distance + 0.3,
        };

        let clamped = biased_distance.clamp(0.0, 5.0);

        return Ok(MatchedQuality {
            crf: 0.0,
            distance: clamped,
            effective_bpp: analysis.bpp,
            analysis_details: AnalysisDetails {
                confidence: calculate_confidence_v3(analysis),
                match_mode: mode,
                quality_bias: bias,
                ..Default::default()
            },
        });
    }

    let (effective_bpp, details) =
        calculate_effective_bpp_with_options(analysis, EncoderType::Jxl, mode, bias)?;

    if effective_bpp <= 0.0_f64 {
        return Err(format!(
            "❌ Cannot calculate JXL distance: effective_bpp is {} (must be > 0)\n\
             💡 Possible causes:\n\
             - File size is 0 or unknown\n\
             - Invalid dimensions\n\
             💡 For JPEG sources, ensure JPEG quality analysis is available\n\
             💡 Confidence: {:.0}%",
            effective_bpp,
            details.confidence * 100.0
        ));
    }

    let estimated_quality = 15.0f64.mul_add((effective_bpp * 5.0).max(0.001).log2(), 70.0);

    let clamped_quality = estimated_quality.clamp(50.0, 100.0);
    let base_distance = crate::numeric_cast::f64_to_f32_lossy((100.0 - clamped_quality) / 10.0);

    let content_adj = f32::from(details.content_type_adjustment) * 0.1;
    let distance_with_content = base_distance - content_adj;

    let distance_with_bias = match bias {
        QualityBias::Conservative => distance_with_content - 0.2,
        QualityBias::Balanced => distance_with_content,
        QualityBias::Aggressive => distance_with_content + 0.3,
    };

    let clamped_distance = distance_with_bias.clamp(0.0, 5.0);

    Ok(MatchedQuality {
        crf: 0.0,
        distance: clamped_distance,
        effective_bpp,
        analysis_details: details,
    })
}

/// Calculate effective bits per pixel with advanced options.
///
/// # Errors
/// Returns an error message if calculation fails (e.g., missing or invalid dimensions).
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn calculate_effective_bpp_with_options(
    analysis: &QualityAnalysis,
    target_encoder: EncoderType,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<(f64, AnalysisDetails), String> {
    if analysis.width == 0 || analysis.height == 0 {
        return Err("❌ Invalid dimensions: width or height is 0".to_string());
    }

    let pixels = u64::from(analysis.width) * u64::from(analysis.height);

    let raw_bpp = calculate_raw_bpp(analysis, pixels)?;

    let source_codec = parse_source_codec(&analysis.source_codec);
    let codec_factor = calculate_codec_efficiency(source_codec, analysis.encoder_preset.as_deref());

    let gop_factor = calculate_gop_factor(
        analysis.gop_size,
        analysis
            .b_frame_count
            .unwrap_or(if analysis.has_b_frames { 2 } else { 0 }),
    );

    let chroma_factor = calculate_chroma_factor(analysis.pix_fmt.as_deref());

    let hdr_factor = calculate_hdr_factor(analysis.is_hdr, analysis.color_space.as_deref());

    let content_type_adjustment = analysis
        .content_type
        .unwrap_or(ContentType::Unknown)
        .crf_adjustment();

    let resolution_factor = calculate_resolution_factor(pixels);

    let alpha_factor = if analysis.has_alpha { 0.9_f64 } else { 1.0_f64 };

    let color_depth_factor = calculate_color_depth_factor(analysis.bit_depth, source_codec);

    let aspect_factor = calculate_aspect_factor(analysis.width, analysis.height);

    let complexity_factor = calculate_complexity_factor(
        analysis.spatial_complexity,
        analysis.temporal_complexity,
        raw_bpp,
        pixels,
    );

    let grain_factor = if analysis.has_film_grain == Some(true) {
        1.20_f64
    } else {
        1.0_f64
    };

    let target_adjustment = match target_encoder {
        EncoderType::Av1 => 0.5_f64,
        EncoderType::Hevc => 0.7_f64,
        EncoderType::Jxl => 0.8_f64,
    };

    let mode_adjustment = match mode {
        MatchMode::Quality => 1.0_f64,
        MatchMode::Size => 0.8_f64,
        MatchMode::Speed => 0.9_f64,
    };

    let effective_bpp = {
        #[cfg(feature = "high-precision")]
        {
            use crate::numeric_cast::f64_to_rational_strict;
            let mut res = f64_to_rational_strict(raw_bpp, "raw_bpp").ok_or("Invalid raw_bpp")?;
            res *= f64_to_rational_strict(gop_factor, "gop_factor")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(chroma_factor, "chroma_factor")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(hdr_factor, "hdr_factor")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(aspect_factor, "aspect_factor")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(complexity_factor, "complexity_factor")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(grain_factor, "grain_factor")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(mode_adjustment, "mode_adjustment")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(resolution_factor, "resolution_factor")
                .unwrap_or_else(|| Rational::from(1));
            res *= f64_to_rational_strict(alpha_factor, "alpha_factor")
                .unwrap_or_else(|| Rational::from(1));
            res /= f64_to_rational_strict(codec_factor, "codec_factor")
                .unwrap_or_else(|| Rational::from(1));
            res /= f64_to_rational_strict(color_depth_factor, "color_depth_factor")
                .unwrap_or_else(|| Rational::from(1));
            res /= f64_to_rational_strict(target_adjustment, "target_adjustment")
                .unwrap_or_else(|| Rational::from(1));
            res.to_f64()
        }
        #[cfg(not(feature = "high-precision"))]
        {
            let mut res = raw_bpp;
            res *= gop_factor;
            res *= chroma_factor;
            res *= hdr_factor;
            res *= aspect_factor;
            res *= complexity_factor;
            res *= grain_factor;
            res *= mode_adjustment;
            res *= resolution_factor;
            res *= alpha_factor;
            res /= codec_factor;
            res /= color_depth_factor;
            res /= target_adjustment;
            res
        }
    };

    let confidence = calculate_confidence_v3(analysis);

    let details = AnalysisDetails {
        raw_bpp,
        codec_factor,
        resolution_factor,
        alpha_factor,
        color_depth_factor,
        gop_factor,
        chroma_factor,
        hdr_factor,
        content_type_adjustment,
        aspect_factor,
        complexity_factor,
        grain_factor,
        bframe_factor: gop_factor,
        fps_factor: 1.0,
        duration_factor: 1.0,
        confidence,
        match_mode: mode,
        quality_bias: bias,
    };

    Ok((effective_bpp, details))
}

fn calculate_raw_bpp(analysis: &QualityAnalysis, pixels: u64) -> Result<f64, String> {
    if analysis.bpp > 0.0_f64 {
        return Ok(analysis.bpp);
    }

    // Explicit zero check to prevent division by zero
    if pixels == 0 {
        return Err("❌ Cannot calculate bpp: pixels is 0 (invalid dimensions)".to_string());
    }

    if let Some(video_bitrate) = analysis.video_bitrate {
        if video_bitrate > 0 {
            if let Some(fps) = analysis.fps {
                if fps > 0.0_f64 {
                    #[cfg(feature = "high-precision")]
                    {
                        let bits_per_frame = Rational::from(video_bitrate)
                            / crate::numeric_cast::f64_to_rational_strict(fps, "fps")
                                .ok_or_else(|| "Invalid FPS for rational conversion".to_string())?;
                        return Ok((bits_per_frame / Rational::from(pixels)).to_f64());
                    }
                    #[cfg(not(feature = "high-precision"))]
                    {
                        let bits_per_frame = crate::numeric_cast::u64_to_f64(video_bitrate) / fps;
                        return Ok(bits_per_frame / crate::numeric_cast::u64_to_f64(pixels));
                    }
                }
            }
        }
    }

    if analysis.file_size > 0 {
        if let Some(duration) = analysis.duration_secs {
            if duration > 0.0_f64 {
                let fps = analysis
                    .fps
                    .ok_or_else(|| "Missing FPS for BPP calculation".to_string())?;
                if fps <= 0.0_f64 {
                    return Err("❌ Cannot calculate bpp: FPS is 0 or negative".to_string());
                }
                let total_frames =
                    crate::numeric_cast::f64_to_u64_strict(duration * fps, "total_frames")
                        .ok_or_else(|| {
                            "❌ Cannot calculate bpp: total_frames is invalid".to_string()
                        })?;
                if total_frames == 0 {
                    return Err("❌ Cannot calculate bpp: total_frames is 0".to_string());
                }
                #[cfg(feature = "high-precision")]
                {
                    let bits_per_frame = (Rational::from(analysis.file_size)
                        * Rational::from(8_i32))
                        / Rational::from(total_frames);
                    return Ok((bits_per_frame / Rational::from(pixels)).to_f64());
                }
                #[cfg(not(feature = "high-precision"))]
                {
                    let bits_per_frame = (crate::numeric_cast::u64_to_f64(analysis.file_size)
                        * 8.0)
                        / crate::numeric_cast::u64_to_f64(total_frames);
                    return Ok(bits_per_frame / crate::numeric_cast::u64_to_f64(pixels));
                }
            }
        }
        // BPP = bits per pixel; file_size is in bytes so multiply by 8
        #[cfg(feature = "high-precision")]
        {
            return Ok(((Rational::from(analysis.file_size) * Rational::from(8))
                / Rational::from(pixels))
            .to_f64());
        }
        #[cfg(not(feature = "high-precision"))]
        {
            return Ok((crate::numeric_cast::u64_to_f64(analysis.file_size) * 8.0)
                / crate::numeric_cast::u64_to_f64(pixels));
        }
    }

    Err("❌ Cannot calculate bpp: no video_bitrate, file_size, or bpp provided".to_string())
}

fn calculate_gop_factor(gop_size: Option<u32>, b_frames: u8) -> f64 {
    let gop_base = match gop_size {
        Some(1) => 0.70_f64,
        Some(2..=10) => 0.85_f64,
        Some(11..=50) | None => 1.0_f64,
        Some(51..=150) => 1.15_f64,
        Some(151..=300) => 1.20_f64,
        Some(_) => 1.25_f64,
    };

    let b_pyramid_bonus = match b_frames {
        0 => 1.0_f64,
        1 => 1.05_f64,
        2 => 1.08_f64,
        _ => 1.12_f64,
    };

    gop_base * b_pyramid_bonus
}

fn calculate_chroma_factor(pix_fmt: Option<&str>) -> f64 {
    pix_fmt.map_or(1.0, |fmt| {
        let fmt_lower = fmt.to_lowercase();
        if fmt_lower.contains("444") {
            1.15_f64
        } else if fmt_lower.contains("422") {
            1.05_f64
        } else if fmt_lower.contains("rgb") || fmt_lower.contains("gbr") {
            1.20_f64
        } else {
            1.0_f64
        }
    })
}

fn calculate_hdr_factor(is_hdr: Option<bool>, color_space: Option<&str>) -> f64 {
    if is_hdr == Some(true) {
        return 1.25;
    }

    if let Some(cs) = color_space {
        let cs_lower = cs.to_lowercase();
        if cs_lower.contains("bt2020") || cs_lower.contains("2020") {
            return 1.15;
        }
    }

    1.0
}

fn calculate_codec_efficiency(codec: SourceCodec, preset: Option<&str>) -> f64 {
    let base_efficiency = codec.efficiency_factor();

    if let Some(p) = preset {
        let p_lower = p.to_lowercase();

        if p_lower.contains("placebo") || p_lower.contains("veryslow") {
            return base_efficiency * 0.85;
        } else if p_lower.contains("slow") {
            return base_efficiency * 0.90;
        } else if p_lower.contains("fast") || p_lower.contains("veryfast") {
            return base_efficiency * 1.15;
        } else if p_lower.contains("ultrafast") {
            return base_efficiency * 1.30;
        }

        if let Ok(preset_num) = p.parse::<u8>() {
            return match preset_num {
                0..=2 => base_efficiency * 0.80,
                3..=4 => base_efficiency * 0.90,
                5..=6 => base_efficiency * 1.0,
                7..=8 => base_efficiency * 1.10,
                9..=10 => base_efficiency * 1.20,
                _ => base_efficiency * 1.30,
            };
        }
    }

    base_efficiency
}

fn calculate_resolution_factor(pixels: u64) -> f64 {
    let megapixels = crate::numeric_cast::u64_to_f64(pixels) / 1_000_000.0_f64;
    if megapixels > 8.0 {
        0.05f64.mul_add((8.0 / megapixels).min(1.0), 0.80)
    } else if megapixels > 2.0 {
        0.05f64.mul_add((8.0 - megapixels) / 6.0, 0.85)
    } else if megapixels > 0.5 {
        0.05f64.mul_add((2.0 - megapixels) / 1.5, 0.90)
    } else {
        0.05f64.mul_add(((0.5 - megapixels) / 0.5).min(1.0), 0.95)
    }
}

fn calculate_color_depth_factor(bit_depth: u8, codec: SourceCodec) -> f64 {
    match bit_depth {
        1..=8 if codec == SourceCodec::Gif => 1.3,
        10 => 1.25,
        12 => 1.5,
        16 => 2.0,
        _ => 1.0,
    }
}

fn calculate_aspect_factor(width: u32, height: u32) -> f64 {
    let aspect_ratio = f64::from(width) / f64::from(height.max(1));
    if aspect_ratio > 2.5 {
        1.08
    } else if aspect_ratio > 2.0 {
        1.04
    } else if aspect_ratio < 0.5 {
        1.08
    } else {
        1.0
    }
}

fn calculate_complexity_factor(si: Option<f64>, ti: Option<f64>, raw_bpp: f64, pixels: u64) -> f64 {
    if let (Some(si_val), Some(temporal)) = (si, ti) {
        let si_ratio = si_val / 50.0_f64;
        let ti_ratio = temporal / 20.0_f64;

        let spatial_factor = if si_ratio > 1.3_f64 {
            1.15_f64
        } else if si_ratio < 0.7_f64 {
            0.85_f64
        } else {
            1.0_f64
        };

        let temporal_factor = if ti_ratio > 1.5_f64 {
            1.10_f64
        } else if ti_ratio < 0.5_f64 {
            0.90_f64
        } else {
            1.0_f64
        };

        return spatial_factor * temporal_factor;
    }

    let expected_bpp = if pixels > 8_000_000 {
        0.15_f64
    } else if pixels > 2_000_000 {
        0.20_f64
    } else if pixels > 500_000 {
        0.30_f64
    } else {
        0.50_f64
    };

    let ratio = raw_bpp / expected_bpp;
    if ratio > 2.0 {
        1.15
    } else if ratio > 1.0 {
        0.15f64.mul_add((ratio - 1.0) / 1.0, 1.0)
    } else if ratio > 0.5 {
        1.0
    } else {
        0.95
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn calculate_confidence_v3(analysis: &QualityAnalysis) -> f64 {
    let mut score: f64 = 0.0;
    let mut max_score: f64 = 0.0;

    max_score += 25.0_f64;
    if analysis.width > 0 && analysis.height > 0 {
        score += 25.0_f64;
    }

    max_score += 20.0_f64;
    if analysis.file_size > 0 || analysis.video_bitrate.is_some() {
        score += 20.0_f64;
    }

    max_score += 10.0_f64;
    if analysis.bpp > 0.0_f64 {
        score += 10.0_f64;
    }

    max_score += 8.0_f64;
    let codec = parse_source_codec(&analysis.source_codec);
    if codec != SourceCodec::Unknown {
        score += 8.0_f64;
    }

    max_score += 5.0_f64;
    if analysis.video_bitrate.is_some() {
        score += 5.0_f64;
    }

    max_score += 4.0_f64;
    if analysis.gop_size.is_some() {
        score += 4.0_f64;
    }

    max_score += 3.0_f64;
    if analysis.b_frame_count.is_some() {
        score += 3.0_f64;
    }

    max_score += 3.0_f64;
    if analysis.pix_fmt.is_some() {
        score += 3.0_f64;
    }

    max_score += 3.0_f64;
    if analysis.is_hdr.is_some() || analysis.color_space.is_some() {
        score += 3.0_f64;
    }

    max_score += 2.0_f64;
    if analysis.content_type.is_some() {
        score += 2.0_f64;
    }

    max_score += 3.0_f64;
    if analysis.spatial_complexity.is_some() && analysis.temporal_complexity.is_some() {
        score += 3.0_f64;
    }

    max_score += 4.0_f64;
    if analysis.duration_secs.is_some() {
        score += 4.0_f64;
    }

    max_score += 4.0_f64;
    if analysis.fps.is_some() {
        score += 4.0_f64;
    }

    max_score += 3.0_f64;
    if analysis.estimated_quality.is_some() {
        score += 3.0_f64;
    }

    max_score += 3.0_f64;
    if analysis.bit_depth > 0 {
        score += 3.0_f64;
    }

    if let (Some(fps), Some(duration)) = (analysis.fps, analysis.duration_secs) {
        if fps > 0.0_f64 && duration > 0.0_f64 && (1.0_f64..=240.0_f64).contains(&fps) {
            score += 2.0_f64;
            max_score += 2.0_f64;
        }
    }

    if let (Some(video_bitrate), Some(fps)) = (analysis.video_bitrate, analysis.fps) {
        let pixels = u64::from(analysis.width) * u64::from(analysis.height);
        if pixels > 0 && video_bitrate > 0 && fps > 0.0_f64 {
            // Use u64 throughout to prevent saturation at 4 Gbps (u32::MAX = ~4.3 Gbps)
            let bpp_estimate = crate::numeric_cast::u64_to_f64(video_bitrate)
                / (crate::numeric_cast::u64_to_f64(pixels) * fps);
            if (0.01_f64..=5.0_f64).contains(&bpp_estimate) {
                score += 2.0_f64;
                max_score += 2.0_f64;
            }
        }
    }

    (score / max_score).clamp(0.0, 1.0)
}

fn parse_modern_codecs(codec_lower: &str) -> Option<SourceCodec> {
    if codec_lower.contains("vvc") || codec_lower.contains("h266") || codec_lower.contains("h.266")
    {
        return Some(SourceCodec::Vvc);
    }
    if codec_lower.contains("av2") || codec_lower.contains("avm") {
        return Some(SourceCodec::Av2);
    }
    if codec_lower.contains("av1")
        || codec_lower.contains("svt")
        || codec_lower.contains("aom")
        || codec_lower.contains("libaom")
    {
        return Some(SourceCodec::Av1);
    }
    if codec_lower.contains("h265")
        || codec_lower.contains("hevc")
        || codec_lower.contains("x265")
        || codec_lower.contains("h.265")
    {
        return Some(SourceCodec::H265);
    }
    if codec_lower.contains("vp9") {
        return Some(SourceCodec::Vp9);
    }
    if codec_lower.contains("vp8") || codec_lower == "libvpx" {
        return Some(SourceCodec::Vp8);
    }
    if codec_lower.contains("h264")
        || codec_lower.contains("avc")
        || codec_lower.contains("x264")
        || codec_lower.contains("h.264")
    {
        return Some(SourceCodec::H264);
    }
    None
}

fn parse_legacy_codecs(codec_lower: &str) -> Option<SourceCodec> {
    if codec_lower.contains("mpeg4")
        || codec_lower.contains("xvid")
        || codec_lower.contains("divx")
        || codec_lower.contains("mp4v")
    {
        return Some(SourceCodec::Mpeg4);
    }
    if codec_lower.contains("mpeg2") || codec_lower == "mpeg2video" {
        return Some(SourceCodec::Mpeg2);
    }
    if codec_lower.contains("mpeg1") || codec_lower == "mpeg1video" {
        return Some(SourceCodec::Mpeg1);
    }
    if codec_lower.contains("wmv") || codec_lower.contains("vc1") || codec_lower.contains("vc-1") {
        return Some(SourceCodec::Wmv);
    }
    if codec_lower.contains("theora") {
        return Some(SourceCodec::Theora);
    }
    if codec_lower.contains("rv10")
        || codec_lower.contains("rv20")
        || codec_lower.contains("rv30")
        || codec_lower.contains("rv40")
        || codec_lower.contains("realvideo")
    {
        return Some(SourceCodec::RealVideo);
    }
    if codec_lower.contains("flv") || codec_lower.contains("vp6") || codec_lower.contains("flashsv")
    {
        return Some(SourceCodec::FlashVideo);
    }
    None
}

fn parse_pro_and_lossless_codecs(codec_lower: &str) -> Option<SourceCodec> {
    if codec_lower.contains("prores") {
        return Some(SourceCodec::ProRes);
    }
    if codec_lower.contains("dnxh") || codec_lower.contains("dnxhr") {
        return Some(SourceCodec::DnxHD);
    }
    if codec_lower.contains("mjpeg") || codec_lower.contains("motion jpeg") {
        return Some(SourceCodec::Mjpeg);
    }
    if codec_lower.contains("ffv1") {
        return Some(SourceCodec::Ffv1);
    }
    if codec_lower.contains("utvideo") || codec_lower.contains("ut video") {
        return Some(SourceCodec::UtVideo);
    }
    if codec_lower.contains("huffyuv") || codec_lower.contains("ffvhuff") {
        return Some(SourceCodec::HuffYuv);
    }
    if codec_lower.contains("rawvideo") || codec_lower == "raw" {
        return Some(SourceCodec::RawVideo);
    }
    if codec_lower.contains("lagarith") {
        return Some(SourceCodec::Lagarith);
    }
    if codec_lower.contains("magicyuv") {
        return Some(SourceCodec::MagicYuv);
    }
    None
}

fn parse_image_and_animated_codecs(codec_lower: &str) -> Option<SourceCodec> {
    if codec_lower.contains("gif") {
        return Some(SourceCodec::Gif);
    }
    if codec_lower.contains("apng") {
        return Some(SourceCodec::Apng);
    }
    if codec_lower.contains("webp") {
        if codec_lower.contains("anim") {
            return Some(SourceCodec::WebpAnimated);
        }
        return Some(SourceCodec::WebpStatic);
    }
    if codec_lower.contains("jxl")
        || codec_lower.contains("jpeg xl")
        || codec_lower.contains("jpegxl")
        || codec_lower.contains("jpeg-xl")
    {
        return Some(SourceCodec::JpegXl);
    }
    if codec_lower.contains("avif") {
        return Some(SourceCodec::Avif);
    }
    if codec_lower.contains("heic") || codec_lower.contains("heif") {
        return Some(SourceCodec::Heic);
    }
    if codec_lower.contains("jpeg") || codec_lower.contains("jpg") {
        return Some(SourceCodec::Jpeg);
    }
    if codec_lower.contains("png") {
        return Some(SourceCodec::Png);
    }
    if codec_lower.contains("bmp") || codec_lower.contains("bitmap") {
        return Some(SourceCodec::Bmp);
    }
    if codec_lower.contains("tiff") || codec_lower.contains("tif") {
        return Some(SourceCodec::Tiff);
    }
    None
}

#[must_use]
pub fn parse_source_codec(codec_str: &str) -> SourceCodec {
    let codec_lower = codec_str.to_lowercase();

    if let Some(codec) = parse_modern_codecs(&codec_lower) {
        return codec;
    }
    if let Some(codec) = parse_legacy_codecs(&codec_lower) {
        return codec;
    }
    if let Some(codec) = parse_pro_and_lossless_codecs(&codec_lower) {
        return codec;
    }
    if let Some(codec) = parse_image_and_animated_codecs(&codec_lower) {
        return codec;
    }

    SourceCodec::Unknown
}

fn log_analysis_header(encoder_name: &str, d: &AnalysisDetails) {
    eprintln!("   Quality Analysis v3.0 ({encoder_name}):");
    eprintln!(
        "      Mode: {:?} | Bias: {:?}",
        d.match_mode, d.quality_bias
    );
    eprintln!("      Confidence: {:.0}%", d.confidence * 100.0_f64);
    eprintln!();
}

fn log_source_info(analysis: &QualityAnalysis, codec: SourceCodec, d: &AnalysisDetails) {
    eprintln!("      Source:");
    eprintln!(
        "         Codec: {} ({:?}, efficiency: {:.2})",
        analysis.source_codec, codec, d.codec_factor
    );
    if codec.is_cutting_edge() {
        eprintln!("         CUTTING-EDGE codec (VVC/AV2) - SKIP RECOMMENDED");
    } else if codec.is_modern() {
        eprintln!("         ⚠️  Modern codec - consider skipping re-encode");
    }
    eprintln!(
        "         Resolution: {}x{} (factor: {:.2})",
        analysis.width, analysis.height, d.resolution_factor
    );
    eprintln!(
        "         Bit depth: {}-bit (factor: {:.2})",
        analysis.bit_depth, d.color_depth_factor
    );
    eprintln!();
}

fn log_high_priority_factors(analysis: &QualityAnalysis, d: &AnalysisDetails) {
    eprintln!("      High Priority Factors:");
    eprintln!("         Raw BPP: {:.4}", d.raw_bpp);
    if let Some(vbr) = analysis.video_bitrate {
        eprintln!(
            "         Video bitrate: {} kbps (audio excluded)",
            vbr / 1000
        );
    }
    eprintln!("         GOP factor: {:.2}", d.gop_factor);
    if let Some(gop) = analysis.gop_size {
        eprintln!(
            "            └─ GOP size: {}, B-frames: {:?}",
            gop,
            analysis
                .b_frame_count
                .or_else(|| {
                    warn!("☢️ [ANOMALY] b_frame_count missing during analysis reporting. Information invalidated.");
                    None
                })
        );
    }
    eprintln!("         Chroma factor: {:.2}", d.chroma_factor);
    if let Some(ref pf) = analysis.pix_fmt {
        eprintln!("            └─ Pixel format: {pf}");
    }
    eprintln!("         HDR factor: {:.2}", d.hdr_factor);
    if analysis.is_hdr == Some(true) {
        eprintln!("            └─ HDR content detected");
    }
    if d.content_type_adjustment != 0 {
        eprintln!(
            "         Content type adjustment: {:+} CRF",
            d.content_type_adjustment
        );
        if let Some(ct) = analysis.content_type {
            eprintln!("            └─ Type: {ct:?}");
        }
    }
    eprintln!();
}

fn log_medium_priority_factors(analysis: &QualityAnalysis, d: &AnalysisDetails) {
    eprintln!("      Medium Priority Factors:");
    eprintln!("         Aspect factor: {:.2}", d.aspect_factor);
    eprintln!("         Complexity factor: {:.2}", d.complexity_factor);
    if analysis.spatial_complexity.is_some() || analysis.temporal_complexity.is_some() {
        eprintln!(
            "            └─ SI: {:.1}, TI: {:.1}",
            analysis
                .spatial_complexity
                .unwrap_or(crate::constants::DEFAULT_COMPLEXITY_PRIOR),
            analysis
                .temporal_complexity
                .unwrap_or(crate::constants::DEFAULT_COMPLEXITY_PRIOR)
        );
    }
    eprintln!("         Grain factor: {:.2}", d.grain_factor);
    eprintln!("         Alpha factor: {:.2}", d.alpha_factor);
    eprintln!();
}

fn log_result_info(analysis: &QualityAnalysis, result: &MatchedQuality, encoder: EncoderType) {
    eprintln!("      Result:");
    eprintln!("         Effective BPP: {:.4}", result.effective_bpp);
    if let Some(fps) = analysis.fps {
        eprintln!("         FPS: {fps:.2}");
    }
    if let Some(duration) = analysis.duration_secs {
        eprintln!("         Duration: {duration:.1}s");
    }

    match encoder {
        EncoderType::Av1 | EncoderType::Hevc => {
            eprintln!("         ✅ Calculated CRF: {}", result.crf);
        }
        EncoderType::Jxl => {
            eprintln!("         ✅ Calculated distance: {:.2}", result.distance);
        }
    }
}

pub fn log_quality_analysis(
    analysis: &QualityAnalysis,
    result: &MatchedQuality,
    encoder: EncoderType,
) {
    if !crate::progress_mode::is_verbose_mode() {
        return;
    }
    let encoder_name = match encoder {
        EncoderType::Av1 => "AV1",
        EncoderType::Hevc => "HEVC",
        EncoderType::Jxl => "JXL",
    };

    let d = &result.analysis_details;
    let codec = parse_source_codec(&analysis.source_codec);

    log_analysis_header(encoder_name, d);
    log_source_info(analysis, codec, d);
    log_high_priority_factors(analysis, d);
    log_medium_priority_factors(analysis, d);
    log_result_info(analysis, result, encoder);
}

#[must_use]
#[allow(
    clippy::missing_panics_doc,
    reason = "Explicit panic on data corruption is intended and documented inline."
)]
pub fn from_video_detection(
    file_path: &str,
    codec: &str,
    width: u32,
    height: u32,
    bitrate: u64,
    fps: f64,
    duration_secs: f64,
    has_b_frames: bool,
    bit_depth: u8,
    file_size: u64,
) -> QualityAnalysis {
    let pixels_per_frame = f64::from(width) * f64::from(height);
    let pixels_per_second = pixels_per_frame * fps;

    let bpp = if pixels_per_second > 0.0_f64 && bitrate > 0 {
        f64::from(crate::numeric_cast::u64_to_u32_strict(bitrate, "bitrate").unwrap_or(0))
            / pixels_per_second
    } else {
        if pixels_per_second <= 0.0_f64 {
            eprintln!("   ⚠️  Warning: pixels_per_second is {pixels_per_second} for {file_path}");
        }
        if bitrate == 0 {
            eprintln!("   ⚠️  Warning: bitrate is 0 for {file_path}");
        }
        0.0_f64
    };

    QualityAnalysis {
        bpp,
        source_codec: codec.to_string(),
        width,
        height,
        has_b_frames,
        bit_depth,
        has_alpha: false,
        duration_secs: Some(duration_secs),
        fps: Some(fps),
        file_size,
        estimated_quality: None,
        ..Default::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct VideoAnalysisBuilder {
    analysis: QualityAnalysis,
}

impl VideoAnalysisBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn basic(
        mut self,
        codec: &str,
        width: u32,
        height: u32,
        fps: f64,
        duration_secs: f64,
    ) -> Self {
        self.analysis.source_codec = codec.to_string();
        self.analysis.width = width;
        self.analysis.height = height;
        self.analysis.fps = Some(fps);
        self.analysis.duration_secs = Some(duration_secs);
        self
    }

    #[must_use]
    pub const fn file_size(mut self, size: u64) -> Self {
        self.analysis.file_size = size;
        self
    }

    #[must_use]
    pub fn video_bitrate(mut self, bitrate: u64) -> Self {
        self.analysis.video_bitrate = Some(bitrate);
        if let (Some(fps), w, h) = (self.analysis.fps, self.analysis.width, self.analysis.height) {
            if fps > 0.0_f64 && w > 0 && h > 0 {
                let pixels = f64::from(w) * f64::from(h);
                self.analysis.bpp = (crate::numeric_cast::u64_to_f64(bitrate) / fps) / pixels;
            }
        }
        self
    }

    #[must_use]
    pub const fn gop(mut self, gop_size: u32, b_frames: u8) -> Self {
        self.analysis.gop_size = Some(gop_size);
        self.analysis.b_frame_count = Some(b_frames);
        self.analysis.has_b_frames = b_frames > 0;
        self
    }

    #[must_use]
    pub fn pix_fmt(mut self, fmt: &str) -> Self {
        self.analysis.pix_fmt = Some(fmt.to_string());
        self
    }

    #[must_use]
    pub fn color(mut self, color_space: &str, is_hdr: bool) -> Self {
        self.analysis.color_space = Some(color_space.to_string());
        self.analysis.is_hdr = Some(is_hdr);
        self
    }

    #[must_use]
    pub const fn content_type(mut self, ct: ContentType) -> Self {
        self.analysis.content_type = Some(ct);
        self
    }

    #[must_use]
    pub const fn bit_depth(mut self, depth: u8) -> Self {
        self.analysis.bit_depth = depth;
        self
    }

    #[must_use]
    pub const fn complexity(mut self, spatial: f64, temporal: f64) -> Self {
        self.analysis.spatial_complexity = Some(spatial);
        self.analysis.temporal_complexity = Some(temporal);
        self
    }

    #[must_use]
    pub const fn film_grain(mut self, has_grain: bool) -> Self {
        self.analysis.has_film_grain = Some(has_grain);
        self
    }

    #[must_use]
    pub fn preset(mut self, preset: &str) -> Self {
        self.analysis.encoder_preset = Some(preset.to_string());
        self
    }

    #[must_use]
    pub fn build(self) -> QualityAnalysis {
        self.analysis
    }
}

#[derive(Debug, Clone)]
pub struct SkipDecision {
    pub should_skip: bool,
    pub reason: String,
    pub codec: SourceCodec,
}

#[must_use]
pub fn should_skip_video_codec(codec_str: &str) -> SkipDecision {
    let codec = parse_source_codec(codec_str);

    // Normal mode: skip all modern codecs (HEVC, AV1, VP9, VVC, AV2) — already modern, no need to process.
    // Only when Apple-compat flag is on do we convert AV1/VP9/VVC/AV2 via should_skip_video_codec_apple_compat (skip HEVC only).
    let should_skip = matches!(
        codec,
        SourceCodec::H265
            | SourceCodec::Av1
            | SourceCodec::Vp9
            | SourceCodec::Vvc
            | SourceCodec::Av2
    );

    let reason = if should_skip {
        let codec_name = match codec {
            SourceCodec::H265 => "H.265/HEVC",
            SourceCodec::Av1 => "AV1",
            SourceCodec::Vp9 => "VP9",
            SourceCodec::Vvc => "H.266/VVC (cutting-edge)",
            SourceCodec::Av2 => "AV2 (cutting-edge)",
            _ => "modern codec",
        };
        format!(
            "Source is {codec_name} - skipping (modern format; use Apple-compat mode to convert to HEVC)"
        )
    } else {
        String::new()
    };

    SkipDecision {
        should_skip,
        reason,
        codec,
    }
}

#[must_use]
pub fn should_skip_video_codec_apple_compat(codec_str: &str) -> SkipDecision {
    let codec = parse_source_codec(codec_str);

    let should_skip = matches!(codec, SourceCodec::H265);

    let reason = if should_skip {
        "Source is H.265/HEVC - already Apple compatible, skipping".to_string()
    } else {
        String::new()
    };

    SkipDecision {
        should_skip,
        reason,
        codec,
    }
}

/// True only when we may keep best-effort HEVC/AV1 output on compression/quality failure.
/// - Apple-incompatible (AV1, VP9, VVC, AV2): user still gets an importable file.
/// - ProRes/DNxHD are NOT included: decision is strictly by SSIM and size balance.
#[must_use]
pub fn is_apple_incompatible_video_codec(codec_str: &str) -> bool {
    matches!(
        parse_source_codec(codec_str),
        SourceCodec::Av1 | SourceCodec::Vp9 | SourceCodec::Vvc | SourceCodec::Av2
    )
}

/// Predicate for keeping Apple-compat fallback HEVC output.
// Rationale: This struct serves as a comprehensive configuration or state container where individual boolean flags are the most idiomatic and explicit way to represent discrete options.
#[allow(
    clippy::struct_excessive_bools,
    reason = "Data models naturally require multiple boolean flags to map independent configuration features. Grouping them into bitflags would break explicit serde mapping."
)]
#[derive(Debug, Clone, Copy)]
pub struct AppleFallbackKeepRequest<'a> {
    pub codec_str: &'a str,
    pub total_file_compressed: bool,
    pub total_size_ratio: f64,
    pub allow_size_tolerance: bool,
    pub apple_compat: bool,
    pub source_is_gif: bool,
}

#[must_use]
pub fn should_keep_apple_fallback_hevc_output(request: AppleFallbackKeepRequest<'_>) -> bool {
    // If the source is already Apple-native (like GIF), we never allow fallback to a larger file.
    if request.source_is_gif || is_apple_native_format(request.codec_str) {
        return false;
    }
    if !request.apple_compat || !is_apple_incompatible_video_codec(request.codec_str) {
        return false;
    }
    request.total_file_compressed
        || (request.allow_size_tolerance && request.total_size_ratio < 1.01)
}

#[must_use]
pub fn is_apple_native_format(codec_str: &str) -> bool {
    let codec = parse_source_codec(codec_str);
    matches!(
        codec,
        SourceCodec::H264
            | SourceCodec::H265
            | SourceCodec::ProRes
            | SourceCodec::Mjpeg
            | SourceCodec::Gif
            | SourceCodec::Jpeg
            | SourceCodec::Png
            | SourceCodec::Heic
    )
}

#[must_use]
pub fn is_size_guard_active(codec_str: &str, apple_compat: bool) -> bool {
    !apple_compat || is_apple_native_format(codec_str)
}

#[must_use]
pub fn should_skip_image_format(format_str: &str, is_lossless: bool) -> SkipDecision {
    let codec = parse_source_codec(format_str);

    // Modern lossy static formats: skip to avoid generational loss.
    // WebP/AVIF lossy static are skipped; HEIC/HEIF lossy static follow the same pattern.
    let is_modern_lossy = !is_lossless
        && matches!(
            codec,
            SourceCodec::WebpStatic | SourceCodec::Avif | SourceCodec::Heic | SourceCodec::JpegXl
        );

    let is_jxl = matches!(codec, SourceCodec::JpegXl);

    // Lossless HEIC/HEIF: allow conversion to JXL (archival-friendly format).
    // Lossless HEIC/HEIF is rare but valuable; JXL provides better compression
    // and broader compatibility while maintaining mathematical losslessness.
    let is_heic_lossless = matches!(codec, SourceCodec::Heic) && is_lossless;

    let should_skip = is_modern_lossy || is_jxl;

    let reason = if should_skip {
        let codec_name = match codec {
            SourceCodec::WebpStatic => "lossy WebP",
            SourceCodec::Avif => "lossy AVIF",
            SourceCodec::Heic if !is_lossless => "lossy HEIC/HEIF",
            SourceCodec::Heic => "lossless HEIC/HEIF (converts to JXL)",
            SourceCodec::JpegXl => "JPEG XL (already optimal)",
            _ => "modern lossy format",
        };
        format!("Source is {codec_name} - skipping to avoid generational loss")
    } else if is_heic_lossless {
        // Lossless HEIC/HEIF is not skipped; it will be converted to JXL.
        String::new()
    } else {
        String::new()
    };

    SkipDecision {
        should_skip,
        reason,
        codec,
    }
}

#[must_use]
pub fn from_image_analysis(
    format: &str,
    width: u32,
    height: u32,
    bit_depth: u8,
    has_alpha: bool,
    file_size: u64,
    duration_secs: Option<f64>,
    fps: Option<f64>,
    estimated_quality: Option<u8>,
) -> QualityAnalysis {
    let pixels = u64::from(width) * u64::from(height);

    let bpp = if let (Some(duration), Some(frame_rate)) = (duration_secs, fps) {
        if duration > 0.0_f64 && frame_rate > 0.0_f64 {
            let total_frames = crate::numeric_cast::f64_to_u64_sat(duration * frame_rate);
            let bits_per_frame = crate::numeric_cast::u64_to_f64(file_size) * 8.0_f64
                / crate::numeric_cast::u64_to_f64(total_frames.max(1));
            bits_per_frame / crate::numeric_cast::u64_to_f64(pixels.max(1))
        } else {
            crate::numeric_cast::u64_to_f64(file_size)
                / crate::numeric_cast::u64_to_f64(pixels.max(1))
        }
    } else {
        crate::numeric_cast::u64_to_f64(file_size) / crate::numeric_cast::u64_to_f64(pixels.max(1))
    };

    QualityAnalysis {
        bpp,
        source_codec: format.to_string(),
        width,
        height,
        has_b_frames: false,
        bit_depth,
        has_alpha,
        duration_secs,
        fps,
        file_size,
        estimated_quality,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_av1_crf_calculation() {
        let analysis = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: true,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: Some(60.0_f64),
            fps: Some(30.0_f64),
            file_size: 100_000_000,
            estimated_quality: None,
            ..Default::default()
        };

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
        // Updated: AV1 CRF range is now 0.0-51.0 (not 15.0-40.0) after removing artificial constraints
        assert!(result.crf >= 0.0 && result.crf <= 51.0);
        assert!(result.analysis_details.confidence > 0.5_f64);
    }

    #[test]
    fn test_hevc_crf_calculation() {
        let analysis = QualityAnalysis {
            bpp: 0.5,
            source_codec: "gif".to_string(),
            width: 640,
            height: 480,
            has_b_frames: false,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: Some(5.0_f64),
            fps: Some(10.0_f64),
            file_size: 5_000_000,
            estimated_quality: None,
            ..Default::default()
        };

        let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.crf <= 35.0);
    }

    #[test]
    fn test_size_guard_in_apple_compat_is_disabled_for_non_apple_native_inputs() {
        // Apple compat should never size-guard non-apple-native sources such as WebP/AVIF:
        // compatibility takes priority and the guard is only meaningful for already-native inputs.
        assert!(!is_size_guard_active("webp", true));
        assert!(!is_size_guard_active("avif", true));

        // Apple-native inputs can keep the size guard active under apple_compat.
        assert!(is_size_guard_active("gif", true));
        assert!(is_size_guard_active("jpeg", true));
        assert!(is_size_guard_active("png", true));
    }

    #[test]
    fn test_jxl_distance_with_quality() {
        let analysis = QualityAnalysis {
            bpp: 0.0,
            source_codec: "jpeg".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: false,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: None,
            fps: None,
            file_size: 500_000,
            estimated_quality: Some(85),
            ..Default::default()
        };

        let result = calculate_jxl_distance(&analysis).unwrap_or_else(|e| panic!("{e}"));
        assert!((result.distance - 1.5).abs() < 0.2);
    }

    #[test]
    fn test_gop_factor() {
        assert!(calculate_gop_factor(Some(1), 0) < 0.8_f64);
        assert!(calculate_gop_factor(Some(250), 3) > 1.3_f64);
        assert!((calculate_gop_factor(Some(30), 2) - 1.08).abs() < 0.1_f64);
    }

    #[test]
    fn test_chroma_factor() {
        assert!((calculate_chroma_factor(Some("yuv420p")) - 1.0).abs() < 0.01_f64);
        assert!(calculate_chroma_factor(Some("yuv444p")) > 1.1_f64);
        assert!(calculate_chroma_factor(Some("rgb24")) > 1.1_f64);
    }

    #[test]
    fn test_hdr_factor() {
        assert!((calculate_hdr_factor(None, Some("bt709")) - 1.0).abs() < 0.01_f64);
        assert!(calculate_hdr_factor(Some(true), None) > 1.2_f64);
        assert!(calculate_hdr_factor(None, Some("bt2020nc")) > 1.1_f64);
    }

    #[test]
    fn test_quality_bias() {
        let analysis = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            file_size: 100_000_000,
            fps: Some(30.0_f64),
            duration_secs: Some(60.0_f64),
            ..Default::default()
        };

        let conservative = calculate_av1_crf_with_options(
            &analysis,
            MatchMode::Quality,
            QualityBias::Conservative,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let balanced =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Balanced)
                .unwrap_or_else(|e| panic!("{e}"));
        let aggressive =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Aggressive)
                .unwrap_or_else(|e| panic!("{e}"));

        assert!(conservative.crf <= balanced.crf);
        assert!(aggressive.crf >= balanced.crf);
    }

    #[test]
    fn test_parse_source_codec() {
        assert_eq!(parse_source_codec("h264"), SourceCodec::H264);
        assert_eq!(parse_source_codec("H.265/HEVC"), SourceCodec::H265);
        assert_eq!(parse_source_codec("AV1"), SourceCodec::Av1);
        assert_eq!(parse_source_codec("GIF"), SourceCodec::Gif);

        assert_eq!(parse_source_codec("VVC"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("H.266"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("h266"), SourceCodec::Vvc);
        assert_eq!(parse_source_codec("AV2"), SourceCodec::Av2);
        assert_eq!(parse_source_codec("avm"), SourceCodec::Av2);

        assert_eq!(parse_source_codec("JPEG XL"), SourceCodec::JpegXl);
        assert_eq!(parse_source_codec("jxl"), SourceCodec::JpegXl);
        assert_eq!(parse_source_codec("AVIF"), SourceCodec::Avif);
        assert_eq!(parse_source_codec("HEIC"), SourceCodec::Heic);

        assert_eq!(parse_source_codec("FFV1"), SourceCodec::Ffv1);
        assert_eq!(parse_source_codec("UTVideo"), SourceCodec::UtVideo);
        assert_eq!(parse_source_codec("HuffYUV"), SourceCodec::HuffYuv);

        assert_eq!(parse_source_codec("unknown_codec"), SourceCodec::Unknown);
    }

    #[test]
    fn test_codec_properties() {
        assert!(SourceCodec::H265.is_modern());
        assert!(SourceCodec::Av1.is_modern());
        assert!(SourceCodec::Vvc.is_modern());
        assert!(SourceCodec::Av2.is_modern());
        assert!(!SourceCodec::H264.is_modern());

        assert!(SourceCodec::Vvc.is_cutting_edge());
        assert!(SourceCodec::Av2.is_cutting_edge());
        assert!(!SourceCodec::Av1.is_cutting_edge());

        assert!(SourceCodec::Ffv1.is_lossless());
        assert!(SourceCodec::Png.is_lossless());
        assert!(!SourceCodec::H264.is_lossless());
    }

    #[test]
    fn test_codec_efficiency_ordering() {
        assert!(SourceCodec::Av1.efficiency_factor() < SourceCodec::H265.efficiency_factor());
        assert!(SourceCodec::H265.efficiency_factor() < SourceCodec::H264.efficiency_factor());
        assert!(SourceCodec::Vvc.efficiency_factor() < SourceCodec::Av1.efficiency_factor());
        assert!(SourceCodec::Av2.efficiency_factor() <= SourceCodec::Vvc.efficiency_factor());

        assert!(SourceCodec::Gif.efficiency_factor() > 2.0_f64);
    }

    #[test]
    fn test_confidence_calculation() {
        let complete = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: true,
            bit_depth: 8,
            has_alpha: false,
            duration_secs: Some(60.0_f64),
            fps: Some(30.0_f64),
            file_size: 100_000_000,
            estimated_quality: Some(85),
            video_bitrate: Some(5_000_000),
            gop_size: Some(60),
            b_frame_count: Some(3),
            pix_fmt: Some("yuv420p".to_string()),
            ..Default::default()
        };
        let result = calculate_av1_crf(&complete).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.analysis_details.confidence > 0.8_f64);

        let minimal = QualityAnalysis {
            bpp: 0.0,
            source_codec: "unknown".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: false,
            bit_depth: 0,
            has_alpha: false,
            duration_secs: None,
            fps: None,
            file_size: 100_000_000,
            estimated_quality: None,
            ..Default::default()
        };
        let result = calculate_av1_crf(&minimal).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.analysis_details.confidence < 0.7_f64);
    }

    #[test]
    fn test_should_skip_video_codec() {
        assert!(should_skip_video_codec("hevc").should_skip);
        assert!(should_skip_video_codec("h265").should_skip);
        assert!(should_skip_video_codec("av1").should_skip);
        assert!(should_skip_video_codec("vp9").should_skip);
        assert!(should_skip_video_codec("vvc").should_skip);
        assert!(should_skip_video_codec("h266").should_skip);
        assert!(should_skip_video_codec("av2").should_skip);

        assert!(!should_skip_video_codec("h264").should_skip);
        assert!(!should_skip_video_codec("mpeg4").should_skip);
        assert!(!should_skip_video_codec("prores").should_skip);
        assert!(!should_skip_video_codec("ffv1").should_skip);
    }

    #[test]
    fn test_should_skip_image_format() {
        // Modern lossy static: skip (avoid generational loss)
        assert!(should_skip_image_format("webp", false).should_skip);
        assert!(should_skip_image_format("avif", false).should_skip);
        assert!(should_skip_image_format("heic", false).should_skip); // lossy HEIC → skip

        // JXL: always skip (already optimal)
        assert!(should_skip_image_format("jxl", true).should_skip);
        assert!(should_skip_image_format("jxl", false).should_skip);

        // Modern lossless static: convert to JXL
        assert!(!should_skip_image_format("webp", true).should_skip);
        assert!(!should_skip_image_format("avif", true).should_skip);
        assert!(!should_skip_image_format("heic", true).should_skip); // lossless HEIC → JXL

        // Legacy formats: convert to JXL
        assert!(!should_skip_image_format("jpeg", false).should_skip);
        assert!(!should_skip_image_format("png", true).should_skip);
        assert!(!should_skip_image_format("gif", true).should_skip);
        assert!(!should_skip_image_format("tiff", true).should_skip);
        assert!(!should_skip_image_format("heif", true).should_skip); // lossless HEIF → JXL
    }

    #[test]
    fn test_precision_1080p_h264_8mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .bit_depth(8)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        eprintln!("1080p H.264 8Mbps test:");
        eprintln!("  raw_bpp: {:.4}", result.analysis_details.raw_bpp);
        eprintln!("  effective_bpp: {:.4}", result.effective_bpp);
        eprintln!(
            "  codec_factor: {:.2}",
            result.analysis_details.codec_factor
        );
        eprintln!("  gop_factor: {:.2}", result.analysis_details.gop_factor);
        eprintln!("  CRF: {}", result.crf);

        assert!(
            result.crf >= 18.0 && result.crf <= 32.0,
            "1080p H.264 8Mbps: expected CRF 18-32, got {}",
            result.crf
        );

        assert!(
            result.effective_bpp > 0.05_f64 && result.effective_bpp < 2.0_f64,
            "Effective BPP out of range: {}",
            result.effective_bpp
        );
    }

    #[test]
    fn test_precision_4k_h264_20mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(20_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .bit_depth(8)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 22.0 && result.crf <= 32.0,
            "4K H.264 20Mbps: expected CRF 22-32, got {}",
            result.crf
        );
    }

    #[test]
    fn test_precision_animation_content() {
        let base = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(5_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .build();

        let animation = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(5_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::Animation)
            .build();

        let base_result = calculate_av1_crf(&base).unwrap_or_else(|e| panic!("{e}"));
        let anim_result = calculate_av1_crf(&animation).unwrap_or_else(|e| panic!("{e}"));

        let crf_diff = crate::numeric_cast::f32_to_i32_sat(anim_result.crf)
            - crate::numeric_cast::f32_to_i32_sat(base_result.crf);
        assert!(
            (2_i32..=6_i32).contains(&crf_diff),
            "Animation CRF adjustment: expected +2 to +6, got {crf_diff:+}"
        );
    }

    #[test]
    fn test_precision_film_grain_content() {
        let base = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .build();

        let grain = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 24.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::FilmGrain)
            .film_grain(true)
            .build();

        let base_result = calculate_av1_crf(&base).unwrap_or_else(|e| panic!("{e}"));
        let grain_result = calculate_av1_crf(&grain).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            grain_result.crf <= base_result.crf,
            "Film grain CRF should be <= baseline: grain={}, base={}",
            grain_result.crf,
            base_result.crf
        );

        assert!(
            grain_result.analysis_details.grain_factor > 1.1_f64,
            "Grain factor should be > 1.1: {}",
            grain_result.analysis_details.grain_factor
        );
    }

    #[test]
    fn test_precision_hdr_content() {
        let sdr = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .color("bt709", false)
            .bit_depth(10)
            .build();

        let hdr = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .color("bt2020nc", true)
            .bit_depth(10)
            .build();

        let sdr_result = calculate_av1_crf(&sdr).unwrap_or_else(|e| panic!("{e}"));
        let hdr_result = calculate_av1_crf(&hdr).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            hdr_result.crf <= sdr_result.crf,
            "HDR should have CRF <= SDR: HDR={}, SDR={}",
            hdr_result.crf,
            sdr_result.crf
        );
    }

    #[test]
    fn test_precision_chroma_subsampling() {
        let yuv420 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let yuv444 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv444p")
            .build();

        let yuv420_result = calculate_av1_crf(&yuv420).unwrap_or_else(|e| panic!("{e}"));
        let yuv444_result = calculate_av1_crf(&yuv444).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            yuv444_result.crf <= yuv420_result.crf,
            "YUV444 should have CRF <= YUV420: 444={}, 420={}",
            yuv444_result.crf,
            yuv420_result.crf
        );
    }

    #[test]
    fn test_precision_gop_structure() {
        let all_intra = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(20_000_000)
            .gop(1, 0)
            .pix_fmt("yuv420p")
            .build();

        let long_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(250, 3)
            .pix_fmt("yuv420p")
            .build();

        let intra_result = calculate_av1_crf(&all_intra).unwrap_or_else(|e| panic!("{e}"));
        let gop_result = calculate_av1_crf(&long_gop).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            intra_result.analysis_details.gop_factor < 0.8_f64,
            "All-intra GOP factor should be < 0.8: {}",
            intra_result.analysis_details.gop_factor
        );
        assert!(
            gop_result.analysis_details.gop_factor > 1.2_f64,
            "Long GOP factor should be > 1.2: {}",
            gop_result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_precision_screen_recording() {
        let screen = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(2_000_000)
            .gop(60, 0)
            .pix_fmt("yuv420p")
            .content_type(ContentType::ScreenRecording)
            .build();

        let result = calculate_av1_crf(&screen).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 25.0,
            "Screen recording should allow CRF >= 25, got {}",
            result.crf
        );

        assert!(
            result.analysis_details.content_type_adjustment > 0,
            "Screen recording should have positive CRF adjustment"
        );
    }

    #[test]
    fn test_precision_ultrawide_aspect() {
        let standard = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let ultrawide = VideoAnalysisBuilder::new()
            .basic("h264", 2560, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let _standard_result = calculate_av1_crf(&standard).unwrap_or_else(|e| panic!("{e}"));
        let ultrawide_result = calculate_av1_crf(&ultrawide).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            ultrawide_result.analysis_details.aspect_factor > 1.0_f64,
            "Ultra-wide should have aspect factor > 1.0: {}",
            ultrawide_result.analysis_details.aspect_factor
        );
    }

    #[test]
    fn test_precision_codec_efficiency() {
        let h264_source = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let hevc_source = VideoAnalysisBuilder::new()
            .basic("hevc", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let h264_result = calculate_av1_crf(&h264_source).unwrap_or_else(|e| panic!("{e}"));
        let hevc_result = calculate_av1_crf(&hevc_source).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            hevc_result.analysis_details.codec_factor < h264_result.analysis_details.codec_factor,
            "HEVC should have lower codec factor: HEVC={}, H264={}",
            hevc_result.analysis_details.codec_factor,
            h264_result.analysis_details.codec_factor
        );
    }

    #[test]
    fn test_precision_boundary_low_bpp() {
        let low_bpp = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(500_000)
            .gop(60, 0)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&low_bpp).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf <= 40.0,
            "Ultra-low BPP should cap CRF at 40, got {}",
            result.crf
        );
        assert!(
            result.crf >= 28.0,
            "Ultra-low BPP should have CRF >= 28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_precision_boundary_high_bpp() {
        let high_bpp = VideoAnalysisBuilder::new()
            .basic("prores", 1920, 1080, 30.0, 60.0)
            .video_bitrate(150_000_000)
            .gop(1, 0)
            .pix_fmt("yuv422p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&high_bpp).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 6.0,
            "Ultra-high BPP should floor CRF at 6, got {}",
            result.crf
        );
        assert!(
            result.crf <= 25.0,
            "ProRes source should produce CRF <= 25, got {}",
            result.crf
        );
    }

    #[test]
    fn test_precision_jxl_jpeg_q85() {
        let jpeg = QualityAnalysis {
            source_codec: "jpeg".to_string(),
            width: 1920,
            height: 1080,
            file_size: 500_000,
            estimated_quality: Some(85),
            ..Default::default()
        };

        let result = calculate_jxl_distance(&jpeg).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            (result.distance - 1.5).abs() < 0.3,
            "JPEG Q85 should produce distance ~1.5, got {}",
            result.distance
        );
        assert!(
            (result.analysis_details.confidence - calculate_confidence_v3(&jpeg)).abs()
                < f64::EPSILON,
            "JPEG estimated-quality path should use calculated confidence, got {}",
            result.analysis_details.confidence
        );
        assert!(
            result.analysis_details.confidence < 0.9_f64,
            "Sparse JPEG metadata should not be reported as fixed 0.9 confidence, got {}",
            result.analysis_details.confidence
        );
    }

    #[test]
    fn test_precision_jxl_jpeg_q95() {
        let jpeg = QualityAnalysis {
            source_codec: "jpeg".to_string(),
            width: 1920,
            height: 1080,
            file_size: 1_000_000,
            estimated_quality: Some(95),
            ..Default::default()
        };

        let result = calculate_jxl_distance(&jpeg).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            (result.distance - 0.5).abs() < 0.3,
            "JPEG Q95 should produce distance ~0.5, got {}",
            result.distance
        );
    }

    #[test]
    fn test_precision_hevc_gif_source() {
        let gif = QualityAnalysis {
            bpp: 0.5,
            source_codec: "gif".to_string(),
            width: 640,
            height: 480,
            bit_depth: 8,
            duration_secs: Some(5.0_f64),
            fps: Some(10.0_f64),
            file_size: 5_000_000,
            ..Default::default()
        };

        let result = calculate_hevc_crf(&gif).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 20.0 && result.crf <= 32.0,
            "GIF to HEVC should produce CRF 20-32, got {}",
            result.crf
        );

        assert!(
            result.analysis_details.codec_factor > 2.0_f64,
            "GIF codec factor should be > 2.0: {}",
            result.analysis_details.codec_factor
        );
    }

    #[test]
    fn test_precision_consistency() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result1 = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
        let result2 = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            crate::float_compare::approx_eq_crf(result1.crf, result2.crf),
            "Same input should produce same CRF"
        );
        assert!(
            (result1.effective_bpp - result2.effective_bpp).abs() < 0.000_1_f64,
            "Same input should produce same effective BPP"
        );
    }

    #[test]
    fn test_precision_mode_comparison() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let quality =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Balanced)
                .unwrap_or_else(|e| panic!("{e}"));
        let size =
            calculate_av1_crf_with_options(&analysis, MatchMode::Size, QualityBias::Balanced)
                .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            size.crf >= quality.crf,
            "Size mode should have CRF >= Quality mode: Size={}, Quality={}",
            size.crf,
            quality.crf
        );
    }

    #[test]
    fn test_strict_1080p_5mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 120.0)
            .video_bitrate(5_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 23.0 && result.crf <= 27.0,
            "STRICT: 1080p 5Mbps expected CRF 23-27, got {}",
            result.crf
        );
    }

    #[test]
    fn test_strict_720p_2mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1280, 720, 30.0, 60.0)
            .video_bitrate(2_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 25.0 && result.crf <= 29.0,
            "STRICT: 720p 2Mbps expected CRF 25-29, got {}",
            result.crf
        );
    }

    #[test]
    fn test_strict_4k_15mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 24.0 && result.crf <= 28.0,
            "STRICT: 4K 15Mbps expected CRF 24-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_extremely_low_bitrate() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(500_000)
            .gop(60, 0)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 30.0 && result.crf <= 40.0,
            "EDGE: Extremely low bitrate should cap CRF 30-40, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_extremely_high_bitrate() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("prores", 1920, 1080, 30.0, 60.0)
            .video_bitrate(100_000_000)
            .gop(1, 0)
            .pix_fmt("yuv422p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 0.0 && result.crf <= 30.0,
            "EDGE: Extremely high bitrate should produce low CRF (high quality), got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_small_resolution() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 320, 240, 15.0, 30.0)
            .video_bitrate(500_000)
            .gop(30, 1)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 10.0 && result.crf <= 25.0,
            "EDGE: Small resolution high-bpp should produce CRF 10-25, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_8k_resolution() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 7680, 4320, 30.0, 60.0)
            .video_bitrate(50_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 28.0 && result.crf <= 38.0,
            "EDGE: 8K low-bpp should produce CRF 28-38, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_high_framerate() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 120.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(120, 3)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 18.0 && result.crf <= 28.0,
            "EDGE: 120fps should produce CRF 18-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_short_gop() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(10_000_000)
            .gop(2, 0)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.analysis_details.gop_factor < 0.9_f64,
            "EDGE: Short GOP factor should be < 0.9, got {}",
            result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_edge_max_bframes() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(250, 8)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.analysis_details.gop_factor > 1.3_f64,
            "EDGE: Max B-frames GOP factor should be > 1.3, got {}",
            result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_edge_10bit_hdr() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, 30.0, 60.0)
            .video_bitrate(20_000_000)
            .gop(60, 3)
            .pix_fmt("yuv420p10le")
            .color("bt2020nc", true)
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.analysis_details.hdr_factor > 1.1_f64,
            "EDGE: HDR factor should be > 1.1, got {}",
            result.analysis_details.hdr_factor
        );

        assert!(
            result.crf >= 20.0 && result.crf <= 28.0,
            "EDGE: 10-bit HDR should produce CRF 20-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_rgb_format() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(15_000_000)
            .gop(60, 2)
            .pix_fmt("rgb24")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.analysis_details.chroma_factor > 1.1_f64,
            "EDGE: RGB chroma factor should be > 1.1, got {}",
            result.analysis_details.chroma_factor
        );
    }

    #[test]
    fn test_edge_vertical_video() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1080, 1920, 30.0, 60.0)
            .video_bitrate(5_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 20.0 && result.crf <= 30.0,
            "EDGE: Vertical video should produce CRF 20-30, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_ultrawide_cinema() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 2560, 1080, 24.0, 120.0)
            .video_bitrate(8_000_000)
            .gop(48, 2)
            .pix_fmt("yuv420p")
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 20.0 && result.crf <= 28.0,
            "EDGE: Ultra-wide cinema should produce CRF 20-28, got {}",
            result.crf
        );
    }

    #[test]
    fn test_edge_lossless_source() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("ffv1", 1920, 1080, 30.0, 60.0)
            .video_bitrate(200_000_000)
            .gop(1, 0)
            .pix_fmt("yuv444p10le")
            .bit_depth(10)
            .build();

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            result.crf >= 15.0 && result.crf <= 25.0,
            "EDGE: Lossless source should produce CRF 15-25, got {}",
            result.crf
        );
    }

    #[test]
    fn test_factor_gop_isolation() {
        let short_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(10, 1)
            .pix_fmt("yuv420p")
            .build();

        let long_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(250, 3)
            .pix_fmt("yuv420p")
            .build();

        let short_result = calculate_av1_crf(&short_gop).unwrap_or_else(|e| panic!("{e}"));
        let long_result = calculate_av1_crf(&long_gop).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            long_result.analysis_details.gop_factor > short_result.analysis_details.gop_factor,
            "Long GOP factor ({}) should be > short GOP factor ({})",
            long_result.analysis_details.gop_factor,
            short_result.analysis_details.gop_factor
        );
    }

    #[test]
    fn test_factor_chroma_isolation() {
        let yuv420 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let yuv444 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv444p")
            .build();

        let yuv420_result = calculate_av1_crf(&yuv420).unwrap_or_else(|e| panic!("{e}"));
        let yuv444_result = calculate_av1_crf(&yuv444).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            yuv444_result.analysis_details.chroma_factor
                > yuv420_result.analysis_details.chroma_factor,
            "YUV444 chroma factor ({}) should be > YUV420 ({})",
            yuv444_result.analysis_details.chroma_factor,
            yuv420_result.analysis_details.chroma_factor
        );
    }

    #[test]
    fn test_factor_hdr_isolation() {
        let sdr = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .build();

        let hdr = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .color("bt2020nc", true)
            .build();

        let sdr_result = calculate_av1_crf(&sdr).unwrap_or_else(|e| panic!("{e}"));
        let hdr_result = calculate_av1_crf(&hdr).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            hdr_result.analysis_details.hdr_factor > sdr_result.analysis_details.hdr_factor,
            "HDR factor ({}) should be > SDR ({})",
            hdr_result.analysis_details.hdr_factor,
            sdr_result.analysis_details.hdr_factor
        );
    }

    #[test]
    fn test_factor_content_type_isolation() {
        let live_action = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::LiveAction)
            .build();

        let animation = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .content_type(ContentType::Animation)
            .build();

        let live_result = calculate_av1_crf(&live_action).unwrap_or_else(|e| panic!("{e}"));
        let anim_result = calculate_av1_crf(&animation).unwrap_or_else(|e| panic!("{e}"));

        assert!(
            anim_result.analysis_details.content_type_adjustment
                > live_result.analysis_details.content_type_adjustment,
            "Animation adjustment ({}) should be > LiveAction ({})",
            anim_result.analysis_details.content_type_adjustment,
            live_result.analysis_details.content_type_adjustment
        );

        assert!(
            anim_result.crf > live_result.crf,
            "Animation CRF ({}) should be > LiveAction ({})",
            anim_result.crf,
            live_result.crf
        );
    }

    #[test]
    fn test_factor_bias_isolation() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, 30.0, 60.0)
            .video_bitrate(8_000_000)
            .gop(60, 2)
            .pix_fmt("yuv420p")
            .build();

        let conservative = calculate_av1_crf_with_options(
            &analysis,
            MatchMode::Quality,
            QualityBias::Conservative,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let balanced =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Balanced)
                .unwrap_or_else(|e| panic!("{e}"));
        let aggressive =
            calculate_av1_crf_with_options(&analysis, MatchMode::Quality, QualityBias::Aggressive)
                .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            conservative.crf < balanced.crf,
            "Conservative CRF ({}) should be < Balanced ({})",
            conservative.crf,
            balanced.crf
        );
        assert!(
            balanced.crf < aggressive.crf,
            "Balanced CRF ({}) should be < Aggressive ({})",
            balanced.crf,
            aggressive.crf
        );

        assert!(
            (balanced.crf - conservative.crf - 2.0).abs() < 0.1,
            "Conservative should be exactly 2 less than Balanced"
        );
        assert!(
            (aggressive.crf - balanced.crf - 2.0).abs() < 0.1,
            "Aggressive should be exactly 2 more than Balanced"
        );
    }
}

#[test]
fn test_apple_compat_skip_hevc_only() {
    let hevc = should_skip_video_codec_apple_compat("hevc");
    assert!(
        hevc.should_skip,
        "HEVC should be skipped in Apple compat mode"
    );
    assert!(
        hevc.reason.contains("Apple compatible"),
        "HEVC skip reason should mention Apple compatible"
    );

    let h265 = should_skip_video_codec_apple_compat("h265");
    assert!(
        h265.should_skip,
        "H.265 should be skipped in Apple compat mode"
    );
}

#[test]
fn test_apple_compat_convert_vp9() {
    let vp9 = should_skip_video_codec_apple_compat("vp9");
    assert!(
        !vp9.should_skip,
        "VP9 should NOT be skipped in Apple compat mode"
    );
    assert_eq!(vp9.codec, SourceCodec::Vp9);
}

#[test]
fn test_apple_compat_convert_av1() {
    let av1 = should_skip_video_codec_apple_compat("av1");
    assert!(
        !av1.should_skip,
        "AV1 should NOT be skipped in Apple compat mode"
    );
    assert_eq!(av1.codec, SourceCodec::Av1);
}

#[test]
fn test_apple_compat_convert_vvc() {
    let vvc = should_skip_video_codec_apple_compat("vvc");
    assert!(
        !vvc.should_skip,
        "VVC should NOT be skipped in Apple compat mode"
    );

    let h266 = should_skip_video_codec_apple_compat("h266");
    assert!(
        !h266.should_skip,
        "H.266 should NOT be skipped in Apple compat mode"
    );
}

#[test]
fn test_apple_compat_convert_av2() {
    let av2 = should_skip_video_codec_apple_compat("av2");
    assert!(
        !av2.should_skip,
        "AV2 should NOT be skipped in Apple compat mode"
    );
}

#[test]
fn test_apple_compat_legacy_codecs() {
    assert!(!should_skip_video_codec("h264").should_skip);
    assert!(!should_skip_video_codec_apple_compat("h264").should_skip);

    assert!(!should_skip_video_codec("mpeg4").should_skip);
    assert!(!should_skip_video_codec_apple_compat("mpeg4").should_skip);

    assert!(!should_skip_video_codec("prores").should_skip);
    assert!(!should_skip_video_codec_apple_compat("prores").should_skip);
}

#[test]
fn test_apple_compat_vs_normal_mode() {
    assert!(should_skip_video_codec("vp9").should_skip);
    assert!(!should_skip_video_codec_apple_compat("vp9").should_skip);

    assert!(should_skip_video_codec("av1").should_skip);
    assert!(!should_skip_video_codec_apple_compat("av1").should_skip);

    assert!(should_skip_video_codec("hevc").should_skip);
    assert!(should_skip_video_codec_apple_compat("hevc").should_skip);

    assert!(!should_skip_video_codec("h264").should_skip);
    assert!(!should_skip_video_codec_apple_compat("h264").should_skip);
}

#[test]
fn test_apple_compat_codec_detection() {
    assert_eq!(
        should_skip_video_codec_apple_compat("vp9").codec,
        SourceCodec::Vp9
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("av1").codec,
        SourceCodec::Av1
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("hevc").codec,
        SourceCodec::H265
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("vvc").codec,
        SourceCodec::Vvc
    );
    assert_eq!(
        should_skip_video_codec_apple_compat("h264").codec,
        SourceCodec::H264
    );
}

#[test]
fn test_apple_compat_case_insensitive() {
    assert!(should_skip_video_codec_apple_compat("HEVC").should_skip);
    assert!(should_skip_video_codec_apple_compat("Hevc").should_skip);
    assert!(should_skip_video_codec_apple_compat("hevc").should_skip);

    assert!(!should_skip_video_codec_apple_compat("VP9").should_skip);
    assert!(!should_skip_video_codec_apple_compat("Vp9").should_skip);
    assert!(!should_skip_video_codec_apple_compat("vp9").should_skip);
}

#[test]
fn test_is_apple_incompatible_video_codec() {
    assert!(is_apple_incompatible_video_codec("av1"));
    assert!(is_apple_incompatible_video_codec("vp9"));
    assert!(is_apple_incompatible_video_codec("vvc"));
    assert!(is_apple_incompatible_video_codec("h266"));
    assert!(is_apple_incompatible_video_codec("av2"));
    assert!(is_apple_incompatible_video_codec("AV1"));
    assert!(is_apple_incompatible_video_codec("libaom-av1"));

    assert!(!is_apple_incompatible_video_codec("hevc"));
    assert!(!is_apple_incompatible_video_codec("h265"));
    assert!(!is_apple_incompatible_video_codec("h264"));
    assert!(!is_apple_incompatible_video_codec("H.264"));
    assert!(!is_apple_incompatible_video_codec("prores"));
    assert!(!is_apple_incompatible_video_codec("dnxhd"));
    assert!(!is_apple_incompatible_video_codec("ffv1"));
}

#[test]
fn test_strict_apple_compat_routing() {
    let test_cases = [
        ("h264", false, false),
        ("mpeg4", false, false),
        ("prores", false, false),
        ("hevc", true, true),
        ("h265", true, true),
        ("vp9", true, false),
        ("av1", true, false),
        ("vvc", true, false),
        ("h266", true, false),
        ("av2", true, false),
    ];

    for (codec, expected_normal, expected_apple) in test_cases {
        let normal = should_skip_video_codec(codec);
        let apple = should_skip_video_codec_apple_compat(codec);

        assert_eq!(
            normal.should_skip, expected_normal,
            "STRICT: {} normal mode: expected skip={}, got skip={}",
            codec, expected_normal, normal.should_skip
        );

        assert_eq!(
            apple.should_skip, expected_apple,
            "STRICT: {} Apple compat mode: expected skip={}, got skip={}",
            codec, expected_apple, apple.should_skip
        );
    }
}

#[test]
fn test_apple_compat_hevc_crf_vp9_source() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("vp9", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(45_000_000)
        .video_bitrate(6_000_000)
        .pix_fmt("yuv420p")
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 18.0 && result.crf <= 28.0,
        "VP9→HEVC CRF should be 18-28, got {:.1}",
        result.crf
    );
}

#[test]
fn test_apple_compat_hevc_crf_av1_source() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("av1", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(30_000_000)
        .video_bitrate(4_000_000)
        .pix_fmt("yuv420p")
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 16.0 && result.crf <= 26.0,
        "AV1→HEVC CRF should be 16-26, got {:.1}",
        result.crf
    );
}

#[test]
fn test_apple_compat_hevc_crf_4k_hdr() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("av1", 3840, 2160, 60.0, 120.0)
        .bit_depth(10)
        .file_size(1_800_000_000)
        .video_bitrate(120_000_000)
        .pix_fmt("yuv420p10le")
        .color("bt2020nc", true)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 0.0 && result.crf <= 22.0,
        "4K HDR should get CRF <= 22, got {:.1}",
        result.crf
    );
    assert!(
        result.analysis_details.hdr_factor > 1.0_f64,
        "HDR factor should increase effective BPP (>1.0), got {:.2}",
        result.analysis_details.hdr_factor
    );
}

#[test]
fn test_apple_compat_codec_efficiency() {
    assert!(SourceCodec::Av1.efficiency_factor() < SourceCodec::Vp9.efficiency_factor());
    assert!(
        (SourceCodec::Vp9.efficiency_factor() - SourceCodec::H265.efficiency_factor()).abs()
            < 0.1_f64
    );
    assert!(SourceCodec::Vvc.efficiency_factor() < SourceCodec::Av1.efficiency_factor());
}

#[test]
fn test_h264_to_hevc_crf_1080p_8mbps() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 1920, 1080, 30.0, 120.0)
        .bit_depth(8)
        .file_size(120_000_000)
        .video_bitrate(8_000_000)
        .pix_fmt("yuv420p")
        .gop(60, 2)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 18.0 && result.crf <= 26.0,
        "H.264 8Mbps 1080p→HEVC should get CRF 18-26, got {:.1}",
        result.crf
    );
    assert!(
        (result.analysis_details.codec_factor - 1.0).abs() < 0.2_f64,
        "H.264 codec factor should be ~1.0"
    );
}

#[test]
fn test_h264_to_hevc_crf_720p_4mbps() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 1280, 720, 30.0, 60.0)
        .bit_depth(8)
        .file_size(30_000_000)
        .video_bitrate(4_000_000)
        .pix_fmt("yuv420p")
        .gop(30, 2)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 20.0 && result.crf <= 28.0,
        "H.264 4Mbps 720p→HEVC should get CRF 20-28, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_to_hevc_crf_4k_20mbps() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 3840, 2160, 30.0, 180.0)
        .bit_depth(8)
        .file_size(450_000_000)
        .video_bitrate(20_000_000)
        .pix_fmt("yuv420p")
        .gop(60, 3)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 18.0 && result.crf <= 30.0,
        "H.264 20Mbps 4K→HEVC should get CRF 18-30, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_to_hevc_crf_low_bitrate() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 854, 480, 24.0, 300.0)
        .bit_depth(8)
        .file_size(45_000_000)
        .video_bitrate(1_200_000)
        .pix_fmt("yuv420p")
        .gop(48, 1)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 24.0 && result.crf <= 32.0,
        "H.264 1.2Mbps 480p→HEVC should get CRF 24-32, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_to_hevc_crf_bluray_quality() {
    let analysis = VideoAnalysisBuilder::new()
        .basic("h264", 1920, 1080, 24.0, 7200.0)
        .bit_depth(8)
        .file_size(4_500_000_000)
        .video_bitrate(40_000_000)
        .pix_fmt("yuv420p")
        .gop(24, 3)
        .build();

    let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        result.crf >= 0.0 && result.crf <= 22.0,
        "H.264 40Mbps Blu-ray→HEVC should get CRF 0-22, got {:.1}",
        result.crf
    );
}

#[test]
fn test_h264_vs_av1_efficiency_comparison() {
    let h264 = VideoAnalysisBuilder::new()
        .basic("h264", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(60_000_000)
        .video_bitrate(8_000_000)
        .pix_fmt("yuv420p")
        .build();

    let av1 = VideoAnalysisBuilder::new()
        .basic("av1", 1920, 1080, 30.0, 60.0)
        .bit_depth(8)
        .file_size(30_000_000)
        .video_bitrate(4_000_000)
        .pix_fmt("yuv420p")
        .build();

    let h264_result = calculate_hevc_crf(&h264).unwrap_or_else(|e| panic!("{e}"));
    let av1_result = calculate_hevc_crf(&av1).unwrap_or_else(|e| panic!("{e}"));

    let crf_diff = (h264_result.crf - av1_result.crf).abs();
    assert!(
        crf_diff <= 4.0,
        "H.264 vs AV1 CRF diff should be <=4, got {:.1} (H.264:{:.1}, AV1:{:.1})",
        crf_diff,
        h264_result.crf,
        av1_result.crf
    );
}

#[test]
fn test_h264_should_not_skip() {
    let decision = should_skip_video_codec("h264");
    assert!(!decision.should_skip, "H.264 should NOT be skipped");
    assert_eq!(decision.codec, SourceCodec::H264);

    let avc = should_skip_video_codec("avc");
    assert!(!avc.should_skip, "AVC should NOT be skipped");
}

#[test]
fn test_h264_apple_compat_should_not_skip() {
    let decision = should_skip_video_codec_apple_compat("h264");
    assert!(
        !decision.should_skip,
        "H.264 should NOT be skipped in Apple compat"
    );
    assert_eq!(decision.codec, SourceCodec::H264);
}

#[cfg(test)]
mod content_id_tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_temp_with_content(content: &[u8]) -> NamedTempFile {
        let mut file =
            NamedTempFile::new().unwrap_or_else(|e| panic!("Failed to create temp file: {e}"));
        file.write_all(content)
            .unwrap_or_else(|e| panic!("Failed to write to temp file: {e}"));
        file
    }

    #[test]
    fn test_identify_jpeg() {
        let file =
            create_temp_with_content(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F']);
        let codec = SourceCodec::identify_by_content(file.path())
            .unwrap_or_else(|| panic!("Should identify JPEG"));
        assert_eq!(codec, SourceCodec::Jpeg);
        assert!(codec.is_extension_compatible("jpg"));
        assert!(codec.is_extension_compatible("jpeg"));
        assert!(!codec.is_extension_compatible("png"));
    }

    #[test]
    fn test_identify_png() {
        let file = create_temp_with_content(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        let codec = SourceCodec::identify_by_content(file.path())
            .unwrap_or_else(|| panic!("Should identify PNG"));
        assert_eq!(codec, SourceCodec::Png);
        assert!(codec.is_extension_compatible("png"));
    }

    #[test]
    fn test_identify_mp4() {
        let file = create_temp_with_content(&[
            0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm',
        ]);
        let codec = SourceCodec::identify_by_content(file.path())
            .unwrap_or_else(|| panic!("Should identify MP4 (H264 fallback)"));
        assert_eq!(codec, SourceCodec::H264);
        assert!(codec.is_extension_compatible("mp4"));
        assert!(codec.is_extension_compatible("mov"));
    }

    #[test]
    fn test_identify_heic() {
        let file = create_temp_with_content(&[
            0x00, 0x00, 0x00, 0x1C, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c',
        ]);
        let codec = SourceCodec::identify_by_content(file.path())
            .unwrap_or_else(|| panic!("Should identify HEIC"));
        assert_eq!(codec, SourceCodec::Heic);
        assert!(codec.is_extension_compatible("heic"));
    }

    #[test]
    fn test_identify_mkv() {
        let file = create_temp_with_content(&[0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00, 0x00, 0x00]);
        let codec = SourceCodec::identify_by_content(file.path())
            .unwrap_or_else(|| panic!("Should identify EBML/MKV"));
        assert_eq!(codec, SourceCodec::Av1); // MKV catch-all
        assert!(codec.is_extension_compatible("mkv"));
        assert!(codec.is_extension_compatible("webm"));
    }

    #[test]
    fn test_mismatch_extension_correction() {
        // Create a PNG file but name it .jpg
        let temp_dir =
            tempfile::tempdir().unwrap_or_else(|e| panic!("Failed to create temp dir: {e:?}"));
        let png_as_jpg = temp_dir.path().join("image.jpg");
        {
            let mut file = std::fs::File::create(&png_as_jpg)
                .unwrap_or_else(|e| panic!("Failed to create file: {e:?}"));
            file.write_all(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
                .unwrap_or_else(|e| panic!("Failed to write PNG header: {e:?}"));
        }

        let fixed_path = crate::smart_file_copier::fix_extension_if_mismatch(&png_as_jpg)
            .unwrap_or_else(|e| panic!("Should fix extension: {e:?}"));
        assert_eq!(
            fixed_path
                .extension()
                .unwrap_or_else(|| panic!("missing extension"))
                .to_string_lossy()
                .to_lowercase(),
            "png"
        );
        assert!(fixed_path.exists());
        assert!(!png_as_jpg.exists());
    }
}
