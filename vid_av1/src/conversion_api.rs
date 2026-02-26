//! Video Conversion API Module
//!
//! Pure conversion layer - executes video conversions based on detection results.
//! - Auto Mode: FFV1 for lossless sources, AV1 for lossy sources
//! - Simple Mode: Always AV1 MP4
//! - Size Exploration: Tries higher CRF if output is larger than input

use crate::detection_api::{detect_video, CompressionType, VideoDetectionResult};
use crate::{Result, VidQualityError};

use shared_utils::conversion_types::{
    ConversionConfig, ConversionOutput, ConversionStrategy, TargetVideoFormat,
};
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

pub fn determine_strategy(result: &VideoDetectionResult) -> ConversionStrategy {
    determine_strategy_with_apple_compat(result, false)
}

pub fn determine_strategy_with_apple_compat(
    result: &VideoDetectionResult,
    apple_compat: bool,
) -> ConversionStrategy {
    let skip_decision = if apple_compat {
        shared_utils::should_skip_video_codec_apple_compat(result.codec.as_str())
    } else {
        shared_utils::should_skip_video_codec(result.codec.as_str())
    };

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

    let (target, reason, crf, lossless) = match result.compression {
        CompressionType::Lossless => (
            TargetVideoFormat::Av1Mp4,
            format!(
                "Source is {} (lossless) - converting to AV1 Lossless",
                result.codec.as_str()
            ),
            0.0,
            true,
        ),
        CompressionType::VisuallyLossless => (
            TargetVideoFormat::Av1Mp4,
            format!(
                "Source is {} (visually lossless) - compressing with AV1 CRF 0",
                result.codec.as_str()
            ),
            0.0,
            false,
        ),
        _ => (
            TargetVideoFormat::Av1Mp4,
            format!(
                "Source is {} ({}) - compressing with AV1 CRF 0",
                result.codec.as_str(),
                result.compression.as_str()
            ),
            0.0,
            false,
        ),
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

pub fn simple_convert(input: &Path, output_dir: Option<&Path>) -> Result<ConversionOutput> {
    let detection = detect_video(input)?;

    let output_dir = output_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());

    std::fs::create_dir_all(&output_dir)?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");

    let output_path = if input_ext.eq_ignore_ascii_case("mp4") {
        output_dir.join(format!("{}_av1.mp4", stem))
    } else {
        output_dir.join(format!("{}.mp4", stem))
    };

    info!("🎬 Simple Mode: {} → AV1 MP4 (LOSSLESS)", input.display());

    let thread_config = shared_utils::thread_manager::get_balanced_thread_config(
        shared_utils::thread_manager::WorkloadType::Video,
    );
    let output_size = execute_av1_lossless(&detection, &output_path, thread_config.child_threads)?;

    shared_utils::copy_metadata(input, &output_path);

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
            crf: 0.0,
            lossless: true,
        },
        input_size: detection.file_size,
        output_size,
        size_ratio,
        success: true,
        message: "Simple conversion successful (Lossless)".to_string(),
        final_crf: 0.0,
        exploration_attempts: 0,
    })
}

pub fn auto_convert(input: &Path, config: &ConversionConfig) -> Result<ConversionOutput> {
    let detection = detect_video(input)?;
    let strategy = determine_strategy_with_apple_compat(&detection, config.apple_compat);

    if strategy.target == TargetVideoFormat::Skip {
        info!("🎬 Auto Mode: {} → SKIP", input.display());
        info!("   Reason: {}", strategy.reason);

        if let Some(ref out_dir) = config.output_dir {
            let _ = shared_utils::copy_on_skip_or_fail(
                input,
                Some(out_dir),
                config.base_dir.as_deref(),
                false,
            );
        }

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

    let output_dir = config
        .output_dir
        .clone()
        .unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).to_path_buf());

    std::fs::create_dir_all(&output_dir)?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let target_ext = strategy.target.extension();
    let input_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");

    let output_path = if input_ext.eq_ignore_ascii_case(target_ext) {
        output_dir.join(format!("{}_av1.{}", stem, target_ext))
    } else {
        output_dir.join(format!("{}.{}", stem, target_ext))
    };

    shared_utils::path_validator::check_input_output_conflict(input, &output_path)
        .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

    if output_path.exists() && !config.force {
        info!("⏭️ Output exists, skipping: {}", output_path.display());
        return Ok(ConversionOutput {
            input_path: input.display().to_string(),
            output_path: String::new(),
            strategy: strategy.clone(),
            input_size: detection.file_size,
            output_size: 0,
            size_ratio: 1.0,
            success: true,
            message: format!("Skipped: output exists ({})", output_path.display()),
            final_crf: 0.0,
            exploration_attempts: 0,
        });
    }

    info!(
        "🎬 Auto Mode: {} → {}",
        input.display(),
        strategy.target.as_str()
    );
    info!("   Reason: {}", strategy.reason);

    let (output_size, final_crf, attempts) = match strategy.target {
        TargetVideoFormat::Ffv1Mkv => {
            let size = execute_ffv1_conversion(&detection, &output_path, config.child_threads)?;
            (size, 0.0, 0)
        }
        TargetVideoFormat::Av1Mp4 => {
            if strategy.lossless {
                info!("   🚀 Using AV1 Mathematical Lossless Mode");
                let size = execute_av1_lossless(&detection, &output_path, config.child_threads)?;
                (size, 0.0, 0)
            } else {
                let vf_args = shared_utils::get_ffmpeg_dimension_args(
                    detection.width,
                    detection.height,
                    false,
                );
                let input_path = Path::new(&detection.file_path);

                shared_utils::validate_flags_result(
                    config.explore_smaller,
                    config.match_quality,
                    config.require_compression,
                )
                .map_err(VidQualityError::ConversionError)?;

                let initial_crf = calculate_matched_crf(&detection);
                info!(
                    "   🔬 {}: CRF {}",
                    shared_utils::FlagMode::PreciseQualityWithCompress.description_en(),
                    initial_crf
                );
                let explore_result = shared_utils::explore_precise_quality_match_with_compression(
                    input_path,
                    &output_path,
                    shared_utils::VideoEncoder::Av1,
                    vf_args,
                    initial_crf as f32,
                    50.0,
                    config.min_ssim,
                    config.child_threads,
                )
                .map_err(|e| VidQualityError::ConversionError(e.to_string()))?;

                for log_line in &explore_result.log {
                    info!("{}", log_line);
                }

                (
                    explore_result.output_size,
                    explore_result.optimal_crf,
                    explore_result.iterations as u8,
                )
            }
        }
        TargetVideoFormat::Skip => unreachable!(),
        _ => unreachable!("AV1 tool should not return HEVC target"),
    };

    shared_utils::copy_metadata(input, &output_path);

    let size_ratio = output_size as f64 / detection.file_size as f64;

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

pub fn calculate_matched_crf(detection: &VideoDetectionResult) -> u8 {
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
            warn!("   ⚠️  Quality analysis failed: {}", e);
            warn!("   ⚠️  Using conservative CRF 28");
            28
        }
    }
}

fn execute_ffv1_conversion(
    detection: &VideoDetectionResult,
    output: &Path,
    max_threads: usize,
) -> Result<u64> {
    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    let input_arg = shared_utils::safe_path_arg(Path::new(&detection.file_path))
        .as_ref()
        .to_string();
    let output_arg = shared_utils::safe_path_arg(output).as_ref().to_string();

    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(),
        max_threads.to_string(),
        "-i".to_string(),
        input_arg,
        "-c:v".to_string(),
        "ffv1".to_string(),
        "-level".to_string(),
        "3".to_string(),
        "-coder".to_string(),
        "1".to_string(),
        "-context".to_string(),
        "1".to_string(),
        "-g".to_string(),
        "1".to_string(),
        "-slices".to_string(),
        max_threads.to_string(),
        "-slicecrc".to_string(),
        "1".to_string(),
    ];

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        args.extend(vec!["-c:a".to_string(), "flac".to_string()]);
    } else {
        args.push("-an".to_string());
    }

    args.push(output_arg);

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::FFmpegError(
            String::from_utf8_lossy(&result.stderr).to_string(),
        ));
    }

    let size = std::fs::metadata(output).map_err(|e| {
        VidQualityError::ConversionError(format!("Failed to read FFV1 output: {}", e))
    })?;
    let size = size.len();
    if size == 0 {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "FFV1 output file is empty (encoding may have failed)".to_string(),
        ));
    }
    if shared_utils::conversion::get_input_dimensions(output).is_err() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "FFV1 output file is not readable (invalid or corrupted)".to_string(),
        ));
    }

    Ok(size)
}

fn execute_av1_lossless(
    detection: &VideoDetectionResult,
    output: &Path,
    max_threads: usize,
) -> Result<u64> {
    warn!("⚠️  Mathematical lossless AV1 encoding (SVT-AV1) - this will be SLOW!");

    let svt_params = format!("lossless=1:lp={}", max_threads);

    let vf_args = shared_utils::get_ffmpeg_dimension_args(detection.width, detection.height, false);
    let input_arg = shared_utils::safe_path_arg(Path::new(&detection.file_path))
        .as_ref()
        .to_string();
    let output_arg = shared_utils::safe_path_arg(output).as_ref().to_string();

    let mut args = vec![
        "-y".to_string(),
        "-threads".to_string(),
        max_threads.to_string(),
        "-i".to_string(),
        input_arg,
        "-c:v".to_string(),
        "libsvtav1".to_string(),
        "-crf".to_string(),
        "0".to_string(),
        "-preset".to_string(),
        "4".to_string(),
        "-svtav1-params".to_string(),
        svt_params,
    ];

    for arg in &vf_args {
        args.push(arg.clone());
    }

    if detection.has_audio {
        args.extend(vec!["-c:a".to_string(), "flac".to_string()]);
    } else {
        args.push("-an".to_string());
    }

    args.push(output_arg);

    let result = Command::new("ffmpeg").args(&args).output()?;

    if !result.status.success() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::FFmpegError(
            String::from_utf8_lossy(&result.stderr).to_string(),
        ));
    }

    let size = std::fs::metadata(output).map_err(|e| {
        VidQualityError::ConversionError(format!("Failed to read AV1 output: {}", e))
    })?;
    let size = size.len();
    if size == 0 {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "AV1 output file is empty (encoding may have failed)".to_string(),
        ));
    }
    if shared_utils::conversion::get_input_dimensions(output).is_err() {
        let _ = std::fs::remove_file(output);
        return Err(VidQualityError::ConversionError(
            "AV1 output file is not readable (invalid or corrupted)".to_string(),
        ));
    }

    Ok(size)
}

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
