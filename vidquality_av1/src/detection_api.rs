//! Video Detection API Module
//!
//! Pure analysis layer - detects video properties using ffprobe.
//! Determines codec type, compression level, and archival suitability.

use crate::Result;
use crate::ffprobe::probe_video;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Detected video codec
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectedCodec {
    /// FFV1 - Lossless archival codec
    FFV1,
    /// H.264/AVC
    H264,
    /// H.265/HEVC
    H265,
    /// VP9
    VP9,
    /// AV1
    AV1,
    /// AV2 (Experimental)
    AV2,
    /// H.266/VVC
    VVC,
    /// Apple ProRes
    ProRes,
    /// Avid DNxHD/DNxHR
    DNxHD,
    /// Motion JPEG
    MJPEG,
    /// Uncompressed (rawvideo)
    Uncompressed,
    /// HuffYUV - Lossless
    HuffYUV,
    /// UT Video - Lossless
    UTVideo,
    /// Unknown codec
    Unknown(String),
}

impl DetectedCodec {
    pub fn from_ffprobe(codec_name: &str) -> Self {
        match codec_name.to_lowercase().as_str() {
            "ffv1" => DetectedCodec::FFV1,
            "h264" | "avc" | "libx264" => DetectedCodec::H264,
            "hevc" | "h265" | "libx265" => DetectedCodec::H265,
            "vp9" | "libvpx-vp9" => DetectedCodec::VP9,
            "av1" | "libaom-av1" | "libsvtav1" => DetectedCodec::AV1,
            "av2" => DetectedCodec::AV2, // Experimental
            "vvc" | "h266" => DetectedCodec::VVC,
            "prores" | "prores_ks" => DetectedCodec::ProRes,
            "dnxhd" | "dnxhr" => DetectedCodec::DNxHD,
            "mjpeg" | "mjpegb" => DetectedCodec::MJPEG,
            "rawvideo" => DetectedCodec::Uncompressed,
            "huffyuv" | "ffvhuff" => DetectedCodec::HuffYUV,
            "utvideo" => DetectedCodec::UTVideo,
            _ => DetectedCodec::Unknown(codec_name.to_string()),
        }
    }
    
    /// Check if codec is natively lossless
    pub fn is_lossless(&self) -> bool {
        matches!(self,
            DetectedCodec::FFV1 |
            DetectedCodec::Uncompressed |
            DetectedCodec::HuffYUV |
            DetectedCodec::UTVideo
        )
    }
    
    /// Check if codec can be lossless (like ProRes 4444 XQ)
    pub fn can_be_lossless(&self) -> bool {
        matches!(self,
            DetectedCodec::FFV1 |
            DetectedCodec::Uncompressed |
            DetectedCodec::HuffYUV |
            DetectedCodec::UTVideo |
            DetectedCodec::ProRes |
            DetectedCodec::DNxHD
        )
    }
    
    pub fn as_str(&self) -> &str {
        match self {
            DetectedCodec::FFV1 => "FFV1",
            DetectedCodec::H264 => "H.264",
            DetectedCodec::H265 => "H.265",
            DetectedCodec::VP9 => "VP9",
            DetectedCodec::AV1 => "AV1",
            DetectedCodec::AV2 => "AV2",
            DetectedCodec::VVC => "H.266/VVC",
            DetectedCodec::ProRes => "ProRes",
            DetectedCodec::DNxHD => "DNxHD/DNxHR",
            DetectedCodec::MJPEG => "MJPEG",
            DetectedCodec::Uncompressed => "Uncompressed",
            DetectedCodec::HuffYUV => "HuffYUV",
            DetectedCodec::UTVideo => "UTVideo",
            DetectedCodec::Unknown(s) => s,
        }
    }
}

/// Compression type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    /// Mathematically lossless (FFV1, Uncompressed, etc.)
    Lossless,
    /// Visually lossless (CRF 0-4, high-quality ProRes)
    VisuallyLossless,
    /// High quality lossy (CRF 5-18)
    HighQuality,
    /// Standard quality (CRF 19-28)
    Standard,
    /// Low quality (CRF 29+)
    LowQuality,
}

impl CompressionType {
    pub fn as_str(&self) -> &str {
        match self {
            CompressionType::Lossless => "Lossless",
            CompressionType::VisuallyLossless => "Visually Lossless",
            CompressionType::HighQuality => "High Quality",
            CompressionType::Standard => "Standard Quality",
            CompressionType::LowQuality => "Low Quality",
        }
    }
}

/// Color space information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSpace {
    BT709,
    BT2020,
    SRGB,
    AdobeRGB,
    Unknown(String),
}

impl ColorSpace {
    /// Parse color space from string
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bt709" => ColorSpace::BT709,
            "bt2020" | "bt2020nc" | "bt2020ncl" => ColorSpace::BT2020,
            "srgb" | "iec61966-2-1" => ColorSpace::SRGB,
            "adobergb" => ColorSpace::AdobeRGB,
            _ => ColorSpace::Unknown(s.to_string()),
        }
    }
}

/// Complete video detection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoDetectionResult {
    pub file_path: String,
    pub format: String,
    pub codec: DetectedCodec,
    pub codec_long: String,
    pub compression: CompressionType,
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub fps: f64,
    pub duration_secs: f64,
    pub bit_depth: u8,
    pub pix_fmt: String,
    pub color_space: ColorSpace,
    pub bitrate: u64,
    pub has_audio: bool,
    pub audio_codec: Option<String>,
    pub file_size: u64,
    /// Estimated quality score (0-100)
    pub quality_score: u8,
    /// Recommendation for archival
    pub archival_candidate: bool,
    // Enhanced fields for precise CRF matching
    pub profile: Option<String>,
    pub has_b_frames: bool,
    pub video_bitrate: Option<u64>,
    /// Bits per pixel (for quality estimation)
    pub bits_per_pixel: f64,
}

/// Determine compression type based on codec and bitrate
fn determine_compression_type(
    codec: &DetectedCodec,
    bitrate: u64,
    width: u32,
    height: u32,
    fps: f64,
) -> CompressionType {
    // Always lossless codecs
    if codec.is_lossless() {
        return CompressionType::Lossless;
    }
    
    // ProRes and DNxHD are typically visually lossless
    if matches!(codec, DetectedCodec::ProRes | DetectedCodec::DNxHD) {
        return CompressionType::VisuallyLossless;
    }
    
    // Estimate based on bits per pixel
    let pixels_per_second = (width as f64) * (height as f64) * fps;
    if pixels_per_second > 0.0 {
        let bits_per_pixel = (bitrate as f64 * 8.0) / pixels_per_second;
        
        // High bits per pixel suggests high quality
        if bits_per_pixel > 2.0 {
            return CompressionType::VisuallyLossless;
        } else if bits_per_pixel > 0.5 {
            return CompressionType::HighQuality;
        } else if bits_per_pixel > 0.1 {
            return CompressionType::Standard;
        }
    }
    
    CompressionType::LowQuality
}

/// Calculate quality score (0-100)
fn calculate_quality_score(
    compression: &CompressionType,
    bit_depth: u8,
    _bitrate: u64,
    width: u32,
    height: u32,
) -> u8 {
    let base_score = match compression {
        CompressionType::Lossless => 100,
        CompressionType::VisuallyLossless => 95,
        CompressionType::HighQuality => 80,
        CompressionType::Standard => 60,
        CompressionType::LowQuality => 40,
    };
    
    // Adjust for bit depth
    let depth_bonus = if bit_depth >= 10 { 5 } else { 0 };
    
    // Adjust for resolution (4K+ gets bonus)
    let res_bonus = if width >= 3840 || height >= 2160 { 3 } else { 0 };
    
    (base_score + depth_bonus + res_bonus).min(100)
}

/// Detect video properties - main entry point
pub fn detect_video(path: &Path) -> Result<VideoDetectionResult> {
    let probe = probe_video(path)?;
    
    let codec = DetectedCodec::from_ffprobe(&probe.video_codec);
    
    // Calculate bits per pixel for quality estimation
    let pixels_per_second = (probe.width as f64) * (probe.height as f64) * probe.frame_rate;
    let bits_per_pixel = if pixels_per_second > 0.0 {
        (probe.bit_rate as f64) / pixels_per_second
    } else {
        0.0
    };
    
    let compression = determine_compression_type(
        &codec,
        probe.bit_rate,
        probe.width,
        probe.height,
        probe.frame_rate,
    );
    
    let color_space = probe.color_space
        .as_ref()
        .map(|s| ColorSpace::parse(s))
        .unwrap_or(ColorSpace::Unknown("unknown".to_string()));
    
    let quality_score = calculate_quality_score(
        &compression,
        probe.bit_depth,
        probe.bit_rate,
        probe.width,
        probe.height,
    );
    
    // Determine if suitable for archival (should use FFV1)
    let archival_candidate = matches!(compression, 
        CompressionType::Lossless | CompressionType::VisuallyLossless
    ) || codec.can_be_lossless();
    
    Ok(VideoDetectionResult {
        file_path: path.display().to_string(),
        format: probe.format_name,
        codec,
        codec_long: probe.video_codec_long,
        compression,
        width: probe.width,
        height: probe.height,
        frame_count: probe.frame_count,
        fps: probe.frame_rate,
        duration_secs: probe.duration,
        bit_depth: probe.bit_depth,
        pix_fmt: probe.pix_fmt,
        color_space,
        bitrate: probe.bit_rate,
        has_audio: probe.has_audio,
        audio_codec: probe.audio_codec,
        file_size: probe.size,
        quality_score,
        archival_candidate,
        profile: probe.profile,
        has_b_frames: probe.has_b_frames,
        video_bitrate: probe.video_bit_rate,
        bits_per_pixel,
    })
}
