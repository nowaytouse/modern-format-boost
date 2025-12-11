//! Video Conversion API Module - HEVC/H.265 Version
//!
//! Pure conversion layer - executes video conversions based on detection results.
//! - Auto Mode: HEVC Lossless for lossless sources, HEVC CRF for lossy sources
//! - Simple Mode: Always HEVC MP4
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
    /// HEVC Lossless in MKV container - for archival
    HevcLosslessMkv,
    /// HEVC in MP4 container - for compression
    HevcMp4,
    /// Skip conversion (already modern/efficient)
    Skip,
}

impl TargetVideoFormat {
    pub fn extension(&self) -> &str {
        match self {
            TargetVideoFormat::HevcLosslessMkv => "mkv",
            TargetVideoFormat::HevcMp4 => "mp4",
            TargetVideoFormat::Skip => "",
        }
    }
    
    pub fn as_str(&self) -> &str {
        match self {
            TargetVideoFormat::HevcLosslessMkv => "HEVC Lossless MKV (Archival)",
            TargetVideoFormat::HevcMp4 => "HEVC MP4 (High Quality)",
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
    /// 🔥 v3.4: Changed from u8 to f32 for sub-integer precision (0.5 step)
    pub crf: f32,
    pub lossless: bool,
}

/// Conversion configuration
#[derive(Debug, Clone)]
pub struct ConversionConfig {
    pub output_dir: Option<PathBuf>,
    pub force: bool,
    pub delete_original: bool,
    pub preserve_metadata: bool,
    pub explore_smaller: bool,
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
    /// 🔥 v3.4: Changed from u8 to f32 for sub-integer precision (0.5 step)
    pub final_crf: f32,
    pub exploration_attempts: u8,
}

/// Determine conversion strategy based on detection result (for auto mode)
pub fn determine_strategy(result: &VideoDetectionResult) -> ConversionStrategy {
    // 🔥 使用统一的跳过检测逻辑 (shared_utils::should_skip_video_codec)
    // 支持: H.265/HEVC, AV1, VP9, VVC/H.266, AV2
    let skip_decision = shared_utils::should_skip_video_codec(result.codec.as_str());
    
    if skip_decision.should_skip {
        return ConversionStrategy {
            target: TargetVideoFormat::Skip,
            reason: skip_decision.reason,
            command: String::new(),
            preserve_audio: false,
            crf: 0.0,
            lossless: false,
        };
    }
    
    // 🔥 Also check Unknown codec string for modern formats
    if let crate::detection_api::DetectedCodec::Unknown(ref s) = result.codec {
        let unknown_skip = shared_utils::should_skip_video_codec(s);
        if unknown_skip.should_skip {
            return ConversionStrategy {
                target: TargetVideoFormat::Skip,
                reason: unknown_skip.reason,
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            };
        }
    }

    // 🔥 v3.4: CRF values are now f32 for sub-integer precision
    let (target, reason, crf, lossless) = match result.compression {
        CompressionType::Lossless => {
            (
                TargetVideoFormat::HevcLosslessMkv,
                format!("Source is {} (lossless) - converting to HEVC Lossless", result.codec.as_str()),
                0.0_f32,
                true
            )
        }
        CompressionType::VisuallyLossless => {
            (
                TargetVideoFormat::HevcMp4,
                format!("Source is {} (visually lossless) - compressing with HEVC CRF 18", result.codec.as_str()),
                18.0_f32,
                false
            )
        }
        _ => {
            (
                TargetVideoFormat::HevcMp4,
                format!("Source is {} ({}) - compressing with HEVC CRF 20", result.codec.as_str(), result.compression.as_str()),
                20.0_f32,
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

/// Simple mode conversion - ALWAYS use HEVC MP4 (High Quality CRF 18)
pub fn simple_convert(input: &Path, output_dir: Option<&Path>) -> Result<ConversionOutput> {
    let detection = detect_video(input)?;
    
    let output_dir = output_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());
    
    std::fs::create_dir_all(&output_dir)?;
    
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    
    // 🔥 当输入是 mp4 时，添加 _hevc 后缀避免冲突
    let output_path = if input_ext.eq_ignore_ascii_case("mp4") {
        output_dir.join(format!("{}_hevc.mp4", stem))
    } else {
        output_dir.join(format!("{}.mp4", stem))
    };
    
    info!("🎬 Simple Mode: {} → HEVC MP4 (CRF 18)", input.display());
    
    let output_size = execute_hevc_conversion(&detection, &output_path, 18)?;
    
    copy_metadata(input, &output_path);
    
    let size_ratio = output_size as f64 / detection.file_size as f64;
    
    info!("   ✅ Complete: {:.1}% of original", size_ratio * 100.0);
    
    Ok(ConversionOutput {
        input_path: input.display().to_string(),
        output_path: output_path.display().to_string(),
        strategy: ConversionStrategy {
            target: TargetVideoFormat::HevcMp4,
            reason: "Simple mode: HEVC High Quality".to_string(),
            command: String::new(),
            preserve_audio: detection.has_audio,
            crf: 18.0,
            lossless: false,
        },
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: true,
        message: "Simple conversion successful (HEVC CRF 18)".to_string(),
        final_crf: 18.0,
        exploration_attempts: 0,
    })
}

/// Auto mode conversion with intelligent strategy selection
pub fn auto_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    let detection = detect_video(input)?;
    let strategy = determine_strategy(&detection);
    
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
            final_crf: 0.0,
            exploration_attempts: 0,
        });
    }

    let output_dir = config.output_dir.clone()
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());
    
    std::fs::create_dir_all(&output_dir)?;
    
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let target_ext = strategy.target.extension();
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    
    // 🔥 当输入输出扩展名相同时，添加 _hevc 后缀避免冲突
    let output_path = if input_ext.eq_ignore_ascii_case(target_ext) {
        output_dir.join(format!("{}_hevc.{}", stem, target_ext))
    } else {
        output_dir.join(format!("{}.{}", stem, target_ext))
    };
    
    if output_path.exists() && !config.force {
        return Err(VidQualityError::ConversionError(
            format!("Output exists: {}", output_path.display())
        ));
    }
    
    info!("🎬 Auto Mode: {} → {}", input.display(), strategy.target.as_str());
    info!("   Reason: {}", strategy.reason);
    
    let (output_size, final_crf, attempts) = match strategy.target {
        TargetVideoFormat::HevcLosslessMkv => {
            info!("   🚀 Using HEVC Lossless Mode");
            let size = execute_hevc_lossless(&detection, &output_path)?;
            (size, 0.0, 0) // 🔥 v3.4: CRF is now f32
        }
        TargetVideoFormat::HevcMp4 => {
            if config.use_lossless {
                info!("   🚀 Using HEVC Lossless Mode (forced)");
                let size = execute_hevc_lossless(&detection, &output_path)?;
                (size, 0.0, 0) // 🔥 v3.4: CRF is now f32
            } else {
                // 🔥 统一使用 shared_utils::video_explorer 处理所有探索模式
                let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
                let input_path = Path::new(&detection.file_path);
                
                let explore_result = if config.explore_smaller && config.match_quality {
                    // 模式 3: --explore + --match-quality 组合（精确质量匹配）
                    let initial_crf = calculate_matched_crf(&detection);
                    info!("   🔬 Precise Quality-Match: CRF {:.1} + SSIM validation", initial_crf);
                    shared_utils::explore_hevc(input_path, &output_path, vf_args, initial_crf)
                } else if config.explore_smaller {
                    // 模式 1: --explore 单独使用（仅探索更小大小）
                    info!("   🔍 Size-Only Exploration: finding smaller output");
                    shared_utils::explore_hevc_size_only(input_path, &output_path, vf_args, strategy.crf)
                } else if config.match_quality {
                    // 模式 2: --match-quality 单独使用（单次编码 + SSIM 验证）
                    let matched_crf = calculate_matched_crf(&detection);
                    info!("   🎯 Quality-Match: CRF {:.1} + SSIM validation", matched_crf);
                    shared_utils::explore_hevc_quality_match(input_path, &output_path, vf_args, matched_crf)
                } else {
                    // 默认模式：使用策略 CRF，单次编码
                    info!("   📦 Default: CRF {:.1}", strategy.crf);
                    shared_utils::explore_hevc_quality_match(input_path, &output_path, vf_args, strategy.crf)
                }.map_err(|e| VidQualityError::ConversionError(e.to_string()))?;
                
                // 打印探索日志
                for log_line in &explore_result.log {
                    info!("{}", log_line);
                }
                
                if !explore_result.quality_passed && (config.match_quality || config.explore_smaller) {
                    warn!("   ⚠️  Quality: SSIM {:.4}", explore_result.ssim.unwrap_or(0.0));
                }
                
                (explore_result.output_size, explore_result.optimal_crf, explore_result.iterations as u8)
            }
        }
        TargetVideoFormat::Skip => unreachable!(),
    };
    
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

/// Calculate CRF to match input video quality level (Enhanced Algorithm for HEVC)
/// 
/// Uses the unified quality_matcher module from shared_utils for consistent
/// quality matching across all tools.
/// 
/// 🔥 v3.5: Uses VideoAnalysisBuilder for full field support:
/// - video_bitrate (separate from total bitrate, 10-30% more accurate)
/// - pix_fmt (chroma subsampling factor)
/// - color_space (HDR detection)
/// 
/// HEVC CRF range is 0-51, with 23 being default "good quality"
/// Clamped to range [0, 32] for practical use (allows visually lossless)
/// 
/// 🔥 v3.4: Returns f32 for sub-integer precision (0.5 step)
pub fn calculate_matched_crf(detection: &VideoDetectionResult) -> f32 {
    // 🔥 v3.5: 使用 VideoAnalysisBuilder 传递完整字段
    let mut builder = shared_utils::VideoAnalysisBuilder::new()
        .basic(
            detection.codec.as_str(),
            detection.width,
            detection.height,
            detection.fps,
            detection.duration_secs,
        )
        .bit_depth(detection.bit_depth)
        .file_size(detection.file_size);
    
    // 🔥 优先使用 video_bitrate（排除音频开销，精度提升 10-30%）
    if let Some(vbr) = detection.video_bitrate {
        builder = builder.video_bitrate(vbr);
    } else {
        // Fallback: 使用总比特率（包含音频）
        builder = builder.video_bitrate(detection.bitrate);
    }
    
    // 🔥 传递 pix_fmt（色度子采样因子）
    if !detection.pix_fmt.is_empty() {
        builder = builder.pix_fmt(&detection.pix_fmt);
    }
    
    // 🔥 传递 color_space（HDR 检测）
    let (color_space_str, is_hdr) = match &detection.color_space {
        crate::detection_api::ColorSpace::BT709 => ("bt709", false),
        crate::detection_api::ColorSpace::BT2020 => ("bt2020nc", true), // BT.2020 通常是 HDR
        crate::detection_api::ColorSpace::SRGB => ("srgb", false),
        crate::detection_api::ColorSpace::AdobeRGB => ("adobergb", false),
        crate::detection_api::ColorSpace::Unknown(_) => ("", false),
    };
    if !color_space_str.is_empty() {
        builder = builder.color(color_space_str, is_hdr);
    }
    
    // 🔥 传递 B-frame 信息（使用 gop 方法）
    if detection.has_b_frames {
        // 假设有 B 帧时使用 GOP=60, B-frames=2
        builder = builder.gop(60, 2);
    }
    
    let analysis = builder.build();
    
    match shared_utils::calculate_hevc_crf(&analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(&analysis, &result, shared_utils::EncoderType::Hevc);
            result.crf // 🔥 v3.4: Already f32 from quality_matcher
        }
        Err(e) => {
            // 🔥 Quality Manifesto: 失败时响亮报错，使用保守值
            warn!("   ⚠️  Quality analysis failed: {}", e);
            warn!("   ⚠️  Using conservative CRF 23.0");
            23.0
        }
    }
}

// 🔥 旧的 explore_smaller_size 函数已被 shared_utils::video_explorer 替代
// 新的探索器支持三种模式：
// 1. SizeOnly (--explore): 仅探索更小的文件大小
// 2. QualityMatch (--match-quality): 使用 AI 预测 CRF + SSIM 验证
// 3. PreciseQualityMatch (--explore + --match-quality): 二分搜索 + SSIM 裁判验证

/// Execute HEVC conversion with specified CRF (using libx265)
fn execute_hevc_conversion(detection: &VideoDetectionResult, output: &Path, crf: u8) -> Result<u64> {
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let x265_params = format!("log-level=error:pools={}", max_threads);
    
    // 🔥 偶数分辨率处理：HEVC 编码器要求宽高为偶数
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    
    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(), max_threads.to_string(),  // 限制 ffmpeg 线程数
        "-i".to_string(), detection.file_path.clone(),
        "-c:v".to_string(), "libx265".to_string(),
        "-crf".to_string(), crf.to_string(),
        "-preset".to_string(), "medium".to_string(),
        "-tag:v".to_string(), "hvc1".to_string(),  // Apple 兼容性
        "-x265-params".to_string(), x265_params,  // 限制 x265 线程池
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

/// Execute HEVC lossless conversion (x265 lossless mode)
fn execute_hevc_lossless(detection: &VideoDetectionResult, output: &Path) -> Result<u64> {
    warn!("⚠️  HEVC Lossless encoding - this will be slow and produce large files!");
    
    // 🔥 性能优化：限制 ffmpeg 线程数，避免系统卡顿
    let max_threads = (num_cpus::get() / 2).clamp(1, 4);
    let x265_params = format!("lossless=1:log-level=error:pools={}", max_threads);
    
    // 🔥 偶数分辨率处理：HEVC 编码器要求宽高为偶数
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    
    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(), max_threads.to_string(),  // 限制 ffmpeg 线程数
        "-i".to_string(), detection.file_path.clone(),
        "-c:v".to_string(), "libx265".to_string(),
        "-x265-params".to_string(), x265_params,  // 限制 x265 线程池
        "-preset".to_string(), "medium".to_string(),
        "-tag:v".to_string(), "hvc1".to_string(),
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

/// Copy metadata and timestamps from source to destination
pub fn copy_metadata(src: &Path, dst: &Path) {
    if let Err(e) = shared_utils::preserve_metadata(src, dst) {
         eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }
}

/// Legacy alias for backward compatibility
pub fn smart_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    auto_convert(input, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_target_format() {
        assert_eq!(TargetVideoFormat::HevcLosslessMkv.extension(), "mkv");
        assert_eq!(TargetVideoFormat::HevcMp4.extension(), "mp4");
    }
}
