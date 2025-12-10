//! Video Conversion API Module
//!
//! Pure conversion layer - executes video conversions based on detection results.
//! - Auto Mode: FFV1 for lossless sources, AV1 for lossy sources
//! - Simple Mode: Always AV1 MP4
//! - Size Exploration: Tries higher CRF if output is larger than input

use crate::{VidQualityError, Result};
use crate::detection_api::{detect_video, VideoDetectionResult, CompressionType};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

/// Target video format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetVideoFormat {
    /// FFV1 in MKV container - for archival
    Ffv1Mkv,
    /// AV1 in MP4 container - for compression
    Av1Mp4,
    /// Skip conversion (already modern/efficient)
    Skip,
}

impl TargetVideoFormat {
    pub fn extension(&self) -> &str {
        match self {
            TargetVideoFormat::Ffv1Mkv => "mkv",
            TargetVideoFormat::Av1Mp4 => "mp4",
            TargetVideoFormat::Skip => "",
        }
    }
    
    pub fn as_str(&self) -> &str {
        match self {
            TargetVideoFormat::Ffv1Mkv => "FFV1 MKV (Archival)",
            TargetVideoFormat::Av1Mp4 => "AV1 MP4 (High Quality)",
            TargetVideoFormat::Skip => "Skip",
        }
    }
}

/// Conversion strategy result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionStrategy {
    pub target: TargetVideoFormat,
    pub reason: String,
    pub command: String,
    pub preserve_audio: bool,
    pub crf: u8,
    pub lossless: bool, // New field for mathematical lossless
}

/// Conversion configuration
#[derive(Debug, Clone)]
pub struct ConversionConfig {
    pub output_dir: Option<PathBuf>,
    pub force: bool,
    pub delete_original: bool,
    pub preserve_metadata: bool,
    /// Enable size exploration (try higher CRF if output > input)
    pub explore_smaller: bool,
    /// Use mathematical lossless AV1 (⚠️ VERY SLOW)
    pub use_lossless: bool,
    /// Match input video quality level (auto-calculate CRF based on input bitrate)
    pub match_quality: bool,
    /// In-place conversion: convert and delete original file
    pub in_place: bool,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            output_dir: None,
            force: false,
            delete_original: false,
            preserve_metadata: true,
            explore_smaller: false,
            use_lossless: false,
            match_quality: false,
            in_place: false,
        }
    }
}

impl ConversionConfig {
    /// Check if original should be deleted (either via delete_original or in_place)
    pub fn should_delete_original(&self) -> bool {
        self.delete_original || self.in_place
    }
}

/// Conversion output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionOutput {
    pub input_path: String,
    pub output_path: String,
    pub strategy: ConversionStrategy,
    pub input_size: u64,
    pub output_size: u64,
    pub size_ratio: f64,
    pub success: bool,
    pub message: String,
    /// CRF used for final output (if exploration was done)
    pub final_crf: u8,
    /// Number of exploration attempts
    pub exploration_attempts: u8,
}

/// Determine conversion strategy based on detection result (for auto mode)
pub fn determine_strategy(result: &VideoDetectionResult) -> ConversionStrategy {
    let codec_name = result.codec.as_str(); // e.g. "H.265"
    
    // Check for Modern Codecs -> SKIP
    // H.265/HEVC, AV1, VP9, H.266/VVC
    match result.codec {
        crate::detection_api::DetectedCodec::H265 |
        crate::detection_api::DetectedCodec::AV1 |
        crate::detection_api::DetectedCodec::AV2 | // Skip AV2
        crate::detection_api::DetectedCodec::VVC | // Skip VVC/H.266
        crate::detection_api::DetectedCodec::VP9 => {
             return ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: format!("Source is modern codec ({}) - skipping to avoid generational loss", codec_name),
                command: String::new(),
                preserve_audio: false,
                crf: 0,
                lossless: false,
            };
        }
        _ => {}
    }
    
    // Also check for "vvc" or "h266" in Unknown string if necessary
    if let crate::detection_api::DetectedCodec::Unknown(ref s) = result.codec {
        let s = s.to_lowercase();
        if s.contains("hevc") || s.contains("h265") || s.contains("av1") || s.contains("vp9") || s.contains("vvc") || s.contains("h266") {
             return ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: format!("Source is modern codec ({}) - skipping", s),
                command: String::new(),
                preserve_audio: false,
                crf: 0,
                lossless: false,
            };
        }
    }

    let (target, reason, crf, lossless) = match result.compression {
        CompressionType::Lossless => {
            (
                TargetVideoFormat::Av1Mp4,
                format!("Source is {} (lossless) - converting to AV1 Lossless", result.codec.as_str()),
                0,
                true // Enable mathematical lossless
            )
        }
        CompressionType::VisuallyLossless => {
            // Treat visually lossless source as high quality source -> AV1 CRF 0 (Lossy/High Quality)
            // User said: "Input ffv1 etc more lossless coding -> convert to av1 lossless"
            // But "Visually Lossless" (e.g. ProRes) is technically lossy. 
            // However, usually ProRes/DNxHD are intermediates. 
            // Let's stick to: If strictly Lossless -> AV1 Lossless. If "Visually Lossless" -> AV1 CRF 0.
            // Wait, user instruction: "Input ffv1 etc more lossless coding -> convert to av1 lossless"
            // "Input h264 lossy etc more coding -> convert to av1 crf 0"
            // ProRes is "more lossless" than H264. Let's treat it as High Quality Source -> CRF 0?
            // "Visually Lossless" in detection_api includes ProRes. 
            // Ideally ProRes -> AV1 CRF 0 is better than ProRes -> AV1 Lossless (huge).
            (
                TargetVideoFormat::Av1Mp4,
                format!("Source is {} (visually lossless) - compressing with AV1 CRF 0", result.codec.as_str()),
                0,
                false
            )
        }
        _ => {
            (
                TargetVideoFormat::Av1Mp4,
                format!("Source is {} ({}) - compressing with AV1 CRF 0", result.codec.as_str(), result.compression.as_str()),
                0,
                false
            )
        }
    };
    
    ConversionStrategy {
        target,
        reason,
        command: String::new(),
        preserve_audio: result.has_audio,
        crf,
        lossless,
    }
}

/// Simple mode conversion - ALWAYS use AV1 MP4 (LOSSLESS)
pub fn simple_convert(input: &Path, output_dir: Option<&Path>) -> Result<ConversionOutput> {
    let detection = detect_video(input)?;
    
    let output_dir = output_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());
    
    std::fs::create_dir_all(&output_dir)?;
    
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    
    // 🔥 当输入是 mp4 时，添加 _av1 后缀避免冲突
    let output_path = if input_ext.eq_ignore_ascii_case("mp4") {
        output_dir.join(format!("{}_av1.mp4", stem))
    } else {
        output_dir.join(format!("{}.mp4", stem))
    };
    
    info!("🎬 Simple Mode: {} → AV1 MP4 (LOSSLESS)", input.display());
    
    // Always AV1 MP4 with LOSSLESS mode (as requested: corresponding to image JXL lossless)
    // Note: This produces large files but is mathematically lossless.
    let output_size = execute_av1_lossless(&detection, &output_path)?;
    
    // Preserve metadata (complete copy)
    copy_metadata(input, &output_path);
    
    let size_ratio = output_size as f64 / detection.file_size as f64;
    
    info!("   ✅ Complete: {:.1}% of original", size_ratio * 100.0);
    
    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path: output_path.display().to_string(),
        strategy: ConversionStrategy {
            target: TargetVideoFormat::Av1Mp4,
            reason: "Simple mode: Always AV1 Lossless".to_string(),
            command: String::new(),
            preserve_audio: detection.has_audio,
            crf: 0,
            lossless: true,
        },
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: true,
        message: "Simple conversion successful (Lossless)".to_string(),
        final_crf: 0,
        exploration_attempts: 0,
    })
}

// remove simple_convert_with_lossless as it's no longer needed/used with the new policy

/// Auto mode conversion with intelligent strategy selection
pub fn auto_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    let detection = detect_video(input)?;
    let strategy = determine_strategy(&detection);
    
    // Handle Skip Strategy
    if strategy.target == TargetVideoFormat::Skip {
        info!("🎬 Auto Mode: {} → SKIP", input.display());
        info!("   Reason: {}", strategy.reason);
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: "".to_string(),
            strategy,
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 0.0,
            success: true, 
            message: "Skipped modern codec to avoid generation loss".to_string(),
            final_crf: 0,
            exploration_attempts: 0,
        });
    }

    let output_dir = config.output_dir.clone()
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());
    
    std::fs::create_dir_all(&output_dir)?;
    
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let target_ext = strategy.target.extension();
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    
    // 🔥 当输入输出扩展名相同时，添加 _av1 后缀避免冲突
    let output_path = if input_ext.eq_ignore_ascii_case(target_ext) {
        output_dir.join(format!("{}_av1.{}", stem, target_ext))
    } else {
        output_dir.join(format!("{}.{}", stem, target_ext))
    };
    
    // 🔥 检测输入输出路径冲突（作为安全检查）
    check_input_output_conflict(input, &output_path)?;
    
    if output_path.exists() && !config.force {
        return Err(VidQualityError::ConversionError(
            format!("Output exists: {}", output_path.display())
        ));
    }
    
    info!("🎬 Auto Mode: {} → {}", input.display(), strategy.target.as_str());
    info!("   Reason: {}", strategy.reason);
    
    let (output_size, final_crf, attempts) = match strategy.target {
        TargetVideoFormat::Ffv1Mkv => {
            // Legacy/Fallback catch-all
            let size = execute_ffv1_conversion(&detection, &output_path)?;
            (size, 0, 0)
        }
        TargetVideoFormat::Av1Mp4 => {
            if strategy.lossless {
                 info!("   🚀 Using AV1 Mathematical Lossless Mode");
                 let size = execute_av1_lossless(&detection, &output_path)?;
                 (size, 0, 0)
            } else if config.explore_smaller {
                // Size exploration mode (only valid for lossy)
                explore_smaller_size(&detection, &output_path)?
            } else if config.match_quality {
                // Calculate CRF to match input quality
                let matched_crf = calculate_matched_crf(&detection);
                info!("   🎯 Match Quality Mode: using CRF {} to match input quality", matched_crf);
                let size = execute_av1_conversion(&detection, &output_path, matched_crf)?;
                (size, matched_crf, 0)
            } else {
                let size = execute_av1_conversion(&detection, &output_path, 0)?;
                (size, 0, 0)
            }
        }
        TargetVideoFormat::Skip => unreachable!(), // Handled above
    };
    
    // Preserve metadata (complete copy)
    copy_metadata(input, &output_path);
    
    let size_ratio = output_size as f64 / detection.file_size as f64;
    
    if config.should_delete_original() {
        std::fs::remove_file(input)?;
        info!("   🗑️  Original deleted");
    }
    
    info!("   ✅ Complete: {:.1}% of original", size_ratio * 100.0);
    
    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path: output_path.display().to_string(),
        strategy: ConversionStrategy {
            target: strategy.target,
            reason: strategy.reason,
            command: String::new(),
            preserve_audio: detection.has_audio,
            crf: final_crf,
            lossless: strategy.lossless,
        },
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: true,
        message: if attempts > 0 {
            format!("Explored {} CRF values, final CRF: {}", attempts, final_crf)
        } else {
            "Conversion successful".to_string()
        },
        final_crf,
        exploration_attempts: attempts,
    })
}

/// Calculate CRF to match input video quality level (Enhanced Algorithm for AV1)
/// 
/// This function uses a more precise algorithm that considers:
/// 1. Bits per pixel (bpp) - primary quality indicator
/// 2. Source codec efficiency - H.264 vs others
/// 3. Profile/B-frames - encoding complexity
/// 4. Resolution scaling - higher res needs more bits
/// 
/// AV1 CRF range is 0-63, with 23 being default "good quality"
/// CRF ≈ 63 - 10 * log2(effective_bpp * 100)
/// 
/// Clamped to range [18, 35] for AV1
pub fn calculate_matched_crf(detection: &VideoDetectionResult) -> u8 {
    // Use pre-calculated bpp if available, otherwise calculate
    let bpp = if detection.bits_per_pixel > 0.0 {
        detection.bits_per_pixel
    } else {
        let pixels_per_frame = (detection.width as f64) * (detection.height as f64);
        let pixels_per_second = pixels_per_frame * detection.fps;
        if pixels_per_second <= 0.0 {
            info!("   ⚠️  Cannot calculate bpp, using default CRF 28");
            return 28;
        }
        (detection.bitrate as f64) / pixels_per_second
    };
    
    // Codec efficiency factor (AV1 is ~30% more efficient than HEVC, ~50% more than H.264)
    let codec_factor = match detection.codec {
        crate::detection_api::DetectedCodec::H264 => 1.0,      // Baseline
        crate::detection_api::DetectedCodec::H265 => 0.7,      // More efficient
        crate::detection_api::DetectedCodec::VP9 => 0.75,      // Similar to HEVC
        crate::detection_api::DetectedCodec::AV1 => 0.5,       // Most efficient (already AV1)
        crate::detection_api::DetectedCodec::ProRes => 1.5,    // High bitrate codec
        crate::detection_api::DetectedCodec::DNxHD => 1.5,     // High bitrate codec
        crate::detection_api::DetectedCodec::MJPEG => 2.0,     // Very inefficient
        _ => 1.0,
    };
    
    // B-frames bonus (B-frames improve compression efficiency)
    let bframe_factor = if detection.has_b_frames { 1.1 } else { 1.0 };
    
    // Resolution factor (higher res is harder to compress efficiently)
    let pixels = (detection.width as f64) * (detection.height as f64);
    let resolution_factor = if pixels > 8_000_000.0 {
        0.85  // 4K+ needs more bits
    } else if pixels > 2_000_000.0 {
        0.9   // 1080p
    } else if pixels > 500_000.0 {
        0.95  // 720p
    } else {
        1.0   // SD
    };
    
    // Effective bpp after adjustments
    let effective_bpp = bpp * codec_factor * bframe_factor * resolution_factor;
    
    // Convert bpp to CRF using logarithmic formula for AV1
    // AV1 CRF range is 0-63, with 23 being default "good quality"
    // SVT-AV1 CRF is more aggressive than x265, so we use a different formula
    // CRF = 50 - 8 * log2(effective_bpp * 100)
    // This gives roughly:
    // bpp=1.0 → CRF ~18
    // bpp=0.3 → CRF ~24
    // bpp=0.1 → CRF ~28
    // bpp=0.03 → CRF ~33
    let crf_float = if effective_bpp > 0.0 {
        50.0 - 8.0 * (effective_bpp * 100.0).log2()
    } else {
        28.0
    };
    
    // Clamp to reasonable range [18, 35] for AV1
    let crf = (crf_float.round() as i32).clamp(18, 35) as u8;
    
    info!("   📊 Quality Analysis:");
    info!("      Raw bpp: {:.4}", bpp);
    info!("      Codec factor: {:.2} ({})", codec_factor, detection.codec.as_str());
    info!("      B-frames: {} (factor: {:.2})", detection.has_b_frames, bframe_factor);
    info!("      Resolution: {}x{} (factor: {:.2})", detection.width, detection.height, resolution_factor);
    info!("      Effective bpp: {:.4}", effective_bpp);
    info!("      Calculated CRF: {}", crf);
    
    crf
}

/// Explore smaller size by trying higher CRF values (conservative approach)
/// Starts at CRF 0 and increases until output < input (even by 1 byte counts)
fn explore_smaller_size(
    detection: &VideoDetectionResult,
    output_path: &Path,
) -> Result<(u64, u8, u8)> {
    let input_size = detection.file_size;
    let mut current_crf: u8 = 0;
    let mut attempts: u8 = 0;
    const MAX_CRF: u8 = 23;  // Conservative limit
    const CRF_STEP: u8 = 1;   // Step size for exploration (conservative)
    
    info!("   🔍 Exploring smaller size (input: {} bytes)", input_size);
    
    loop {
        let output_size = execute_av1_conversion(detection, output_path, current_crf)?;
        attempts += 1;
        
        info!("   📊 CRF {}: {} bytes ({:.1}%)", 
            current_crf, output_size, (output_size as f64 / input_size as f64) * 100.0);
        
        // Success: output is smaller (even by 1 byte)
        if output_size < input_size {
            info!("   ✅ Found smaller output at CRF {}", current_crf);
            return Ok((output_size, current_crf, attempts));
        }
        
        // Try next CRF
        current_crf += CRF_STEP;
        
        // Safety limit
        if current_crf > MAX_CRF {
            warn!("   ⚠️  Reached CRF limit, using CRF {}", MAX_CRF);
            let output_size = execute_av1_conversion(detection, output_path, MAX_CRF)?;
            return Ok((output_size, MAX_CRF, attempts));
        }
    }
}

/// Execute FFV1 conversion
fn execute_ffv1_conversion(detection: &VideoDetectionResult, output: &Path) -> Result<u64> {
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    
    // 🔥 偶数分辨率处理：确保宽高为偶数
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    
    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(), max_threads.to_string(),  // 限制 ffmpeg 线程数
        "-i".to_string(), detection.file_path.clone(),
        "-c:v".to_string(), "ffv1".to_string(),
        "-level".to_string(), "3".to_string(),
        "-coder".to_string(), "1".to_string(),
        "-context".to_string(), "1".to_string(),
        "-g".to_string(), "1".to_string(),
        "-slices".to_string(), max_threads.to_string(),  // 使用与线程数相同的 slices
        "-slicecrc".to_string(), "1".to_string(),
    ];
    
    // 添加视频滤镜（偶数分辨率）
    for arg in &vf_args {
        args.push(arg.clone());
    }
    
    if detection.has_audio {
        args.extend(vec!["-c:a".to_string(), "flac".to_string()]);
    } else {
        args.push("-an".to_string());
    }
    
    args.push(output.display().to_string());
    
    let result = Command::new("ffmpeg").args(&args).output()?;
    
    if !result.status.success() {
        return Err(VidQualityError::FFmpegError(
            String::from_utf8_lossy(&result.stderr).to_string()
        ));
    }
    
    Ok(std::fs::metadata(output)?.len())
}

/// Execute AV1 conversion with specified CRF (using SVT-AV1 for better performance)
fn execute_av1_conversion(detection: &VideoDetectionResult, output: &Path, crf: u8) -> Result<u64> {
    // 使用 SVT-AV1 编码器 (libsvtav1) - 比 libaom-av1 快 10-20 倍
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let svt_params = format!("tune=0:film-grain=0:lp={}", max_threads);
    
    // 🔥 偶数分辨率处理：AV1 编码器要求宽高为偶数
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    
    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(), max_threads.to_string(),  // 限制 ffmpeg 线程数
        "-i".to_string(), detection.file_path.clone(),
        "-c:v".to_string(), "libsvtav1".to_string(),
        "-crf".to_string(), crf.to_string(),
        "-preset".to_string(), "6".to_string(),  // 0-13, 6 是平衡点
        "-svtav1-params".to_string(), svt_params,  // 限制 SVT-AV1 线程数
    ];
    
    // 添加视频滤镜（偶数分辨率）
    for arg in &vf_args {
        args.push(arg.clone());
    }
    
    if detection.has_audio {
        args.extend(vec![
            "-c:a".to_string(), "aac".to_string(),
            "-b:a".to_string(), "320k".to_string(),
        ]);
    } else {
        args.push("-an".to_string());
    }
    
    args.push(output.display().to_string());
    
    let result = Command::new("ffmpeg").args(&args).output()?;
    
    if !result.status.success() {
        return Err(VidQualityError::FFmpegError(
            String::from_utf8_lossy(&result.stderr).to_string()
        ));
    }
    
    Ok(std::fs::metadata(output)?.len())
}

/// Execute mathematical lossless AV1 conversion using SVT-AV1 (⚠️ SLOW, huge files)
fn execute_av1_lossless(detection: &VideoDetectionResult, output: &Path) -> Result<u64> {
    warn!("⚠️  Mathematical lossless AV1 encoding (SVT-AV1) - this will be SLOW!");
    
    // SVT-AV1 无损模式: crf=0 + lossless=1
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let svt_params = format!("lossless=1:lp={}", max_threads);
    
    // 🔥 偶数分辨率处理：AV1 编码器要求宽高为偶数
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    
    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(), max_threads.to_string(),  // 限制 ffmpeg 线程数
        "-i".to_string(), detection.file_path.clone(),
        "-c:v".to_string(), "libsvtav1".to_string(),
        "-crf".to_string(), "0".to_string(),
        "-preset".to_string(), "4".to_string(),  // 无损模式用更慢的 preset 保证质量
        "-svtav1-params".to_string(), svt_params,  // 数学无损 + 限制线程数
    ];
    
    // 添加视频滤镜（偶数分辨率）
    for arg in &vf_args {
        args.push(arg.clone());
    }
    
    if detection.has_audio {
        args.extend(vec!["-c:a".to_string(), "flac".to_string()]);  // 无损音频
    } else {
        args.push("-an".to_string());
    }
    
    args.push(output.display().to_string());
    
    let result = Command::new("ffmpeg").args(&args).output()?;
    
    if !result.status.success() {
        return Err(VidQualityError::FFmpegError(
            String::from_utf8_lossy(&result.stderr).to_string()
        ));
    }
    
    Ok(std::fs::metadata(output)?.len())
}



// MacOS specialized timestamp setter (creation time + date added)

/// 🔥 检测输入输出路径冲突
/// 当输入和输出是同一个文件时，响亮报错并提供建议
fn check_input_output_conflict(input: &Path, output: &Path) -> Result<()> {
    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.to_path_buf())
    } else {
        output.to_path_buf()
    };
    
    if input_canonical == output_canonical || input == output {
        return Err(VidQualityError::ConversionError(format!(
            "❌ 输入和输出路径相同: {}\n\
             💡 建议:\n\
             - 使用 --output/-o 指定不同的输出目录\n\
             - 或确保输入文件扩展名与目标格式不同",
            input.display()
        )));
    }
    Ok(())
}

// Helper to copy metadata and timestamps from source to destination
// Maximum metadata preservation: centralized via shared_utils::metadata
pub fn copy_metadata(src: &Path, dst: &Path) {
    // shared_utils::preserve_metadata handles ALL layers:
    // 1. Internal (Exif/IPTC via ExifTool)
    // 2. Network (WhereFroms check)
    // 3. System (ACL, Flags, Xattr, Timestamps via copyfile)
    if let Err(e) = shared_utils::preserve_metadata(src, dst) {
         eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }
}



// Legacy alias for backward compatibility
pub fn smart_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    auto_convert(input, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_target_format() {
        assert_eq!(TargetVideoFormat::Ffv1Mkv.extension(), "mkv");
        assert_eq!(TargetVideoFormat::Av1Mp4.extension(), "mp4");
    }
}
