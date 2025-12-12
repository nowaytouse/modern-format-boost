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
    /// 🔥 v3.5: Minimum SSIM threshold for quality validation (default: 0.95)
    pub min_ssim: f64,
    /// 🔥 v3.5: Enable VMAF validation (slower but more accurate)
    pub validate_vmaf: bool,
    /// 🔥 v3.5: Minimum VMAF threshold (default: 85.0)
    pub min_vmaf: f64,
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
            min_ssim: 0.95,      // 🔥 v3.5: Default SSIM threshold
            validate_vmaf: false, // 🔥 v3.5: VMAF disabled by default (slower)
            min_vmaf: 85.0,      // 🔥 v3.5: Default VMAF threshold
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
    // 🔥 使用统一的跳过检测逻辑 (shared_utils::should_skip_video_codec)
    // 支持: H.265/HEVC, AV1, VP9, VVC/H.266, AV2
    let skip_decision = shared_utils::should_skip_video_codec(result.codec.as_str());
    
    if skip_decision.should_skip {
        return ConversionStrategy {
            target: TargetVideoFormat::Skip,
            reason: skip_decision.reason,
            command: String::new(),
            preserve_audio: false,
            crf: 0,
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
    
    // 🔥 修复：输出文件已存在时返回跳过状态而非错误
    if output_path.exists() && !config.force {
        info!("⏭️ Output exists, skipping: {}", output_path.display());
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),  // 空路径表示跳过
            strategy: strategy.clone(),
            input_size: detection.file_size,
            output_size: 0,  // 0 表示跳过
            size_ratio: 1.0,
            success: true,
            message: format!("Skipped: output exists ({})", output_path.display()),
            final_crf: 0,
            exploration_attempts: 0,
        });
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
            } else if config.explore_smaller && config.match_quality {
                // 🔥 v3.5: 精确质量匹配模式 (--explore + --match-quality)
                // 二分搜索 + SSIM/VMAF 裁判验证，找到最优质量-大小平衡
                info!("   🔬 Precise Quality-Match Mode: binary search + quality validation");
                explore_precise_quality_match_av1(
                    &detection, 
                    &output_path, 
                    config.min_ssim,
                    config.validate_vmaf,
                    config.min_vmaf,
                )?
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
    
    // 🔥 Safe delete with integrity check (断电保护)
    if config.should_delete_original() {
        if let Err(e) = shared_utils::conversion::safe_delete_original(input, &output_path, 1000) {
            warn!("   ⚠️  Safe delete failed: {}", e);
        } else {
            info!("   🗑️  Original deleted (integrity verified)");
        }
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
/// Uses the unified quality_matcher module from shared_utils for consistent
/// quality matching across all tools.
/// 
/// AV1 CRF range is 0-63, with 23 being default "good quality"
/// Clamped to range [18, 35] for practical use
pub fn calculate_matched_crf(detection: &VideoDetectionResult) -> u8 {
    // 🔥 使用统一的 quality_matcher 模块
    let analysis = shared_utils::from_video_detection(
        &detection.file_path,
        detection.codec.as_str(),
        detection.width,
        detection.height,
        detection.bitrate,
        detection.fps,
        detection.duration_secs,
        detection.has_b_frames,
        detection.bit_depth,
        detection.file_size,
    );
    
    match shared_utils::calculate_av1_crf(&analysis) {
        Ok(result) => {
            shared_utils::log_quality_analysis(&analysis, &result, shared_utils::EncoderType::Av1);
            result.crf.round() as u8
        }
        Err(e) => {
            // 🔥 Quality Manifesto: 失败时响亮报错，使用保守值
            warn!("   ⚠️  Quality analysis failed: {}", e);
            warn!("   ⚠️  Using conservative CRF 28");
            28
        }
    }
}

/// 🔥 v3.5: 精确质量匹配探索 (--explore + --match-quality 组合)
/// 
/// 策略：二分搜索 + SSIM/VMAF 裁判验证
/// 找到满足质量阈值的最高 CRF（最小文件）
/// 
/// ## 裁判机制 (Referee Mechanism)
/// 1. 使用 AI 预测的 CRF 作为起点
/// 2. 二分搜索找到满足 SSIM >= min_ssim 的最高 CRF
/// 3. 可选 VMAF 验证（更准确但更慢）
/// 4. 自校准：如果初始 CRF 不满足质量，向下搜索
/// 
/// ## 评价标准 (Evaluation Criteria)
/// - SSIM >= 0.95: 视觉无损 (Good)
/// - SSIM >= 0.98: 几乎无法区分 (Excellent)
/// - VMAF >= 85: 流媒体质量 (Good)
/// - VMAF >= 93: 存档质量 (Excellent)
fn explore_precise_quality_match_av1(
    detection: &VideoDetectionResult,
    output_path: &Path,
    min_ssim: f64,
    validate_vmaf: bool,
    min_vmaf: f64,
) -> Result<(u64, u8, u8)> {
    use shared_utils::video_explorer::{
        VideoExplorer, VideoEncoder, ExploreConfig, ExploreMode, QualityThresholds
    };
    
    let input_path = std::path::Path::new(&detection.file_path);
    
    // 计算 AI 预测的 CRF
    let initial_crf = calculate_matched_crf(detection);
    
    info!("   🔬 Precise Quality-Match Exploration (AV1)");
    info!("      Input: {} bytes", detection.file_size);
    info!("      Initial CRF: {} (AI predicted)", initial_crf);
    info!("      Min SSIM: {:.4}", min_ssim);
    if validate_vmaf {
        info!("      Min VMAF: {:.1}", min_vmaf);
    }
    
    // 配置探索器
    let config = ExploreConfig {
        mode: ExploreMode::PreciseQualityMatch,
        initial_crf: initial_crf as f32,
        min_crf: 15.0,  // AV1 最低 CRF
        max_crf: 40.0,  // AV1 最高可接受 CRF
        target_ratio: 1.0,  // 目标：输出 <= 输入
        quality_thresholds: QualityThresholds {
            min_ssim,
            min_psnr: 35.0,
            min_vmaf,
            validate_ssim: true,
            validate_psnr: false,
            validate_vmaf,
        },
        max_iterations: 8,  // 最多 8 次迭代
    };
    
    // 获取视频滤镜参数
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    
    // 创建探索器
    let explorer = VideoExplorer::new(
        input_path,
        output_path,
        VideoEncoder::Av1,
        vf_args,
        config,
    ).map_err(|e| VidQualityError::ConversionError(format!("Explorer init failed: {}", e)))?;
    
    // 执行探索
    let result = explorer.explore()
        .map_err(|e| VidQualityError::ConversionError(format!("Exploration failed: {}", e)))?;
    
    // 输出探索日志
    for line in &result.log {
        info!("{}", line);
    }
    
    // 🔥 裁判验证结果
    if result.quality_passed {
        info!("   ✅ Quality validation PASSED");
        if let Some(ssim) = result.ssim {
            info!("      SSIM: {:.4} ({})", ssim, shared_utils::video_explorer::precision::ssim_quality_grade(ssim));
        }
        if let Some(vmaf) = result.vmaf {
            info!("      VMAF: {:.2} ({})", vmaf, shared_utils::video_explorer::precision::vmaf_quality_grade(vmaf));
        }
    } else {
        warn!("   ⚠️  Quality validation FAILED - using best available CRF");
        if let Some(ssim) = result.ssim {
            warn!("      SSIM: {:.4} < {:.4} threshold", ssim, min_ssim);
        }
    }
    
    info!("   📊 Final: CRF {:.1}, {} bytes ({:+.1}%)", 
        result.optimal_crf, result.output_size, result.size_change_pct);
    
    Ok((result.output_size, result.optimal_crf.round() as u8, result.iterations as u8))
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
