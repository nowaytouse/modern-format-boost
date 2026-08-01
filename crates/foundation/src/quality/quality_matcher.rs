//! Quality Matcher Module
//!
//! Unified quality matching algorithm for all `modern_format_boost` tools.
//! Calculates optimal encoding parameters (CRF/distance) based on input quality
//! analysis.

#[cfg(feature = "high-precision")]
use rug::Rational;
use serde::{Deserialize, Serialize};

use crate::media_precision::{BitDepthMetadata, MediaPrecision};

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
    /// Relative encoding efficiency vs. H.264 (1.0). Lower value = more
    /// efficient at same quality. H.265/HEVC ≈ 0.65 and AV1 ≈ 0.50 are
    /// empirical from bitrate comparison studies; no single
    /// canonical reference—values tuned for CRF mapping consistency across
    /// codecs.
    #[must_use]
    pub const fn efficiency_factor(&self) -> f64 {
        match self {
            Self::Av1 => crate::constants::EFF_RATIO_AV1,
            Self::Vp9 => crate::constants::EFF_RATIO_VP9,
            Self::Vp8 => crate::constants::EFF_RATIO_VP8,
            Self::Vvc | Self::Av2 => crate::constants::EFF_RATIO_VVC,
            Self::H265 | Self::Heic => crate::constants::EFF_RATIO_HEVC,

            Self::Mpeg4 => crate::constants::EFF_RATIO_MPEG4,
            Self::Mpeg1 | Self::Mjpeg => crate::constants::EFF_RATIO_MJPEG,
            Self::Wmv => crate::constants::EFF_RATIO_WMV,
            Self::Theora | Self::Tiff => crate::constants::EFF_RATIO_THEORA,
            Self::RealVideo => crate::constants::EFF_RATIO_REALVIDEO,
            Self::FlashVideo | Self::Png => crate::constants::EFF_RATIO_FLASH,
            Self::Mpeg2 | Self::ProRes | Self::DnxHD | Self::Apng => {
                crate::constants::EFF_RATIO_MPEG2
            }

            Self::Gif | Self::Bmp => crate::constants::EFF_RATIO_GIF,
            Self::WebpAnimated => crate::constants::EFF_RATIO_WEBP_ANIM,
            Self::JpegXl => crate::constants::EFF_RATIO_JXL,
            Self::WebpStatic => crate::constants::EFF_RATIO_WEBP_STATIC,
            Self::Avif => crate::constants::EFF_RATIO_AVIF,

            Self::H264
            | Self::Ffv1
            | Self::UtVideo
            | Self::HuffYuv
            | Self::RawVideo
            | Self::Lagarith
            | Self::MagicYuv
            | Self::Jpeg
            | Self::Unknown => crate::constants::EFF_RATIO_H264,
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

    /// Returns true if the format is known to support animation (GIF, APNG,
    /// WebP, AVIF, HEIC, JXL).
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
        crate::constants::IMAGE_EXTENSIONS
    }

    /// Image extensions that should be collected for conversion (excludes
    /// JXL/AVIF/HEIC depending on tool).
    #[must_use]
    pub const fn image_extensions_for_convert() -> &'static [&'static str] {
        crate::constants::IMAGE_EXTENSIONS
    }

    /// Video extensions supported by the pipeline.
    #[must_use]
    pub const fn supported_video_extensions() -> &'static [&'static str] {
        crate::constants::VIDEO_EXTENSIONS
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
            | Self::Mpeg4 => crate::constants::EXT_MP4,
            Self::Mpeg2 | Self::Mpeg1 => "mpg",
            Self::Wmv => "wmv",
            Self::Theora => "ogv",
            Self::RealVideo => "rm",
            Self::FlashVideo => "flv",
            Self::ProRes | Self::DnxHD => crate::constants::EXT_MOV,
            Self::Ffv1
            | Self::UtVideo
            | Self::HuffYuv
            | Self::RawVideo
            | Self::Lagarith
            | Self::MagicYuv => crate::constants::EXT_MKV,
            Self::Gif => crate::constants::EXT_GIF,
            Self::Apng => crate::constants::EXT_APNG,
            Self::WebpAnimated | Self::WebpStatic => crate::constants::EXT_WEBP,
            Self::Mjpeg | Self::Jpeg => crate::constants::EXT_JPG,
            Self::JpegXl => crate::constants::EXT_JXL,
            Self::Png => crate::constants::EXT_PNG,
            Self::Avif => crate::constants::EXT_AVIF,
            Self::Heic => crate::constants::EXT_HEIC,
            Self::Bmp => crate::constants::EXT_BMP,
            Self::Tiff => crate::constants::EXT_TIFF,
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
            Self::ProRes
            | Self::DnxHD
            | Self::Ffv1
            | Self::UtVideo
            | Self::HuffYuv
            | Self::RawVideo
            | Self::Lagarith
            | Self::MagicYuv
            | Self::Theora
            | Self::RealVideo
            | Self::FlashVideo
            | Self::Unknown => true, // Relaxed for other video formats.
        }
    }

    /// Identifies the file format based on internal magic bytes.
    /// This is the "Tight Entry" mechanism that avoids relying on file
    /// extensions.
    pub fn identify_by_content(path: &std::path::Path) -> std::io::Result<Option<Self>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path).map_err(|e| {
            crate::media_conversion_gate::probe_quality_layer_audit(
                "quality_matcher_open_failed",
                path,
                format!("failed to open for content identification: {e}"),
            );
            std::io::Error::new(
                e.kind(),
                format!(
                    "failed to open {} for content identification: {e}",
                    path.display()
                ),
            )
        })?;
        let mut header = [0u8; 64]; // Expanded to 64 bytes to capture VP8X and acTL chunks
        let n = file.read(&mut header).map_err(|e| {
            crate::media_conversion_gate::probe_quality_layer_audit(
                "quality_matcher_header_read_failed",
                path,
                format!("failed to read header for content identification: {e}"),
            );
            std::io::Error::new(
                e.kind(),
                format!(
                    "failed to read {} for content identification: {e}",
                    path.display()
                ),
            )
        })?;
        if n < 2 {
            return Ok(None);
        }

        let Some(header_slice) = header.get(..n) else {
            crate::media_conversion_gate::probe_quality_layer_audit(
                "quality_matcher_header_slice_failed",
                path,
                format!("failed to slice header for identification (n={n})"),
            );
            return Ok(None);
        };

        let mut codec = Self::identify_by_header(header_slice);

        // Deep WebP animation verification
        // Some WebP files (notably Safari exports) may not place `VP8X` within the
        // first 64 bytes even when the file is animated, causing false
        // `WebpStatic` classification. We scan a bounded prefix for
        // `ANIM`/`ANMF` markers as a fast, reliable fallback.
        if codec == Some(Self::WebpStatic)
            && header.starts_with(b"RIFF")
            && n >= 12
            && header.get(8..12) == Some(b"WEBP")
        {
            const SCAN_LIMIT: usize = crate::constants::QUALITY_MATCHER_SCAN_LIMIT;
            let mut buf = Vec::with_capacity(SCAN_LIMIT);
            buf.extend_from_slice(header_slice);

            let remaining = SCAN_LIMIT.saturating_sub(n);
            if remaining > 0 {
                let mut extra = vec![0u8; remaining];
                let read_n = file.read(&mut extra).map_err(|e| {
                    crate::media_conversion_gate::probe_quality_layer_audit(
                        "quality_matcher_webp_scan_read_failed",
                        path,
                        format!("failed to read extra WEBP scan buffer: {e}"),
                    );
                    e
                })?;
                let Some(extra_slice) = extra.get(..read_n) else {
                    crate::media_conversion_gate::probe_quality_layer_audit(
                        "quality_matcher_webp_scan_slice_failed",
                        path,
                        format!("failed to slice extra WEBP scan buffer (read_n={read_n})"),
                    );
                    return Ok(codec);
                };
                buf.extend_from_slice(extra_slice);
            }

            if buf.windows(4).any(|w| w == b"ANIM") || buf.windows(4).any(|w| w == b"ANMF") {
                codec = Some(Self::WebpAnimated);
            }
        }

        // Deep APNG verification
        // 64 bytes is insufficient for PNG because large chunks (like iCCP or eXIf)
        // can push the acTL chunk far beyond the header. We use Seek to jump over chunk
        // data.
        if codec == Some(Self::Png) {
            file.seek(SeekFrom::Start(8)).map_err(|e| {
                crate::media_conversion_gate::probe_quality_layer_audit(
                    "quality_matcher_apng_seek_failed",
                    path,
                    format!("failed to seek for APNG chunk scan: {e}"),
                );
                e
            })?;
            let mut chunk_header = [0u8; 8];
            loop {
                if let Err(e) = file.read_exact(&mut chunk_header) {
                    if e.kind() != std::io::ErrorKind::UnexpectedEof {
                        crate::media_conversion_gate::probe_quality_layer_audit(
                            "quality_matcher_apng_chunk_read_failed",
                            path,
                            format!("failed to read APNG chunk header: {e}"),
                        );
                        return Err(e);
                    }
                    break;
                }
                let b1 = if let Some(b) = chunk_header.first() {
                    *b
                } else {
                    crate::log_corruption!(
                        crate::infra::static_logs::messages::LABEL_ANOMALY,
                        &format!(
                            "APNG CORRUPTION AUDIT: Chunk header missing byte 0 at position {:?} \
                             | Forensic: Unexpected EOF during animation traversal; breaking scan",
                            file.stream_position()
                        )
                    );
                    break;
                };
                let b2 = if let Some(b) = chunk_header.get(1) {
                    *b
                } else {
                    crate::log_corruption!(
                        crate::infra::static_logs::messages::LABEL_ANOMALY,
                        "APNG CORRUPTION AUDIT: Chunk header missing byte 1 | Forensic: Truncated \
                         bitstream; breaking scan"
                    );
                    break;
                };
                let b3 = if let Some(b) = chunk_header.get(2) {
                    *b
                } else {
                    crate::log_corruption!(
                        crate::infra::static_logs::messages::LABEL_ANOMALY,
                        "APNG CORRUPTION AUDIT: Chunk header missing byte 2 | Forensic: Truncated \
                         bitstream; breaking scan"
                    );
                    break;
                };
                let b4 = if let Some(b) = chunk_header.get(3) {
                    *b
                } else {
                    crate::log_corruption!(
                        crate::infra::static_logs::messages::LABEL_ANOMALY,
                        "APNG CORRUPTION AUDIT: Chunk header missing byte 3 | Forensic: Truncated \
                         bitstream; breaking scan"
                    );
                    break;
                };
                let length = u32::from_be_bytes([b1, b2, b3, b4]);
                let Some(chunk_type) = chunk_header.get(4..8) else {
                    crate::media_conversion_gate::probe_quality_layer_audit(
                        "quality_matcher_apng_chunk_type_missing",
                        path,
                        format!(
                            "APNG chunk type missing at position {:?}; terminating animation \
                             search",
                            file.stream_position()
                        ),
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
                file.seek(SeekFrom::Current(i64::from(length) + 4))
                    .map_err(|e| {
                        crate::media_conversion_gate::probe_quality_layer_audit(
                            "quality_matcher_apng_chunk_seek_failed",
                            path,
                            format!("failed to seek past APNG chunk payload length {length}: {e}"),
                        );
                        e
                    })?;
            }
        }

        Ok(codec)
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
            // Check for APNG acTL chunk which usually follows immediately after IHDR (byte
            // 33 starts the second chunk)
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
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "quality_matcher_riff_brand_missing",
                    "RIFF container missing brand field",
                );
                return None;
            };
            if brand == b"WEBP" {
                // Check for VP8X extended header which contains the animation flag
                if header.len() >= 21 && header.get(12..16) == Some(b"VP8X") {
                    // The animation flag is the 2nd bit of the flags byte at offset 20
                    let Some(flags) = crate::media_conversion_gate::probe_webp_vp8x_flags_optional(
                        header.get(20).copied(),
                    ) else {
                        return Some(Self::WebpStatic);
                    };
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
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "quality_matcher_isobmff_brand_missing",
                    "ISOBMFF header missing brand field",
                );
                return None;
            };
            match brand {
                b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1" => {
                    return Some(Self::Heic);
                }
                b"avif" | b"avis" => return Some(Self::Avif),
                b"isom" | b"mp41" | b"mp42" | b"piso" | b"mp4v" | b"3gp4" | b"3gp5" | b"3g2a" => {
                    return Some(Self::H264);
                }
                _ => {
                    // Refusing to assume H264 for unknown MP4/MOV brands.
                    // Information invalidated to prevent false quality matching.
                    crate::media_conversion_gate::probe_quality_batch_audit(
                        "quality_matcher_isobmff_unknown_brand",
                        format!(
                            "unknown ISOBMFF brand '{}'; refusing to forge codec",
                            String::from_utf8_lossy(brand)
                        ),
                    );
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
    pub bit_depth: Option<u8>,
    pub bit_depth_inferred_from_pix_fmt: bool,
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
            Self::Animation => crate::constants::QUALITY_MATCHER_CRF_ADJ_ANIMATION,
            Self::ScreenRecording => crate::constants::QUALITY_MATCHER_CRF_ADJ_SCREEN,
            Self::LiveAction | Self::Unknown => 0,
            Self::Gaming => crate::constants::QUALITY_MATCHER_CRF_ADJ_GAMING,
            Self::FilmGrain => crate::constants::QUALITY_MATCHER_CRF_ADJ_GRAIN,
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
            bit_depth: None,
            bit_depth_inferred_from_pix_fmt: false,
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

impl MediaPrecision for QualityAnalysis {
    fn bit_depth_metadata(&self) -> BitDepthMetadata {
        BitDepthMetadata::new(self.bit_depth, self.bit_depth_inferred_from_pix_fmt)
    }

    fn has_hdr_signaling(&self) -> bool {
        crate::media_conversion_gate::probe_bool_or_false(
            self.is_hdr,
            "quality_hdr_signaling_absent",
            "QualityAnalysis::has_hdr_signaling",
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedQuality {
    pub crf: f32,
    pub distance: f32,
    pub effective_bpp: f64,
    pub analysis_details: AnalysisDetails,
}

impl AnalysisDetails {
    fn seal_algorithm_outputs(&mut self) {
        if let Some(v) = crate::algorithm_seal::seal_non_negative_finite(self.raw_bpp) {
            self.raw_bpp = v;
        }
        seal_matcher_factor_field(&mut self.codec_factor);
        seal_matcher_factor_field(&mut self.resolution_factor);
        seal_matcher_factor_field(&mut self.alpha_factor);
        seal_matcher_factor_field(&mut self.color_depth_factor);
        seal_matcher_factor_field(&mut self.gop_factor);
        seal_matcher_factor_field(&mut self.chroma_factor);
        seal_matcher_factor_field(&mut self.hdr_factor);
        seal_matcher_factor_field(&mut self.aspect_factor);
        seal_matcher_factor_field(&mut self.complexity_factor);
        seal_matcher_factor_field(&mut self.grain_factor);
        seal_matcher_factor_field(&mut self.bframe_factor);
        seal_matcher_factor_field(&mut self.fps_factor);
        seal_matcher_factor_field(&mut self.duration_factor);
        self.confidence = self
            .confidence
            .and_then(crate::algorithm_seal::seal_unit_probability);
    }
}

#[inline]
fn seal_matcher_factor_field(value: &mut f64) {
    if let Some(v) = crate::algorithm_seal::seal_non_negative_finite(*value) {
        *value = v;
    }
}

impl MatchedQuality {
    /// Sanitize matcher outputs before they feed `VideoExplorer` or encoder
    /// setpoints.
    pub fn seal_algorithm_outputs(&mut self) {
        if let Some(v) = crate::algorithm_seal::seal_non_negative_finite(self.effective_bpp) {
            self.effective_bpp = v;
        }
        self.analysis_details.seal_algorithm_outputs();
        if self.crf.is_finite() {
            self.crf = crate::video_explorer::precision::seal_exploration_crf(self.crf);
        } else {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "quality_matcher",
                branch = "crf_non_finite_rejected",
                "CRF matcher produced non-finite crf; value poisoned"
            );
            self.crf = f32::NAN;
        }
        if !self.distance.is_finite() {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "quality_matcher",
                branch = "distance_non_finite",
                "JXL matcher produced non-finite distance; clamping to floor"
            );
            self.distance = crate::constants::JXL_MIN_DISTANCE;
        }
    }

    fn sealed(mut self) -> Self {
        self.seal_algorithm_outputs();
        self
    }

    /// Public terminal contract for matcher outputs.
    #[must_use]
    pub fn into_sealed(self) -> Self {
        self.sealed()
    }

    #[inline]
    #[must_use]
    pub fn crf_hevc_typed(&self) -> Option<crate::types::Crf<crate::types::HevcEncoder>> {
        match crate::types::Crf::<crate::types::HevcEncoder>::new(self.crf) {
            Ok(crf) => Some(crf),
            Err(e) => {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "quality_matcher_crf",
                    format!("invalid HEVC CRF {}: {e}; returning None", self.crf),
                );
                None
            }
        }
    }

    #[inline]
    #[must_use]
    pub fn crf_av1_typed(&self) -> Option<crate::types::Crf<crate::types::Av1Encoder>> {
        match crate::types::Crf::<crate::types::Av1Encoder>::new(self.crf) {
            Ok(crf) => Some(crf),
            Err(e) => {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "quality_matcher_crf",
                    format!("invalid AV1 CRF {}: {e}; returning None", self.crf),
                );
                None
            }
        }
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

    pub confidence: Option<f64>,
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
            confidence: None,
            match_mode: MatchMode::Quality,
            quality_bias: QualityBias::Balanced,
        }
    }
}

/// Safe BPP range for CRF formula: avoids log2(0), NaN, and overflow. Final CRF
/// is still clamped to [0, 51] for maximum flexibility.
const SAFE_BPP_MIN: f64 = crate::constants::SAFE_BPP_MIN;
const SAFE_BPP_MAX: f64 = crate::constants::SAFE_BPP_MAX;

/// AV1 CRF output range; final clamp is the last line of defense for extreme
/// BPP or content/bias adjustments.
const AV1_CRF_CLAMP_MIN: f32 = crate::constants::AV1_CRF_CLAMP_MIN;
const AV1_CRF_CLAMP_MAX: f32 = crate::constants::AV1_CRF_CLAMP_MAX;

/// HEVC CRF output range (x265 0–51, we use 0–51 to allow full range in
/// ultimate mode).
const HEVC_CRF_CLAMP_MIN: f32 = crate::constants::HEVC_CRF_CLAMP_MIN;
const HEVC_CRF_CLAMP_MAX: f32 = crate::constants::HEVC_CRF_CLAMP_MAX;

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
        let conf_str = crate::media_conversion_gate::ui_confidence_pct_whole_or_na(
            details.confidence,
            "quality_matcher_effective_bpp",
        );
        return Err(format!(
            "{}\n   {} Possible causes:
- File size is 0 or unknown\n- video_bitrate not provided\n- Duration/fps detection failed\n- \
             Invalid dimensions\n{} Confidence: {conf_str}",
            crate::media_conversion_gate::ui_user_facing_error(format!(
                "Cannot calculate AV1 CRF: effective_bpp is {effective_bpp} (must be > 0)"
            )),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]"),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]")
        ));
    }
    if !effective_bpp.is_finite() {
        let conf_str = crate::media_conversion_gate::ui_confidence_pct_whole_or_na(
            details.confidence,
            "quality_matcher_effective_bpp",
        );
        return Err(format!(
            "{}\n   {} Confidence: {conf_str}",
            crate::media_conversion_gate::ui_user_facing_error(
                "Cannot calculate AV1 CRF: effective_bpp is non-finite (NaN/Inf)",
            ),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]")
        ));
    }
    // Defensive clamp so formula inputs are always in a safe range; final CRF clamp
    // [15, 40] remains the safeguard.
    effective_bpp = effective_bpp.clamp(SAFE_BPP_MIN, SAFE_BPP_MAX);

    let crf_float = if effective_bpp < crate::constants::BPP_LOW_GATE_HEVC {
        crate::constants::CRF_EST_H26X_MAX.min(
            crate::constants::CRF_EST_H26X_SLOPE.mul_add(
                -(effective_bpp * crate::constants::PERCENTAGE_FACTOR)
                    .max(crate::constants::LOG2_SAFETY_FLOOR)
                    .log2(),
                crate::constants::CRF_EST_H26X_INTERCEPT,
            ),
        )
    } else if effective_bpp > crate::constants::BPP_UPPER_CAP {
        crate::constants::CRF_EST_H26X_MIN.max(crate::constants::CRF_EST_H26X_SLOPE.mul_add(
            -(effective_bpp * crate::constants::PERCENTAGE_FACTOR).log2(),
            crate::constants::CRF_EST_H26X_INTERCEPT,
        ))
    } else {
        crate::constants::CRF_EST_H26X_SLOPE.mul_add(
            -(effective_bpp * crate::constants::PERCENTAGE_FACTOR).log2(),
            crate::constants::CRF_EST_H26X_INTERCEPT,
        )
    };

    let crf_with_content = crf_float + f64::from(details.content_type_adjustment);

    let crf_with_bias = match bias {
        QualityBias::Conservative => crf_with_content + crate::constants::MATCHER_BIAS_CONSERVATIVE,
        QualityBias::Balanced => crf_with_content,
        QualityBias::Aggressive => crf_with_content + crate::constants::MATCHER_BIAS_AGGRESSIVE,
    };

    let rf = crate::constants::MATCHER_CRF_ROUNDING_FACTOR;
    let crf_rounded = (crf_with_bias * rf).round() / rf;
    // Last line of defense: guarantee CRF in valid range regardless of extreme BPP
    // or content/bias.
    let crf = (crate::numeric_cast::f64_to_f32_lossy(crf_rounded))
        .clamp(AV1_CRF_CLAMP_MIN, AV1_CRF_CLAMP_MAX);

    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    }
    .sealed())
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
        let conf_str = crate::media_conversion_gate::ui_confidence_pct_whole_or_na(
            details.confidence,
            "quality_matcher_effective_bpp",
        );
        return Err(format!(
            "{}\n   {} Possible causes:
- File size is 0 or unknown\n- video_bitrate not provided\n- Duration/fps detection failed\n- \
             Invalid dimensions\n{} Confidence: {conf_str}",
            crate::media_conversion_gate::ui_user_facing_error(format!(
                "Cannot calculate HEVC CRF: effective_bpp is {effective_bpp} (must be > 0)"
            )),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]"),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]")
        ));
    }
    if !effective_bpp.is_finite() {
        let conf_str = crate::media_conversion_gate::ui_confidence_pct_whole_or_na(
            details.confidence,
            "quality_matcher_effective_bpp",
        );
        return Err(format!(
            "{}\n   {} Confidence: {conf_str}",
            crate::media_conversion_gate::ui_user_facing_error(
                "Cannot calculate HEVC CRF: effective_bpp is non-finite (NaN/Inf)",
            ),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]")
        ));
    }
    effective_bpp = effective_bpp.clamp(SAFE_BPP_MIN, SAFE_BPP_MAX);

    let crf_float = if effective_bpp < crate::constants::BPP_LOW_GATE_AV1 {
        crate::constants::CRF_EST_AV1_MAX.min(
            crate::constants::CRF_EST_AV1_SLOPE.mul_add(
                -(effective_bpp * crate::constants::PERCENTAGE_FACTOR)
                    .max(crate::constants::LOG2_SAFETY_FLOOR)
                    .log2(),
                crate::constants::CRF_EST_AV1_INTERCEPT,
            ),
        )
    } else if effective_bpp > crate::constants::BPP_UPPER_CAP {
        crate::constants::CRF_EST_AV1_MIN.max(crate::constants::CRF_EST_AV1_SLOPE.mul_add(
            -(effective_bpp * crate::constants::PERCENTAGE_FACTOR).log2(),
            crate::constants::CRF_EST_AV1_INTERCEPT,
        ))
    } else {
        crate::constants::CRF_EST_AV1_SLOPE.mul_add(
            -(effective_bpp * crate::constants::PERCENTAGE_FACTOR).log2(),
            crate::constants::CRF_EST_AV1_INTERCEPT,
        )
    };

    let crf_with_content = crf_float + f64::from(details.content_type_adjustment);

    let crf_with_bias = match bias {
        QualityBias::Conservative => crf_with_content + crate::constants::MATCHER_BIAS_CONSERVATIVE,
        QualityBias::Balanced => crf_with_content,
        QualityBias::Aggressive => crf_with_content + crate::constants::MATCHER_BIAS_AGGRESSIVE,
    };

    let rf = crate::constants::MATCHER_CRF_ROUNDING_FACTOR;
    let crf_rounded = (crf_with_bias * rf).round() / rf;
    let crf = (crate::numeric_cast::f64_to_f32_lossy(crf_rounded))
        .clamp(HEVC_CRF_CLAMP_MIN, HEVC_CRF_CLAMP_MAX);

    Ok(MatchedQuality {
        crf,
        distance: 0.0,
        effective_bpp,
        analysis_details: details,
    }
    .sealed())
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
        let base_distance =
            (crate::numeric_cast::f64_to_f32_lossy(crate::constants::PERCENTAGE_FACTOR)
                - f32::from(quality))
                / crate::constants::JXL_QUALITY_MAP_DIVISOR;

        let biased_distance = match bias {
            QualityBias::Conservative => {
                base_distance + crate::constants::JXL_QUALITY_BIAS_CONSERVATIVE
            }
            QualityBias::Balanced => base_distance,
            QualityBias::Aggressive => {
                base_distance + crate::constants::JXL_QUALITY_BIAS_AGGRESSIVE
            }
        };

        let clamped = biased_distance.clamp(
            crate::constants::JXL_MIN_DISTANCE,
            crate::constants::JXL_MAX_DISTANCE,
        );

        return Ok(MatchedQuality {
            // crf is unused on the JXL path — `distance` is the encoder knob. 0.0 is a
            // placeholder, not a derived CRF; do not consume it downstream.
            crf: 0.0,
            distance: clamped,
            effective_bpp: analysis.bpp,
            analysis_details: AnalysisDetails {
                confidence: Some(calculate_confidence_v3(analysis)),
                match_mode: mode,
                quality_bias: bias,
                ..Default::default()
            },
        }
        .sealed());
    }

    let (effective_bpp, details) =
        calculate_effective_bpp_with_options(analysis, EncoderType::Jxl, mode, bias)?;

    if effective_bpp <= 0.0_f64 {
        let conf_str = crate::media_conversion_gate::ui_confidence_pct_whole_or_na(
            details.confidence,
            "quality_matcher_effective_bpp",
        );
        return Err(format!(
            "{}\n   {} Possible causes:
- File size is 0 or unknown\n- Invalid dimensions\n{} For JPEG sources, ensure JPEG quality \
             analysis is available\n{} Confidence: {conf_str}",
            crate::media_conversion_gate::ui_user_facing_error(format!(
                "Cannot calculate JXL distance: effective_bpp is {effective_bpp} (must be > 0)"
            )),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]"),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]"),
            crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]")
        ));
    }

    let estimated_quality = crate::constants::JXL_QUAL_EST_SLOPE.mul_add(
        (effective_bpp * crate::constants::JXL_QUAL_EST_BPP_SCALE)
            .max(crate::constants::LOG2_SAFETY_FLOOR)
            .log2(),
        crate::constants::JXL_QUAL_EST_INTERCEPT,
    );

    let clamped_quality = estimated_quality.clamp(50.0, crate::constants::PERCENTAGE_FACTOR);
    let base_distance = crate::numeric_cast::f64_to_f32_lossy(
        (crate::constants::PERCENTAGE_FACTOR - clamped_quality)
            / f64::from(crate::constants::JXL_QUALITY_MAP_DIVISOR),
    );

    let content_adj =
        f32::from(details.content_type_adjustment) * crate::constants::JXL_CONTENT_ADJ_SCALE;
    let distance_with_content = base_distance - content_adj;

    let distance_with_bias = match bias {
        QualityBias::Conservative => {
            distance_with_content + crate::constants::JXL_QUALITY_BIAS_CONSERVATIVE
        }
        QualityBias::Balanced => distance_with_content,
        QualityBias::Aggressive => {
            distance_with_content + crate::constants::JXL_QUALITY_BIAS_AGGRESSIVE
        }
    };

    let clamped_distance = distance_with_bias.clamp(
        crate::constants::JXL_MIN_DISTANCE,
        crate::constants::JXL_MAX_DISTANCE,
    );

    Ok(MatchedQuality {
        crf: 0.0,
        distance: clamped_distance,
        effective_bpp,
        analysis_details: details,
    }
    .sealed())
}

/// Calculate effective bits per pixel with advanced options.
///
/// # Errors
/// Returns an error message if calculation fails (e.g., missing or invalid
/// dimensions).
pub fn calculate_effective_bpp_with_options(
    analysis: &QualityAnalysis,
    target_encoder: EncoderType,
    mode: MatchMode,
    bias: QualityBias,
) -> Result<(f64, AnalysisDetails), String> {
    if analysis.width == 0 || analysis.height == 0 {
        return Err(crate::media_conversion_gate::ui_user_facing_error(
            "Invalid dimensions: width or height is 0",
        ));
    }

    let pixels = u64::from(analysis.width) * u64::from(analysis.height);

    let raw_bpp = calculate_raw_bpp(analysis, pixels)?;

    let source_codec = parse_source_codec(&analysis.source_codec);
    let codec_factor = calculate_codec_efficiency(source_codec, analysis.encoder_preset.as_deref());

    let gop_factor = calculate_gop_factor(analysis.gop_size, analysis.b_frame_count);

    let chroma_factor = calculate_chroma_factor(analysis.pix_fmt.as_deref())?;

    let hdr_factor = calculate_hdr_factor(analysis.is_hdr, analysis.color_space.as_deref());

    let content_type_adjustment =
        content_type_for_crf_analysis(analysis.content_type).crf_adjustment();

    let resolution_factor = calculate_resolution_factor(pixels);

    let alpha_factor = if analysis.has_alpha {
        crate::constants::ALPHA_FACTOR_TRUE
    } else {
        1.0_f64
    };

    let color_depth_factor =
        if analysis.bit_depth_inferred_from_pix_fmt && analysis.confirmed_bit_depth().is_none() {
            1.0_f64
        } else {
            calculate_color_depth_factor(crf_effective_bit_depth(analysis), source_codec)?
        };

    let aspect_factor = calculate_aspect_factor(analysis.width, analysis.height);

    let complexity_factor = calculate_complexity_factor(
        analysis.spatial_complexity,
        analysis.temporal_complexity,
        raw_bpp,
        pixels,
    );

    let grain_factor = if analysis.has_film_grain == Some(true) {
        crate::constants::GRAIN_FACTOR_TRUE
    } else {
        1.0_f64
    };

    let target_adjustment = match target_encoder {
        EncoderType::Av1 => crate::constants::TARGET_ENCODER_AV1_FACTOR,
        EncoderType::Hevc => crate::constants::TARGET_ENCODER_HEVC_FACTOR,
        EncoderType::Jxl => crate::constants::TARGET_ENCODER_JXL_FACTOR,
    };

    let mode_adjustment = match mode {
        MatchMode::Quality => crate::constants::MATCH_MODE_QUALITY_FACTOR,
        MatchMode::Size => crate::constants::MATCH_MODE_SIZE_FACTOR,
        MatchMode::Speed => crate::constants::MATCH_MODE_SPEED_FACTOR,
    };

    let effective_bpp = {
        #[cfg(feature = "high-precision")]
        {
            use crate::numeric_cast::f64_to_rational_strict;
            let mut res = f64_to_rational_strict(raw_bpp, "raw_bpp").ok_or("Invalid raw_bpp")?;
            res *= f64_to_rational_strict(gop_factor, "gop_factor")
                .ok_or("gop_factor is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(chroma_factor, "chroma_factor")
                .ok_or("chroma_factor is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(hdr_factor, "hdr_factor")
                .ok_or("hdr_factor is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(aspect_factor, "aspect_factor")
                .ok_or("aspect_factor is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(complexity_factor, "complexity_factor")
                .ok_or("complexity_factor is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(grain_factor, "grain_factor")
                .ok_or("grain_factor is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(mode_adjustment, "mode_adjustment")
                .ok_or("mode_adjustment is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(resolution_factor, "resolution_factor")
                .ok_or("resolution_factor is NaN/Inf — logic error in factor computation")?;
            res *= f64_to_rational_strict(alpha_factor, "alpha_factor")
                .ok_or("alpha_factor is NaN/Inf — logic error in factor computation")?;
            res /= f64_to_rational_strict(codec_factor, "codec_factor")
                .ok_or("codec_factor is NaN/Inf — logic error in factor computation")?;
            res /= f64_to_rational_strict(color_depth_factor, "color_depth_factor")
                .ok_or("color_depth_factor is NaN/Inf — logic error in factor computation")?;
            res /= f64_to_rational_strict(target_adjustment, "target_adjustment")
                .ok_or("target_adjustment is NaN/Inf — logic error in factor computation")?;
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
        confidence: Some(confidence),
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
        return Err(crate::media_conversion_gate::ui_user_facing_error(
            "Cannot calculate bpp: pixels is 0 (invalid dimensions)",
        ));
    }

    if let Some(video_bitrate) = analysis.video_bitrate
        && video_bitrate > 0
        && let Some(fps) = analysis.fps
        && fps > 0.0_f64
    {
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

    if analysis.file_size > 0 {
        if let Some(duration) = analysis.duration_secs
            && duration > 0.0_f64
        {
            let fps = analysis
                .fps
                .ok_or_else(|| "Missing FPS for BPP calculation".to_string())?;
            if fps <= 0.0_f64 {
                return Err(crate::media_conversion_gate::ui_user_facing_error(
                    "Cannot calculate bpp: FPS is 0 or negative",
                ));
            }
            let total_frames =
                crate::numeric_cast::f64_to_u64_strict(duration * fps, "total_frames").ok_or_else(
                    || {
                        crate::media_conversion_gate::ui_user_facing_error(
                            "Cannot calculate bpp: total_frames is invalid",
                        )
                    },
                )?;
            if total_frames == 0 {
                return Err(crate::media_conversion_gate::ui_user_facing_error(
                    "Cannot calculate bpp: total_frames is 0",
                ));
            }
            #[cfg(feature = "high-precision")]
            {
                let bits_per_frame = (Rational::from(analysis.file_size) * Rational::from(8_i32))
                    / Rational::from(total_frames);
                return Ok((bits_per_frame / Rational::from(pixels)).to_f64());
            }
            #[cfg(not(feature = "high-precision"))]
            {
                let bits_per_frame = (crate::numeric_cast::u64_to_f64(analysis.file_size)
                    * crate::constants::BITS_PER_BYTE)
                    / crate::numeric_cast::u64_to_f64(total_frames);
                return Ok(bits_per_frame / crate::numeric_cast::u64_to_f64(total_frames));
            }
        }

        let source_codec = parse_source_codec(&analysis.source_codec);
        if source_codec.is_video() || source_codec.can_be_animated() {
            return Err(crate::media_conversion_gate::ui_user_facing_error(format!(
                "Cannot calculate bpp for video/animated codec '{}': missing duration or bitrate",
                analysis.source_codec
            )));
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
            return Ok((crate::numeric_cast::u64_to_f64(analysis.file_size)
                * crate::constants::BITS_PER_BYTE)
                / crate::numeric_cast::u64_to_f64(pixels));
        }
    }

    Err(crate::media_conversion_gate::ui_user_facing_error(
        "Cannot calculate bpp: no video_bitrate, file_size, or bpp provided",
    ))
}

fn calculate_gop_factor(gop_size: Option<u32>, b_frames: Option<u8>) -> f64 {
    let gop_base = match gop_size {
        Some(1) => crate::constants::GOP_FACTOR_I_ONLY,
        Some(2..=10) => crate::constants::GOP_FACTOR_VERY_SHORT,
        Some(11..=50) | None => crate::constants::GOP_FACTOR_STANDARD,
        Some(51..=150) => crate::constants::GOP_FACTOR_LONG,
        Some(151..=300) => crate::constants::GOP_FACTOR_VERY_LONG,
        Some(_) => crate::constants::GOP_FACTOR_EXTREME,
    };

    let b_pyramid_bonus = match b_frames {
        Some(0) | None => {
            // Neutral factor: no bonus applied because metadata is unknown.
            // This is honest (no forgery) and doesn't block the workflow.
            1.0_f64
        }
        Some(1) => crate::constants::B_FRAME_BONUS_1,
        Some(2) => crate::constants::B_FRAME_BONUS_2,
        Some(3..) => crate::constants::B_FRAME_BONUS_MANY,
    };

    gop_base * b_pyramid_bonus
}

fn calculate_chroma_factor(pix_fmt: Option<&str>) -> Result<f64, String> {
    crate::media_conversion_gate::probe_chroma_factor_optional(
        pix_fmt,
        "quality_matcher chroma factor",
    )
    .ok_or_else(|| {
        "pix_fmt required for CRF effective_bpp estimate (refusing forged chroma factor)"
            .to_string()
    })
}

fn content_type_for_crf_analysis(content_type: Option<ContentType>) -> ContentType {
    crate::media_conversion_gate::quality_content_type_for_crf_or_unknown(content_type)
}

fn calculate_hdr_factor(is_hdr: Option<bool>, color_space: Option<&str>) -> f64 {
    if is_hdr == Some(true) {
        return crate::constants::HDR_ADJUSTMENT_FACTOR;
    }

    if let Some(cs) = color_space {
        let cs_lower = cs.to_lowercase();
        if cs_lower.contains("bt2020") || cs_lower.contains("2020") {
            return crate::constants::BT2020_ADJUSTMENT_FACTOR;
        }
    }

    1.0
}

fn calculate_codec_efficiency(codec: SourceCodec, preset: Option<&str>) -> f64 {
    let base_efficiency = codec.efficiency_factor();

    if let Some(p) = preset {
        let p_lower = p.to_lowercase();

        if p_lower.contains("placebo") {
            return base_efficiency * crate::constants::EFF_MULT_PLACEBO;
        } else if p_lower.contains("veryslow") {
            return base_efficiency * crate::constants::EFF_MULT_VERYSLOW;
        } else if p_lower.contains("slow") {
            return base_efficiency * crate::constants::EFF_MULT_SLOW;
        } else if p_lower.contains("veryfast") {
            return base_efficiency * crate::constants::EFF_MULT_VERYFAST;
        } else if p_lower.contains("fast") {
            return base_efficiency * crate::constants::EFF_MULT_FAST;
        } else if p_lower.contains("ultrafast") {
            return base_efficiency * crate::constants::EFF_MULT_ULTRAFAST;
        }

        match p.parse::<u8>() {
            Ok(preset_num) => {
                return match preset_num {
                    0..=2 => base_efficiency * crate::constants::EFF_MULT_PLACEBO,
                    3..=4 => base_efficiency * crate::constants::EFF_MULT_SLOW,
                    5..=6 => base_efficiency * crate::constants::EFF_MULT_MEDIUM,
                    7..=8 => base_efficiency * crate::constants::EFF_MULT_FAST,
                    9..=10 => base_efficiency * crate::constants::EFF_MULT_SUPERFAST,
                    _ => base_efficiency * crate::constants::EFF_MULT_ULTRAFAST,
                };
            }
            Err(e) => {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "quality_matcher_preset",
                    format!("failed to parse preset '{p}' as numeric tier: {e}"),
                );
            }
        }
    }

    base_efficiency
}

fn calculate_resolution_factor(pixels: u64) -> f64 {
    let megapixels = crate::numeric_cast::u64_to_f64(pixels) / crate::constants::MEGAPIXEL_FACTOR;
    if megapixels > crate::constants::RES_FACTOR_THRESHOLD_ULTRA_HD {
        crate::constants::RES_FACTOR_SLOPE.mul_add(
            (crate::constants::RES_FACTOR_THRESHOLD_ULTRA_HD / megapixels).min(1.0),
            crate::constants::RES_FACTOR_BASE_UHD,
        )
    } else if megapixels > crate::constants::RES_FACTOR_THRESHOLD_FULL_HD {
        crate::constants::RES_FACTOR_SLOPE.mul_add(
            (crate::constants::RES_FACTOR_THRESHOLD_ULTRA_HD - megapixels)
                / (crate::constants::RES_FACTOR_THRESHOLD_ULTRA_HD
                    - crate::constants::RES_FACTOR_THRESHOLD_FULL_HD),
            crate::constants::RES_FACTOR_BASE_FHD,
        )
    } else if megapixels > crate::constants::RES_FACTOR_THRESHOLD_SD {
        crate::constants::RES_FACTOR_SLOPE.mul_add(
            (crate::constants::RES_FACTOR_THRESHOLD_FULL_HD - megapixels)
                / (crate::constants::RES_FACTOR_THRESHOLD_FULL_HD
                    - crate::constants::RES_FACTOR_THRESHOLD_SD),
            crate::constants::RES_FACTOR_BASE_SD,
        )
    } else {
        crate::constants::RES_FACTOR_SLOPE.mul_add(
            ((crate::constants::RES_FACTOR_THRESHOLD_SD - megapixels)
                / crate::constants::RES_FACTOR_THRESHOLD_SD)
                .min(1.0),
            crate::constants::RES_FACTOR_BASE_THUMB,
        )
    }
}

/// Bit depth for CRF math: confirmed probe only; `#[cfg(test)]` supplies 8 when
/// fixtures omit it.
fn crf_effective_bit_depth(analysis: &QualityAnalysis) -> Option<u8> {
    if let Some(depth) = analysis.confirmed_bit_depth() {
        return Some(depth);
    }
    #[cfg(test)]
    if analysis.bit_depth.is_none() && !analysis.bit_depth_inferred_from_pix_fmt {
        return Some(8);
    }
    None
}

fn calculate_color_depth_factor(bit_depth: Option<u8>, codec: SourceCodec) -> Result<f64, String> {
    match bit_depth {
        Some(1..=8) if codec == SourceCodec::Gif => Ok(crate::constants::COLOR_DEPTH_FACTOR_GIF),
        Some(8) => Ok(1.0_f64),
        Some(10) => Ok(crate::constants::COLOR_DEPTH_FACTOR_10BIT),
        Some(12) => Ok(crate::constants::COLOR_DEPTH_FACTOR_12BIT),
        Some(16) => Ok(crate::constants::COLOR_DEPTH_FACTOR_16BIT),
        Some(other) => Err(format!(
            "unsupported bit_depth={other} for CRF effective_bpp (refusing neutral default)"
        )),
        None => {
            Err("bit_depth missing for CRF effective_bpp (refusing neutral default)".to_string())
        }
    }
}

fn calculate_aspect_factor(width: u32, height: u32) -> f64 {
    let aspect_ratio = f64::from(width) / f64::from(height.max(1));
    if aspect_ratio > crate::constants::ASPECT_RATIO_ULTRA_WIDE {
        crate::constants::ASPECT_FACTOR_EXTREME
    } else if aspect_ratio > crate::constants::ASPECT_RATIO_WIDE {
        crate::constants::ASPECT_FACTOR_MODERATE
    } else if aspect_ratio < crate::constants::ASPECT_RATIO_TALL {
        crate::constants::ASPECT_FACTOR_EXTREME
    } else {
        1.0
    }
}

fn calculate_complexity_factor(si: Option<f64>, ti: Option<f64>, raw_bpp: f64, pixels: u64) -> f64 {
    if let (Some(si_val), Some(temporal)) = (si, ti) {
        let si_ratio = si_val / crate::constants::SI_DIVISOR;
        let ti_ratio = temporal / crate::constants::TI_DIVISOR;

        let spatial_factor = if si_ratio > crate::constants::SI_RATIO_HIGH_THRESHOLD {
            crate::constants::SI_FACTOR_HIGH
        } else if si_ratio < crate::constants::SI_RATIO_LOW_THRESHOLD {
            crate::constants::SI_FACTOR_LOW
        } else {
            1.0_f64
        };

        let temporal_factor = if ti_ratio > crate::constants::TI_RATIO_HIGH_THRESHOLD {
            crate::constants::TI_FACTOR_HIGH
        } else if ti_ratio < crate::constants::TI_RATIO_LOW_THRESHOLD {
            crate::constants::TI_FACTOR_LOW
        } else {
            1.0_f64
        };

        return spatial_factor * temporal_factor;
    }

    let expected_bpp = if pixels > 8_000_000 {
        crate::constants::BPP_EXPECTED_UHD
    } else if pixels > 2_000_000 {
        crate::constants::BPP_EXPECTED_FHD
    } else if pixels > 500_000 {
        crate::constants::BPP_EXPECTED_SD
    } else {
        crate::constants::BPP_EXPECTED_THUMB
    };

    let ratio = raw_bpp / expected_bpp;
    if ratio > crate::constants::COMPLEXITY_RATIO_HIGH_THRESHOLD {
        crate::constants::COMPLEXITY_RATIO_MAX_FACTOR
    } else if ratio > 1.0 {
        crate::constants::COMPLEXITY_RATIO_SLOPE.mul_add(ratio - 1.0, 1.0)
    } else if ratio > crate::constants::COMPLEXITY_RATIO_LOW_THRESHOLD {
        1.0
    } else {
        crate::constants::COMPLEXITY_RATIO_MIN_FACTOR
    }
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
fn calculate_confidence_v3(analysis: &QualityAnalysis) -> f64 {
    let mut score: f64 = 0.0;
    let mut max_score: f64 = 0.0;

    max_score += crate::constants::CONF_W_DIMENSIONS;
    if analysis.width > 0 && analysis.height > 0 {
        score += crate::constants::CONF_W_DIMENSIONS;
    }

    max_score += crate::constants::CONF_W_FILE_SIZE;
    if analysis.file_size > 0 || analysis.video_bitrate.is_some() {
        score += crate::constants::CONF_W_FILE_SIZE;
    }

    max_score += crate::constants::CONF_W_BPP;
    if analysis.bpp > 0.0_f64 {
        score += crate::constants::CONF_W_BPP;
    }

    max_score += crate::constants::CONF_W_CODEC;
    let codec = parse_source_codec(&analysis.source_codec);
    if codec != SourceCodec::Unknown {
        score += crate::constants::CONF_W_CODEC;
    }

    max_score += crate::constants::CONF_W_BITRATE;
    if analysis.video_bitrate.is_some() {
        score += crate::constants::CONF_W_BITRATE;
    }

    max_score += crate::constants::CONF_W_GOP;
    if analysis.gop_size.is_some() {
        score += crate::constants::CONF_W_GOP;
    }

    max_score += crate::constants::CONF_W_B_FRAMES;
    if analysis.b_frame_count.is_some() {
        score += crate::constants::CONF_W_B_FRAMES;
    }

    max_score += crate::constants::CONF_W_PIX_FMT;
    if analysis.pix_fmt.is_some() {
        score += crate::constants::CONF_W_PIX_FMT;
    }

    max_score += crate::constants::CONF_W_COLOR;
    if analysis.is_hdr.is_some() || analysis.color_space.is_some() {
        score += crate::constants::CONF_W_COLOR;
    }

    max_score += crate::constants::CONF_W_CONTENT;
    if analysis.content_type.is_some() {
        score += crate::constants::CONF_W_CONTENT;
    }

    max_score += crate::constants::CONF_W_COMPLEXITY;
    if analysis.spatial_complexity.is_some() && analysis.temporal_complexity.is_some() {
        score += crate::constants::CONF_W_COMPLEXITY;
    }

    max_score += crate::constants::MATCHER_SCORE_TRUST_WEIGHT;
    if analysis.duration_secs.is_some() {
        score += crate::constants::MATCHER_SCORE_TRUST_WEIGHT;
    }

    max_score += crate::constants::MATCHER_SCORE_TRUST_WEIGHT;
    if analysis.fps.is_some() {
        score += crate::constants::MATCHER_SCORE_TRUST_WEIGHT;
    }

    max_score += crate::constants::MATCHER_SCORE_FORMAT_WEIGHT;
    if analysis.estimated_quality.is_some() {
        score += crate::constants::MATCHER_SCORE_FORMAT_WEIGHT;
    }

    max_score += crate::constants::MATCHER_SCORE_FORMAT_WEIGHT;
    if let Some(bd) = analysis.bit_depth.filter(|bd| *bd > 0) {
        // Unrecognized bit-depth values must not receive "known-feature" credit.
        // Even if downstream factors treat them neutrally, confidence weighting must be
        // fail-closed.
        let codec = parse_source_codec(&analysis.source_codec);
        let bit_depth_recognized = match codec {
            SourceCodec::Gif => (1..=8).contains(&bd) || matches!(bd, 10 | 12 | 16),
            _ => matches!(bd, 8 | 10 | 12 | 16),
        };
        if bit_depth_recognized {
            score += if analysis.bit_depth_inferred_from_pix_fmt {
                crate::constants::MATCHER_SCORE_FORMAT_WEIGHT / 2.0
            } else {
                crate::constants::MATCHER_SCORE_FORMAT_WEIGHT
            };
        }
    }

    if let (Some(fps), Some(duration)) = (analysis.fps, analysis.duration_secs)
        && fps > 0.0_f64
        && duration > 0.0_f64
        && (crate::constants::CONF_FPS_MIN..=crate::constants::CONF_FPS_MAX).contains(&fps)
    {
        score += crate::constants::MATCHER_SCORE_BITRATE_WEIGHT;
        max_score += crate::constants::MATCHER_SCORE_BITRATE_WEIGHT;
    }

    if let (Some(video_bitrate), Some(fps)) = (analysis.video_bitrate, analysis.fps) {
        let pixels = u64::from(analysis.width) * u64::from(analysis.height);
        if pixels > 0 && video_bitrate > 0 && fps > 0.0_f64 {
            // Use u64 throughout to prevent saturation at 4 Gbps (u32::MAX = ~4.3 Gbps)
            let bpp_estimate = crate::numeric_cast::u64_to_f64(video_bitrate)
                / (crate::numeric_cast::u64_to_f64(pixels) * fps);
            if (crate::constants::CONF_BPP_MIN..=crate::constants::CONF_BPP_MAX)
                .contains(&bpp_estimate)
            {
                score += crate::constants::MATCHER_SCORE_BITRATE_WEIGHT;
                max_score += crate::constants::MATCHER_SCORE_BITRATE_WEIGHT;
            }
        }
    }

    (score / max_score).clamp(0.0, 1.0)
}

fn parse_modern_codecs(codec_lower: &str) -> Option<SourceCodec> {
    if crate::constants::SIG_VVC
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::Vvc);
    }
    if crate::constants::SIG_AV2
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::Av2);
    }
    if crate::constants::SIG_AV1
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::Av1);
    }
    if crate::constants::SIG_HEVC
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::H265);
    }
    if crate::constants::SIG_VP9
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::Vp9);
    }
    if crate::constants::SIG_VP8
        .iter()
        .any(|&s| codec_lower.contains(s))
        || codec_lower == "libvpx"
    {
        return Some(SourceCodec::Vp8);
    }
    if crate::constants::SIG_H264
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::H264);
    }
    None
}

fn parse_legacy_codecs(codec_lower: &str) -> Option<SourceCodec> {
    if crate::constants::SIG_MPEG4
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::Mpeg4);
    }
    if crate::constants::SIG_MPEG2
        .iter()
        .any(|&s| codec_lower.contains(s))
        || codec_lower == "mpeg2video"
    {
        return Some(SourceCodec::Mpeg2);
    }
    if crate::constants::SIG_MPEG1
        .iter()
        .any(|&s| codec_lower.contains(s))
        || codec_lower == "mpeg1video"
    {
        return Some(SourceCodec::Mpeg1);
    }
    if crate::constants::SIG_WMV
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::Wmv);
    }
    if crate::constants::SIG_THEORA
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::Theora);
    }
    if crate::constants::SIG_REALVIDEO
        .iter()
        .any(|&s| codec_lower.contains(s))
    {
        return Some(SourceCodec::RealVideo);
    }
    if crate::constants::SIG_FLASH
        .iter()
        .any(|&s| codec_lower.contains(s))
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
    if codec_lower.contains("heic") || codec_lower.contains("heif") || codec_lower == "hif" {
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
    crate::log_summary_header!(&format!(
        "{} Quality Analysis v3.1 Engine ({encoder_name})",
        crate::media_conversion_gate::ui_icon_pick("⚖️", "[=]")
    ));
    crate::log_report_stat!(
        crate::infra::static_logs::messages::LABEL_DECISION_AUDIT,
        format!(
            "Execution Mode: {:?} | Target Bias: {:?}",
            d.match_mode, d.quality_bias
        )
    );
    let conf_str = crate::media_conversion_gate::ui_confidence_pct_whole_or_na(
        d.confidence,
        "quality_matcher_confidence_audit",
    );
    crate::log_report_stat!(
        crate::infra::static_logs::messages::LABEL_CONFIDENCE_AUDIT,
        format!("Predictive Confidence: {conf_str}")
    );
}

fn log_source_info(analysis: &QualityAnalysis, codec: SourceCodec, d: &AnalysisDetails) {
    crate::log_detail!(crate::infra::static_logs::messages::MSG_QUALITY_REPORT_SOURCE);
    crate::log_detail!(&format!(
        "      - Codec: {} ({:?}) | Efficiency Index: {:.2}",
        analysis.source_codec, codec, d.codec_factor
    ));
    if codec.is_cutting_edge() {
        crate::media_conversion_gate::probe_quality_batch_audit(
            "quality_cutting_edge_codec",
            crate::infra::static_logs::messages::MSG_QUALITY_CUTTING_EDGE,
        );
    } else if codec.is_modern() {
        crate::log_hint!(
            crate::infra::static_logs::messages::LABEL_HINT,
            crate::infra::static_logs::messages::MSG_QUALITY_MODERN
        );
    }
    crate::log_detail!(&format!(
        "      - Geometry: {}x{} | Scaling Factor: {:.2}",
        analysis.width, analysis.height, d.resolution_factor
    ));
    crate::log_detail!(&format!(
        "      - Bit Depth: {} | Precision Factor: {:.2}",
        analysis.format_bit_depth_label(),
        d.color_depth_factor
    ));
}

fn log_high_priority_factors(analysis: &QualityAnalysis, d: &AnalysisDetails) {
    crate::log_detail!(crate::infra::static_logs::messages::MSG_QUALITY_REPORT_HIGH);
    crate::log_detail!(&format!("      - Raw BPP Baseline: {:.4}", d.raw_bpp));
    if let Some(vbr) = analysis.video_bitrate {
        crate::log_detail!(&format!(
            "      - Forensic Bitrate: {} kbps (Stream Payload Only)",
            vbr / 1000
        ));
    }
    crate::log_detail!(&format!(
        "      - Temporal Topology (GOP): {:.2}",
        d.gop_factor
    ));
    if let Some(gop) = analysis.gop_size {
        crate::log_detail!(&format!(
            "         └─ Sequence: {}, B-Frames: {:?}",
            gop, analysis.b_frame_count
        ));
    }
    crate::log_detail!(&format!(
        "      - Chroma Fidelity Multiplier: {:.2}",
        d.chroma_factor
    ));
    if let Some(ref pf) = analysis.pix_fmt {
        crate::log_detail!(&format!("         └─ Native Format: {pf}"));
    }
    crate::log_detail!(&format!(
        "      - Photometric / Wide-Gamut Factor: {:.2}",
        d.hdr_factor
    ));
    if analysis.is_hdr == Some(true) {
        crate::log_detail!(crate::infra::static_logs::messages::MSG_QUALITY_HDR_CONFIRMED);
    }
    if d.content_type_adjustment != 0 {
        crate::log_detail!(&format!(
            "      - Semantic Context Adjustment: {:+} CRF",
            d.content_type_adjustment
        ));
        if let Some(ct) = analysis.content_type {
            crate::log_detail!(&format!("         └─ Identified Class: {ct:?}"));
        }
    }
}

fn log_medium_priority_factors(analysis: &QualityAnalysis, d: &AnalysisDetails) {
    crate::log_detail!(crate::infra::static_logs::messages::MSG_QUALITY_REPORT_MEDIUM);
    crate::log_detail!(&format!(
        "      - Geometric Aspect Multiplier: {:.2}",
        d.aspect_factor
    ));
    crate::log_detail!(&format!(
        "      - Information Complexity (SI/TI): {:.2}",
        d.complexity_factor
    ));
    if analysis.spatial_complexity.is_some() || analysis.temporal_complexity.is_some() {
        let fmt_complexity = |v: Option<f64>| -> String {
            match v {
                None => "—".to_string(),
                Some(x) => format!("{x:.1}"),
            }
        };
        crate::log_detail!(&format!(
            "         └─ Spatial Index: {}, Temporal Index: {}",
            fmt_complexity(analysis.spatial_complexity),
            fmt_complexity(analysis.temporal_complexity)
        ));
    }
    crate::log_detail!(&format!(
        "      - Perceptual Grain Density: {:.2}",
        d.grain_factor
    ));
    crate::log_detail!(&format!(
        "      - Transparency Channel (Alpha): {:.2}",
        d.alpha_factor
    ));
}

fn log_result_info(analysis: &QualityAnalysis, result: &MatchedQuality, encoder: EncoderType) {
    crate::log_detail!(crate::infra::static_logs::messages::MSG_QUALITY_REPORT_OUTPUT);
    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_QUALITY_BPP_TARGET
            .replace("{}", &format!("{:.4}", result.effective_bpp))
    );
    if let Some(fps) = analysis.fps {
        crate::log_detail!(
            &crate::infra::static_logs::messages::MSG_QUALITY_TEMPORAL_VELOCITY
                .replace("{}", &format!("{fps:.2}"))
        );
    }
    if let Some(duration) = analysis.duration_secs {
        crate::log_detail!(
            &crate::infra::static_logs::messages::MSG_QUALITY_STREAM_TIMELINE
                .replace("{}", &format!("{duration:.1}"))
        );
    }

    match encoder {
        EncoderType::Av1 | EncoderType::Hevc => {
            crate::log_success!(
                crate::infra::static_logs::messages::LABEL_DECISION_AUDIT,
                crate::infra::static_logs::messages::MSG_QUALITY_OPTIMAL_PARAM
                    .replace("{}", &format!("CRF {}", result.crf))
            );
        }
        EncoderType::Jxl => {
            crate::log_success!(
                crate::infra::static_logs::messages::LABEL_DECISION_AUDIT,
                crate::infra::static_logs::messages::MSG_QUALITY_OPTIMAL_PARAM
                    .replace("{}", &format!("Distance {:.2}", result.distance))
            );
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
/// # Panics
///
/// Panics if the internal numeric calculations for BPP encounter a
/// division-by-zero that was not caught by earlier guards.
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
        crate::numeric_cast::u64_to_f64(bitrate) / pixels_per_second
    } else {
        if pixels_per_second <= 0.0_f64 {
            crate::media_conversion_gate::probe_quality_batch_audit(
                "quality_bpp_pixels_per_second_zero",
                format!("pixels_per_second is {pixels_per_second} for {file_path}"),
            );
        }
        if bitrate == 0 {
            crate::media_conversion_gate::probe_quality_batch_audit(
                "quality_bpp_bitrate_zero",
                format!("bitrate is 0 for {file_path}"),
            );
        }
        0.0_f64
    };

    QualityAnalysis {
        bpp,
        source_codec: codec.to_string(),
        width,
        height,
        has_b_frames,
        bit_depth: Some(bit_depth),
        bit_depth_inferred_from_pix_fmt: false,
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
        fps: Option<f64>,
        duration_secs: Option<f64>,
    ) -> Self {
        self.analysis.source_codec = codec.to_string();
        self.analysis.width = width;
        self.analysis.height = height;
        self.analysis.fps = fps;
        self.analysis.duration_secs = duration_secs;
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
        if let (Some(fps), w, h) = (self.analysis.fps, self.analysis.width, self.analysis.height)
            && fps > 0.0_f64
            && w > 0
            && h > 0
        {
            let pixels = f64::from(w) * f64::from(h);
            self.analysis.bpp = (crate::numeric_cast::u64_to_f64(bitrate) / fps) / pixels;
        }
        self
    }

    #[must_use]
    pub const fn gop(mut self, gop_size: Option<u32>, b_frames: Option<u8>) -> Self {
        self.analysis.gop_size = gop_size;
        self.analysis.b_frame_count = b_frames;
        if let Some(b) = b_frames {
            self.analysis.has_b_frames = b > 0;
        }
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
    pub const fn hdr(mut self, is_hdr: bool) -> Self {
        self.analysis.is_hdr = Some(is_hdr);
        self
    }

    #[must_use]
    pub const fn content_type(mut self, ct: ContentType) -> Self {
        self.analysis.content_type = Some(ct);
        self
    }

    #[must_use]
    pub const fn bit_depth(mut self, depth: Option<u8>) -> Self {
        self.analysis.bit_depth = depth;
        self.analysis.bit_depth_inferred_from_pix_fmt = false;
        self
    }

    #[must_use]
    pub const fn bit_depth_with_source(
        mut self,
        depth: Option<u8>,
        inferred_from_pix_fmt: bool,
    ) -> Self {
        self.analysis.bit_depth = depth;
        self.analysis.bit_depth_inferred_from_pix_fmt = inferred_from_pix_fmt && depth.is_some();
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

    // Normal mode: skip all modern codecs (HEVC, AV1, VP9, VVC, AV2) — already
    // modern, no need to process. Only when Apple-compat flag is on do we
    // convert AV1/VP9/VVC/AV2 via should_skip_video_codec_apple_compat (skip HEVC
    // only).
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
            SourceCodec::H264
            | SourceCodec::Vp8
            | SourceCodec::Mpeg4
            | SourceCodec::Mpeg2
            | SourceCodec::Mpeg1
            | SourceCodec::Wmv
            | SourceCodec::Theora
            | SourceCodec::RealVideo
            | SourceCodec::FlashVideo
            | SourceCodec::ProRes
            | SourceCodec::DnxHD
            | SourceCodec::Mjpeg
            | SourceCodec::Ffv1
            | SourceCodec::UtVideo
            | SourceCodec::HuffYuv
            | SourceCodec::RawVideo
            | SourceCodec::Lagarith
            | SourceCodec::MagicYuv
            | SourceCodec::Gif
            | SourceCodec::Apng
            | SourceCodec::WebpAnimated
            | SourceCodec::Jpeg
            | SourceCodec::JpegXl
            | SourceCodec::Png
            | SourceCodec::WebpStatic
            | SourceCodec::Avif
            | SourceCodec::Heic
            | SourceCodec::Bmp
            | SourceCodec::Tiff
            | SourceCodec::Unknown => "modern codec",
        };
        crate::infra::static_logs::messages::MSG_QUALITY_SKIP_REASON.replace("{}", codec_name)
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
        crate::infra::static_logs::messages::MSG_QUALITY_APPLE_COMPAT_HEVC.to_string()
    } else {
        String::new()
    };

    SkipDecision {
        should_skip,
        reason,
        codec,
    }
}

/// True only when we may keep best-effort HEVC/AV1 output on
/// compression/quality failure.
/// - Apple-incompatible (AV1, VP9, VVC, AV2): user still gets an importable
///   file.
/// - ProRes/DNxHD are NOT included: decision is strictly by SSIM and size
///   balance.
#[must_use]
pub fn is_apple_incompatible_video_codec(codec_str: &str) -> bool {
    matches!(
        parse_source_codec(codec_str),
        SourceCodec::Av1 | SourceCodec::Vp9 | SourceCodec::Vvc | SourceCodec::Av2
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AppleOutcomeFlags {
    pub pure_media_compressed: bool,
    pub allow_size_tolerance: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AppleContextFlags {
    pub apple_compat: bool,
    pub source_is_gif: bool,
    /// Ultimate explore: 3D gate owns quality; do not salvage via size-only
    /// HEVC keep.
    pub ultimate_explore: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AppleFallbackFlags {
    pub outcome: AppleOutcomeFlags,
    pub context: AppleContextFlags,
}

/// Predicate for keeping Apple-compat fallback HEVC output.
#[derive(Debug, Clone, Copy)]
pub struct AppleFallbackKeepRequest<'a> {
    pub codec_str: &'a str,
    pub pure_media_size_ratio: f64,
    pub flags: AppleFallbackFlags,
}

#[must_use]
pub fn should_keep_apple_fallback_hevc_output(request: AppleFallbackKeepRequest<'_>) -> bool {
    if request.flags.context.ultimate_explore {
        return false;
    }
    // If the source is already Apple-native (like GIF), we never allow fallback to
    // a larger pure-media payload.
    if request.flags.context.source_is_gif || is_apple_native_format(request.codec_str) {
        return false;
    }
    if !request.flags.context.apple_compat || !is_apple_incompatible_video_codec(request.codec_str)
    {
        return false;
    }
    request.flags.outcome.pure_media_compressed
        || (request.flags.outcome.allow_size_tolerance
            && request.pure_media_size_ratio < crate::constants::SIZE_TOLERANCE_RATIO)
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
pub fn tiff_enabled() -> bool {
    match std::env::var("MFB_ENABLE_TIFF") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            tracing::warn!("Failed to read MFB_ENABLE_TIFF env var: {e}");
            false
        }
    }
}

#[must_use]
pub fn should_skip_image_format(format_str: &str, is_lossless: bool) -> SkipDecision {
    let codec = parse_source_codec(format_str);

    // Modern lossy static formats: skip to avoid generational loss.
    // WebP/AVIF lossy static are skipped; HEIC/HEIF lossy static follow the same
    // pattern.
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

    let is_tiff_disabled = matches!(codec, SourceCodec::Tiff) && !tiff_enabled();

    let should_skip = is_modern_lossy || is_jxl || is_tiff_disabled;

    let reason = if should_skip {
        let codec_name = match codec {
            SourceCodec::Tiff if is_tiff_disabled => {
                "TIFF/DNG (disabled by default; set MFB_ENABLE_TIFF=1 to enable)"
            }
            SourceCodec::WebpStatic => "lossy WebP",
            SourceCodec::Avif => "lossy AVIF",
            SourceCodec::Heic if !is_lossless => "lossy HEIC/HEIF",
            SourceCodec::Heic => "lossless HEIC/HEIF (converts to JXL)",
            SourceCodec::JpegXl => "JPEG XL (already optimal)",
            SourceCodec::H264
            | SourceCodec::H265
            | SourceCodec::Vvc
            | SourceCodec::Vp8
            | SourceCodec::Vp9
            | SourceCodec::Av1
            | SourceCodec::Av2
            | SourceCodec::Mpeg4
            | SourceCodec::Mpeg2
            | SourceCodec::Mpeg1
            | SourceCodec::Wmv
            | SourceCodec::Theora
            | SourceCodec::RealVideo
            | SourceCodec::FlashVideo
            | SourceCodec::ProRes
            | SourceCodec::DnxHD
            | SourceCodec::Mjpeg
            | SourceCodec::Ffv1
            | SourceCodec::UtVideo
            | SourceCodec::HuffYuv
            | SourceCodec::RawVideo
            | SourceCodec::Lagarith
            | SourceCodec::MagicYuv
            | SourceCodec::Gif
            | SourceCodec::Apng
            | SourceCodec::WebpAnimated
            | SourceCodec::Jpeg
            | SourceCodec::Png
            | SourceCodec::Bmp
            | SourceCodec::Tiff
            | SourceCodec::Unknown => "modern lossy format",
        };
        if is_tiff_disabled {
            codec_name.to_string()
        } else {
            crate::infra::static_logs::messages::MSG_QUALITY_SKIP_REASON_IMAGE
                .replace("{}", codec_name)
        }
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

/// Calculate quality analysis from raw image analysis results. Returns None if
/// calculation fails.
#[must_use]
pub fn from_image_analysis(
    format: &str,
    width: u32,
    height: u32,
    bit_depth: Option<u8>,
    has_alpha: bool,
    file_size: u64,
    duration_secs: Option<f64>,
    fps: Option<f64>,
    estimated_quality: Option<u8>,
) -> Option<QualityAnalysis> {
    let pixels = u64::from(width) * u64::from(height);

    let bpp = if let (Some(duration), Some(frame_rate)) = (duration_secs, fps) {
        if duration > 0.0_f64 && frame_rate > 0.0_f64 {
            let total_frames =
                crate::numeric_cast::f64_to_u64_strict(duration * frame_rate, "total_frames")?;
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

    Some(QualityAnalysis {
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
    })
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
            bit_depth: Some(8),
            has_alpha: false,
            duration_secs: Some(60.0_f64),
            fps: Some(30.0_f64),
            file_size: 100_000_000,
            estimated_quality: None,
            pix_fmt: Some("yuv420p".to_string()),
            ..Default::default()
        };

        let result = calculate_av1_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
        // Updated: AV1 CRF range is now 0.0-51.0 (not 15.0-40.0) after removing
        // artificial constraints
        assert!(result.crf >= 0.0 && result.crf <= 51.0);
        assert!(result.analysis_details.confidence > Some(0.5_f64));
    }

    #[test]
    #[serial_test::serial]
    fn matched_quality_seal_preserves_rejected_non_finite_factors() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL,
            "0",
        );
        let mut result = MatchedQuality {
            crf: 22.0,
            distance: 0.0,
            effective_bpp: 0.25,
            analysis_details: AnalysisDetails {
                raw_bpp: f64::NAN,
                codec_factor: f64::INFINITY,
                confidence: Some(1.5),
                ..Default::default()
            },
        };
        result.seal_algorithm_outputs();
        assert!(result.analysis_details.raw_bpp.is_nan());
        assert!(result.analysis_details.codec_factor.is_infinite());
        assert_eq!(result.analysis_details.confidence, Some(1.0));
    }

    #[test]
    fn test_hevc_crf_calculation() {
        let analysis = QualityAnalysis {
            bpp: 0.5,
            source_codec: "gif".to_string(),
            width: 640,
            height: 480,
            has_b_frames: false,
            bit_depth: Some(8),
            has_alpha: false,
            duration_secs: Some(5.0_f64),
            fps: Some(10.0_f64),
            file_size: 5_000_000,
            estimated_quality: None,
            pix_fmt: Some("yuv420p".to_string()),
            ..Default::default()
        };

        let result = calculate_hevc_crf(&analysis).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.crf <= 35.0);
    }

    #[test]
    fn test_size_guard_in_apple_compat_is_disabled_for_non_apple_native_inputs() {
        // Apple compat should never size-guard non-apple-native sources such as
        // WebP/AVIF: compatibility takes priority and the guard is only
        // meaningful for already-native inputs.
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
            bit_depth: Some(8),
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
        assert!(calculate_gop_factor(Some(1), Some(0)) < 0.8_f64);
        assert!(calculate_gop_factor(Some(250), Some(3)) > 1.3_f64);
        assert!((calculate_gop_factor(Some(30), Some(2)) - 1.08).abs() < 0.1_f64);
    }

    #[test]
    fn test_chroma_factor() {
        assert!(
            (calculate_chroma_factor(Some("yuv420p")).expect("yuv420p chroma") - 1.0).abs()
                < 0.01_f64
        );
        assert!(calculate_chroma_factor(Some("yuv444p")).expect("yuv444p chroma") > 1.1_f64);
        assert!(calculate_chroma_factor(Some("rgb24")).expect("rgb24 chroma") > 1.1_f64);
        assert!(calculate_chroma_factor(None).is_err());
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
            pix_fmt: Some("yuv420p".to_string()),
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
            bit_depth: Some(8),
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
        assert!(result.analysis_details.confidence > Some(0.8_f64));

        let minimal = QualityAnalysis {
            bpp: 0.0,
            source_codec: "unknown".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: false,
            bit_depth: None,
            has_alpha: false,
            duration_secs: None,
            fps: None,
            file_size: 100_000_000,
            estimated_quality: None,
            pix_fmt: Some("yuv420p".to_string()),
            ..Default::default()
        };
        let result = calculate_av1_crf(&minimal).unwrap_or_else(|e| panic!("{e}"));
        assert!(result.analysis_details.confidence < Some(0.7_f64));
    }

    #[test]
    fn test_quality_analysis_default_preserves_unknown_bit_depth() {
        let analysis = QualityAnalysis::default();
        assert_eq!(analysis.bit_depth, None);
        assert!(!analysis.bit_depth_inferred_from_pix_fmt);
    }

    #[test]
    fn test_from_image_analysis_preserves_unknown_bit_depth() {
        let analysis =
            from_image_analysis("png", 1920, 1080, None, false, 500_000, None, None, None)
                .unwrap_or_else(|| panic!("from_image_analysis should produce a quality analysis"));

        assert_eq!(analysis.bit_depth, None);
    }

    #[test]
    fn test_unknown_bit_depth_reduces_confidence_vs_explicit_bit_depth() {
        let explicit = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: true,
            bit_depth: Some(8),
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
        let unknown = QualityAnalysis {
            bit_depth: None,
            ..explicit.clone()
        };

        assert!(calculate_confidence_v3(&unknown) < calculate_confidence_v3(&explicit));
    }

    #[test]
    fn test_unrecognized_bit_depth_reduces_confidence_vs_recognized_bit_depth() {
        let recognized = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: true,
            bit_depth: Some(8),
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

        let unrecognized = QualityAnalysis {
            bit_depth: Some(14),
            ..recognized.clone()
        };

        assert!(
            calculate_confidence_v3(&unrecognized) < calculate_confidence_v3(&recognized),
            "unrecognized bit-depth must not receive known-feature credit"
        );

        let gif_recognized = QualityAnalysis {
            source_codec: "gif".to_string(),
            bit_depth: Some(4),
            ..recognized.clone()
        };
        let gif_unrecognized = QualityAnalysis {
            source_codec: "gif".to_string(),
            bit_depth: Some(9),
            ..recognized
        };
        assert!(
            calculate_confidence_v3(&gif_unrecognized) < calculate_confidence_v3(&gif_recognized),
            "GIF unrecognized bit-depth must reduce confidence"
        );
    }

    #[test]
    fn test_pix_fmt_inferred_bit_depth_reduces_confidence_vs_explicit_sample_depth() {
        let explicit = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            has_b_frames: true,
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: false,
            has_alpha: false,
            duration_secs: Some(60.0_f64),
            fps: Some(30.0_f64),
            file_size: 100_000_000,
            estimated_quality: Some(85),
            video_bitrate: Some(5_000_000),
            gop_size: Some(60),
            b_frame_count: Some(3),
            pix_fmt: Some("yuv420p10le".to_string()),
            ..Default::default()
        };
        let inferred = QualityAnalysis {
            bit_depth_inferred_from_pix_fmt: true,
            ..explicit.clone()
        };
        let unknown = QualityAnalysis {
            bit_depth: None,
            bit_depth_inferred_from_pix_fmt: false,
            ..explicit.clone()
        };

        let explicit_confidence = calculate_confidence_v3(&explicit);
        let inferred_confidence = calculate_confidence_v3(&inferred);
        let unknown_confidence = calculate_confidence_v3(&unknown);

        assert!(unknown_confidence < inferred_confidence);
        assert!(inferred_confidence < explicit_confidence);
    }

    #[test]
    fn test_pix_fmt_inferred_bit_depth_does_not_get_color_depth_factor() {
        let explicit = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: false,
            file_size: 100_000_000,
            fps: Some(30.0_f64),
            duration_secs: Some(60.0_f64),
            pix_fmt: Some("yuv420p10le".to_string()),
            ..Default::default()
        };
        let inferred = QualityAnalysis {
            bit_depth_inferred_from_pix_fmt: true,
            ..explicit.clone()
        };
        let unknown = QualityAnalysis {
            bit_depth: None,
            bit_depth_inferred_from_pix_fmt: false,
            pix_fmt: None,
            ..explicit.clone()
        };

        let (_, explicit_details) = calculate_effective_bpp_with_options(
            &explicit,
            EncoderType::Hevc,
            MatchMode::Quality,
            QualityBias::Balanced,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let (_, inferred_details) = calculate_effective_bpp_with_options(
            &inferred,
            EncoderType::Hevc,
            MatchMode::Quality,
            QualityBias::Balanced,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let unknown_details = calculate_effective_bpp_with_options(
            &unknown,
            EncoderType::Hevc,
            MatchMode::Quality,
            QualityBias::Balanced,
        )
        .expect_err("missing pix_fmt must fail effective_bpp, not forge chroma");

        assert!(explicit_details.color_depth_factor > 1.0_f64);
        assert!((inferred_details.color_depth_factor - 1.0_f64).abs() < f64::EPSILON);
        assert!(
            unknown_details.contains("pix_fmt"),
            "unknown bit_depth without pix_fmt must not run chroma heuristic: {unknown_details}"
        );
    }

    #[test]
    fn test_unrecognized_bit_depth_fails_instead_of_neutral_default() {
        let analysis = QualityAnalysis {
            bpp: 0.3,
            source_codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            bit_depth: Some(14),
            file_size: 100_000_000,
            fps: Some(30.0_f64),
            duration_secs: Some(60.0_f64),
            pix_fmt: Some("yuv420p".to_string()),
            ..Default::default()
        };
        let err = calculate_effective_bpp_with_options(
            &analysis,
            EncoderType::Hevc,
            MatchMode::Quality,
            QualityBias::Balanced,
        )
        .expect_err("unrecognized bit depth must fail-closed");
        assert!(err.contains("unsupported bit_depth"));
    }

    #[test]
    fn test_gop_none_does_not_force_false_has_b_frames() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0_f64), Some(60.0_f64))
            .gop(Some(60), Some(3))
            .gop(Some(60), None)
            .build();
        assert!(
            analysis.has_b_frames,
            "missing b-frame metadata must not forcibly rewrite prior evidence to false"
        );
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
        unsafe {
            std::env::remove_var("MFB_ENABLE_TIFF");
        }
        assert!(should_skip_image_format("tiff", true).should_skip);
        unsafe {
            std::env::set_var("MFB_ENABLE_TIFF", "1");
        }
        assert!(!should_skip_image_format("tiff", true).should_skip);
        unsafe {
            std::env::remove_var("MFB_ENABLE_TIFF");
        }
        assert!(!should_skip_image_format("heif", true).should_skip); // lossless HEIF → JXL
    }

    #[test]
    fn test_precision_1080p_h264_8mbps() {
        let analysis = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .bit_depth(Some(8))
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
            .basic("h264", 3840, 2160, Some(30.0), Some(60.0))
            .video_bitrate(20_000_000)
            .gop(Some(60), Some(3))
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .bit_depth(Some(8))
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
            .basic("h264", 1920, 1080, Some(24.0), Some(60.0))
            .video_bitrate(5_000_000)
            .gop(Some(48), Some(2))
            .pix_fmt("yuv420p")
            .build();

        let animation = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(24.0), Some(60.0))
            .video_bitrate(5_000_000)
            .gop(Some(48), Some(2))
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
            .basic("h264", 1920, 1080, Some(24.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(48), Some(2))
            .pix_fmt("yuv420p")
            .build();

        let grain = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(24.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(48), Some(2))
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
            .basic("h264", 3840, 2160, Some(30.0), Some(60.0))
            .video_bitrate(15_000_000)
            .gop(Some(60), Some(3))
            .pix_fmt("yuv420p10le")
            .color("bt709", false)
            .bit_depth(Some(10))
            .build();

        let hdr = VideoAnalysisBuilder::new()
            .basic("h264", 3840, 2160, Some(30.0), Some(60.0))
            .video_bitrate(15_000_000)
            .gop(Some(60), Some(3))
            .pix_fmt("yuv420p10le")
            .color("bt2020nc", true)
            .bit_depth(Some(10))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
            .pix_fmt("yuv420p")
            .build();

        let yuv444 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(20_000_000)
            .gop(Some(1), Some(0))
            .pix_fmt("yuv420p")
            .build();

        let long_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(250), Some(3))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(2_000_000)
            .gop(Some(60), Some(0))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
            .pix_fmt("yuv420p")
            .build();

        let ultrawide = VideoAnalysisBuilder::new()
            .basic("h264", 2560, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
            .pix_fmt("yuv420p")
            .build();

        let hevc_source = VideoAnalysisBuilder::new()
            .basic("hevc", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(500_000)
            .gop(Some(60), Some(0))
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
            .basic("prores", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(150_000_000)
            .gop(Some(1), Some(0))
            .pix_fmt("yuv422p10le")
            .bit_depth(Some(10))
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
            (result
                .analysis_details
                .confidence
                .expect("Confidence should be present")
                - calculate_confidence_v3(&jpeg))
            .abs()
                < f64::EPSILON,
            "JPEG estimated-quality path should use calculated confidence, got {:?}",
            result.analysis_details.confidence
        );
        assert!(
            result.analysis_details.confidence < Some(0.9_f64),
            "Sparse JPEG metadata should not be reported as fixed 0.9 confidence, got {:?}",
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
            bit_depth: Some(8),
            duration_secs: Some(5.0_f64),
            fps: Some(10.0_f64),
            file_size: 5_000_000,
            pix_fmt: Some("yuv420p".to_string()),
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(120.0))
            .video_bitrate(5_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1280, 720, Some(30.0), Some(60.0))
            .video_bitrate(2_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 3840, 2160, Some(30.0), Some(60.0))
            .video_bitrate(15_000_000)
            .gop(Some(60), Some(3))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(500_000)
            .gop(Some(60), Some(0))
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
            .basic("prores", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(100_000_000)
            .gop(Some(1), Some(0))
            .pix_fmt("yuv422p10le")
            .bit_depth(Some(10))
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
            .basic("h264", 320, 240, Some(15.0), Some(30.0))
            .video_bitrate(500_000)
            .gop(Some(30), Some(1))
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
            .basic("h264", 7680, 4320, Some(30.0), Some(60.0))
            .video_bitrate(50_000_000)
            .gop(Some(60), Some(3))
            .pix_fmt("yuv420p10le")
            .bit_depth(Some(10))
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
            .basic("h264", 1920, 1080, Some(120.0), Some(60.0))
            .video_bitrate(15_000_000)
            .gop(Some(120), Some(3))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(10_000_000)
            .gop(Some(2), Some(0))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(250), Some(8))
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
            .basic("h264", 3840, 2160, Some(30.0), Some(60.0))
            .video_bitrate(20_000_000)
            .gop(Some(60), Some(3))
            .pix_fmt("yuv420p10le")
            .color("bt2020nc", true)
            .bit_depth(Some(10))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(15_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1080, 1920, Some(30.0), Some(60.0))
            .video_bitrate(5_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 2560, 1080, Some(24.0), Some(120.0))
            .video_bitrate(8_000_000)
            .gop(Some(48), Some(2))
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
            .basic("ffv1", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(200_000_000)
            .gop(Some(1), Some(0))
            .pix_fmt("yuv444p10le")
            .bit_depth(Some(10))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(10), Some(1))
            .pix_fmt("yuv420p")
            .build();

        let long_gop = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(250), Some(3))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
            .pix_fmt("yuv420p")
            .build();

        let yuv444 = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
            .pix_fmt("yuv420p")
            .color("bt709", false)
            .build();

        let hdr = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
            .pix_fmt("yuv420p")
            .content_type(ContentType::LiveAction)
            .build();

        let animation = VideoAnalysisBuilder::new()
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
            .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
            .video_bitrate(8_000_000)
            .gop(Some(60), Some(2))
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
        .basic("vp9", 1920, 1080, Some(30.0), Some(60.0))
        .bit_depth(Some(8))
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
        .basic("av1", 1920, 1080, Some(30.0), Some(60.0))
        .bit_depth(Some(8))
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
        .basic("av1", 3840, 2160, Some(60.0), Some(120.0))
        .bit_depth(Some(10))
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
        .basic("h264", 1920, 1080, Some(30.0), Some(120.0))
        .bit_depth(Some(8))
        .file_size(120_000_000)
        .video_bitrate(8_000_000)
        .pix_fmt("yuv420p")
        .gop(Some(60), Some(2))
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
        .basic("h264", 1280, 720, Some(30.0), Some(60.0))
        .bit_depth(Some(8))
        .file_size(30_000_000)
        .video_bitrate(4_000_000)
        .pix_fmt("yuv420p")
        .gop(Some(30), Some(2))
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
        .basic("h264", 3840, 2160, Some(30.0), Some(180.0))
        .bit_depth(Some(8))
        .file_size(450_000_000)
        .video_bitrate(20_000_000)
        .pix_fmt("yuv420p")
        .gop(Some(60), Some(3))
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
        .basic("h264", 854, 480, Some(24.0), Some(300.0))
        .bit_depth(Some(8))
        .file_size(45_000_000)
        .video_bitrate(1_200_000)
        .pix_fmt("yuv420p")
        .gop(Some(48), Some(1))
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
        .basic("h264", 1920, 1080, Some(24.0), Some(7200.0))
        .bit_depth(Some(8))
        .file_size(4_500_000_000)
        .video_bitrate(40_000_000)
        .pix_fmt("yuv420p")
        .gop(Some(24), Some(3))
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
        .basic("h264", 1920, 1080, Some(30.0), Some(60.0))
        .bit_depth(Some(8))
        .file_size(60_000_000)
        .video_bitrate(8_000_000)
        .pix_fmt("yuv420p")
        .build();

    let av1 = VideoAnalysisBuilder::new()
        .basic("av1", 1920, 1080, Some(30.0), Some(60.0))
        .bit_depth(Some(8))
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
            .expect("JPEG content identification must not fail")
            .unwrap_or_else(|| panic!("Should identify JPEG"));
        assert_eq!(codec, SourceCodec::Jpeg);
        assert!(codec.is_extension_compatible("jpg"));
        assert!(codec.is_extension_compatible("jpeg"));
        assert!(!codec.is_extension_compatible("png"));
    }

    #[test]
    fn test_identify_by_content_ignores_arbitrary_filename_extensions() {
        let temp_dir =
            tempfile::tempdir().unwrap_or_else(|e| panic!("Failed to create temp dir: {e:?}"));
        for (rel, bytes, expected) in [
            (
                "jpeg-as-mp4.mp4",
                &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10][..],
                SourceCodec::Jpeg,
            ),
            (
                "jpeg-as-png.png",
                &[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x10][..],
                SourceCodec::Jpeg,
            ),
            (
                "png-as-txt.txt",
                &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A][..],
                SourceCodec::Png,
            ),
            (
                "heic-as-jpg.jpg",
                &[
                    0x00, 0x00, 0x00, 0x1C, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c',
                ][..],
                SourceCodec::Heic,
            ),
        ] {
            let path = temp_dir.path().join(rel);
            std::fs::write(&path, bytes).unwrap_or_else(|e| panic!("Failed to write {rel}: {e:?}"));
            let codec = SourceCodec::identify_by_content(&path)
                .unwrap_or_else(|e| panic!("Content identification failed for {rel}: {e:?}"))
                .unwrap_or_else(|| panic!("Should identify content for {rel}"));
            assert_eq!(codec, expected, "filename extension affected {rel}");
        }
    }

    #[test]
    fn identify_by_content_missing_file_returns_error_not_none() {
        let temp_dir =
            tempfile::tempdir().unwrap_or_else(|e| panic!("Failed to create temp dir: {e:?}"));
        let missing = temp_dir.path().join("missing.jpg");

        let err = SourceCodec::identify_by_content(&missing)
            .expect_err("missing content-identification target must be an error");

        assert!(err.to_string().contains("missing.jpg"));
    }

    #[test]
    fn test_identify_png() {
        let file = create_temp_with_content(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        let codec = SourceCodec::identify_by_content(file.path())
            .expect("PNG content identification must not fail")
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
            .expect("MP4 content identification must not fail")
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
            .expect("HEIC content identification must not fail")
            .unwrap_or_else(|| panic!("Should identify HEIC"));
        assert_eq!(codec, SourceCodec::Heic);
        assert!(codec.is_extension_compatible("heic"));
    }

    #[test]
    fn test_identify_mkv() {
        let file = create_temp_with_content(&[0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00, 0x00, 0x00]);
        let codec = SourceCodec::identify_by_content(file.path())
            .expect("MKV content identification must not fail")
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
